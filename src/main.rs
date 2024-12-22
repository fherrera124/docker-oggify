#[macro_use]
extern crate log;

mod loader;

use loader::TrackLoader;

use std::{
    process::{Command, Stdio, exit},
    fs::{create_dir_all},
    env,
    io::{self, BufRead, Write},
    collections::HashSet,
    path::{Path, PathBuf}
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
    audio::UniqueFields
};
use tokio::time::{sleep, Duration};
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

    let cache = match Cache::new(Some("/data/.cache"), None, Some("/data/.cache"), None) {
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

    let output_path = PathBuf::from("/data/tracks");
    if let Err(e) = create_dir_all(&output_path) {
        error!("could not create or access the directory: {}", e);
        exit(1);
    }

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
    
    let loader = TrackLoader {
        session: session.clone(),
    };
    for spotify_id in ids {
        if let Err(e) = process_audio_item(&loader, spotify_id, &output_path).await {
            warn!("Error processing audio item: {}", e);
            // TODO: add item failed to file of items failed
            continue;
        }
        // to avoid service unavailable...
        sleep(Duration::from_secs(10)).await;
    }
}

fn usage(program: &str, opts: &getopts::Options) -> String {
    let brief = format!("Usage: {program} [<Options>]");
    opts.usage(&brief)
}

fn run_helper_script(
    track_id: &str,
    cover_url: &str,
    full_path_str: &str,
    track_title: &str,
    group_name: &str,
    origins: Vec<&str>,
    audio_buffer: &[u8]
) -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::new("tag_ogg.sh");
    cmd.arg(track_id)
        .arg(track_title)
        .arg(group_name)
        .arg(full_path_str)
        .arg(cover_url)
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
    loader: &TrackLoader,
    spotify_id: SpotifyId,
    tracks_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {

    let (audio_item, audio_buffer) = match loader.load_track(spotify_id).await {
        Some(track_data) => (track_data.audio_item, track_data.audio_buffer),
        None => {
            return Err(format!("<{}> is not available", spotify_id.to_uri().unwrap_or_default()).into());
        }
    };

    let (origins, group_name) = match &audio_item.unique_fields {
        UniqueFields::Track {
            artists,
            album,
            album_artists, ..
        } => {
            (artists
                .0
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<&str>>(),
                album.to_string())
            },
            _ => (Vec::new(), "test".to_string())
        };

    let cover = audio_item
        .covers
        .first()
        .ok_or("No covers available for this audio item")?;

    let track_id = audio_item.track_id.to_base62()?;
    let fname = sanitize_filename::sanitize(format!("{} - {}", audio_item.name, origins.join(", ")));

    let full_path = tracks_path.join(format!("{}.ogg", &fname));
    if full_path.exists() {
        info!("File '{}' already exists.", full_path.to_str().unwrap());
        return Ok(());
    }

    run_helper_script(
        &track_id,
        &cover.url,
        full_path.to_str().unwrap(),
        &audio_item.name,
        &group_name,
        origins,
        &audio_buffer
    )?;
    Ok(())
}
