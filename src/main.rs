#[macro_use]
extern crate log;

mod loader;

use std::{
    process::{Command, Stdio, exit},
    fs::File,
    env,
    io::{self, BufRead, Read, Write},
    collections::HashSet,
    path::Path
};
use librespot_oauth::get_access_token;
use librespot_core::{
    cache::Cache,
    spotify_id::{SpotifyId, SpotifyItemType},
    authentication::Credentials,
    config::SessionConfig,
    session::Session
};
use librespot_metadata::{
    Album,
    Metadata,
    Playlist,
    audio::{UniqueFields, AudioItem}
};
use regex::Regex;


#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut opts = getopts::Options::new();

    opts.optopt(
        "u",
        "username",
        "Username used to sign in with.",
        "USERNAME",
    )
    .optopt(
        "k",
        "access-token",
        "Spotify access token to sign in with.",
        "TOKEN",
    )
    .optopt(
        "s",
        "helper-script",
        "helper script to convert track.",
        "SCRIPT",
    );

    let args: Vec<_> = env::args().collect();

    let matches = match opts.parse(&args[1..]) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("Error parsing command line options: {e}");
            println!("\n{}", usage(&args[0], &opts));
            exit(1);
        }
    };
    let opt_str = |opt| {
        if matches.opt_present(opt) {
            matches.opt_str(opt)
        } else {
            None
        }
    };
    let empty_string_error_msg = |long: &str, short: &str| {
        error!("`--{long}` / `-{short}` can not be an empty string");
        exit(1);
    };

    let session_config = SessionConfig::default();

    let mut audio_dir = std::env::var("PATH_DIR").unwrap_or_else(|_| "".to_string());
    let creds_dir = format!("{}/creds", audio_dir.trim_end_matches('/'));
    audio_dir = format!("{}/cache", audio_dir.trim_end_matches('/'));

    let cache = match Cache::new(Some(creds_dir), None, Some(audio_dir), None) {
        Ok(cache) => Some(cache),
        Err(e) => {
            warn!("Cannot create cache: {}", e);
            None
        }
    };

    let credentials = {
        let cached_creds = cache.as_ref().and_then(Cache::credentials);

        if let Some(access_token) = opt_str("access-token") {
            if access_token.is_empty() {
                empty_string_error_msg("access-token", "k");
            }
            Some(Credentials::with_access_token(access_token))
        } else if let Some(username) = opt_str("username") {
            if username.is_empty() {
                empty_string_error_msg("username", "u");
            }
            match cached_creds {
                Some(creds) if Some(username) == creds.username => {
                    trace!("Using cached credentials for specified username.");
                    Some(creds)
                }
                _ => {
                    trace!("No cached credentials for specified username.");
                    None
                }
            }
        } else {
            if cached_creds.is_some() {
                trace!("Using cached credentials.");
                cached_creds
            } else {
                let access_token = match get_access_token(
                    &session_config.client_id,
                    &format!("http://127.0.0.1:1234/login"),
                    vec!["streaming"],
                ) {
                    Ok(token) => token.access_token,
                    Err(e) => {
                        error!("Failed to get Spotify access token: {e}");
                        exit(1);
                    }
                };
                Some(Credentials::with_access_token(access_token))
            }
        }
    };

    let script_option = opt_str("helper-script"); // Guardar el Option<String>

    let helper_script = script_option
        .as_deref()
        .and_then(|script| {
            if script.is_empty() {
                empty_string_error_msg("helper-script", "s");
                None
            } else {
                Some(script)
            }
    });

    let session = Session::new(session_config, cache);

    if let Err(e) = session.connect(credentials.clone().unwrap_or_default(), true).await {
        println!("Error connecting: {}", e);
        exit(1);
    }


    info!("Connected!");

    let re = Regex::new(r"(playlist|track|album)[/:]([a-zA-Z0-9]+)").unwrap();
    let mut ids = HashSet::new();

    for line in io::stdin().lock().lines() {
        match line {
            Ok(line) => {
                let line = line.trim();
                if line == "done" {
                    break;
                }
                let spotify_match = match re.captures(line) {
                    None => continue,
                    Some(x) => x,
                };
                let item_type_str = spotify_match.get(1).unwrap().as_str();
                let mut spotify_id =
                    SpotifyId::from_base62(spotify_match.get(2).unwrap().as_str()).unwrap();
                spotify_id.item_type = SpotifyItemType::from(item_type_str);

                match spotify_id.item_type {
                    SpotifyItemType::Playlist => ids.extend(
                        Playlist::get(&session, &spotify_id)
                            .await
                            .unwrap()
                            .tracks()
                            .cloned(),
                    ),
                    SpotifyItemType::Album => ids.extend(
                        Album::get(&session, &spotify_id)
                            .await
                            .unwrap()
                            .tracks()
                            .cloned(),
                    ),
                    SpotifyItemType::Track => {
                        spotify_id.item_type = SpotifyItemType::Track;
                        ids.insert(spotify_id);
                    },
                    _ => warn!("Unknown/unsuported item type: {}", item_type_str),
                };
            }
            Err(e) => warn!("ERROR: {}", e),
        }
    }

    for id in ids {
        download_track(&session, id, helper_script).await;
    }
}

fn usage(program: &str, opts: &getopts::Options) -> String {
    let brief = format!("Usage: {program} [<Options>]");
    opts.usage(&brief)
}

async fn download_track(
    session: &Session,
    spotify_id: SpotifyId,
    helper_script: Option<&str>,
) {

    let loader = loader::TrackLoader {
        session: session.clone(),
    };

    let (audio_item, audio_buffer) = match loader.load_track(spotify_id).await {
        Some(track_data) => (track_data.audio_item, track_data.audio_buffer),
        None => {
            warn!(
                "<{}> is not available",
                spotify_id.to_uri().unwrap_or_default()
            );
            return;
        }
    };

    let (origins, group_name) = match audio_item.unique_fields {
        UniqueFields::Track {
            artists,
            album,
            album_artists, ..
        } => {
                (artists
                    .0
                    .into_iter()
                    .map(|a| a.name)
                    .collect::<Vec<String>>(),
                album)
        },
        _ => (Vec::new(), "test".to_string())
    };

    let path = env::var("PATH_DIR").unwrap_or("".to_string());
    let mut fname = sanitize_filename::sanitize(format!("{} - {}.ogg", origins.join(", "), audio_item.name));
    fname = format!("{}{}", path, fname);
    if Path::new(&fname).exists() {
        info!("File {} already exists.", fname);
        return;
    }

    if let Some(script) = helper_script {
        let cover = audio_item.covers.first().unwrap();

        let response = match reqwest::get(&cover.url).await {
            Ok(response) => response.bytes(),
            Err(e) => {
                eprintln!("Error al obtener datos: {}", e);
                return;
            }
        };
        let image_file = match response.await {
            Ok(bytes) => bytes,
            Err(e) => {
                eprintln!("Error al obtener los bytes de la respuesta: {}", e);
                return;
            }
        };

        let mut file = File::create(format!("{}cover.jpg", path)).expect("failed to create file");
        io::copy(&mut image_file.as_ref(), &mut file).expect("failed to copy content");

        let mut cmd = Command::new(script);
        cmd.stdin(Stdio::piped());
        let track_id = match audio_item.track_id.to_base62() {
            Ok(id) => id,
            Err(e) => {
                eprintln!("Error al convertir track_id a base62: {}", e);
                return;
            }
        };
        cmd.arg(track_id)
            .arg(audio_item.name)
            .arg(group_name)
            .args(origins);
        let mut child = cmd.spawn().expect("Could not run helper program");
        let pipe = child.stdin.as_mut().expect("Could not open helper stdin");
        pipe.write_all(&audio_buffer)
            .expect("Failed to write to stdin");
        assert!(
            child
                .wait()
                .expect("Out of ideas for error messages")
                .success(),
            "Helper script returned an error"
        );
    } else {
        std::fs::write(&fname, audio_buffer).expect("Cannot write decrypted audio stream");
    }

    info!("Filename: {}", fname);
}

async fn download_cover(url: &str, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let response = reqwest::get(url).await?.bytes().await?;
    let mut file = File::create(path)?;
    io::copy(&mut response.as_ref(), &mut file)?;
    Ok(())
}

fn run_helper_script(
    script: &str,
    track_id: &str,
    name: &str,
    group_name: &str,
    origins: &[String],
    audio_buffer: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::new(script);
    cmd.arg(track_id)
        .arg(name)
        .arg(group_name)
        .args(origins)
        .stdin(Stdio::piped());

    let mut child = cmd.spawn()?;
    let pipe = child.stdin.as_mut().ok_or("Failed to open helper stdin")?;
    pipe.write_all(audio_buffer)?;
    let status = child.wait()?;
    if !status.success() {
        return Err("Helper script returned an error".into());
    }
    Ok(())
}

async fn process_audio_item(
    helper_script: Option<&str>,
    audio_item: AudioItem,
    group_name: &str,
    origins: &[String],
    audio_buffer: &[u8],
    path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(script) = helper_script {
        let cover = audio_item
            .covers
            .first()
            .ok_or("No covers available for this audio item")?;
        let cover_path = path.join("cover.jpg");

        // Descargar la portada
        download_cover(&cover.url, &cover_path).await?;

        // Convertir el track_id
        let track_id = audio_item.track_id.to_base62()?;

        // Ejecutar el script auxiliar
        run_helper_script(
            script,
            &track_id,
            &audio_item.name,
            group_name,
            origins,
            audio_buffer,
        )?;
    }
    Ok(())
}


