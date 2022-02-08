#[macro_use]
extern crate log;

use std::fs::File;
use std::env;
use std::io::{self, BufRead};
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::rc::Rc;
use indexmap::map::IndexMap;
use librespot_audio::{AudioDecrypt, AudioFile};
use librespot_core::spotify_id::{FileId, SpotifyId};
use librespot_core::{authentication::Credentials, config::SessionConfig, session::Session};
use librespot_metadata::{Album, Artist, Episode, FileFormat, Metadata, Playlist, Show, Track};
use regex::Regex;
use scoped_threadpool::Pool;
use tokio_core::reactor::Core;

enum IndexedTy {
    Track { album: Option<Rc<Album>> },
    Episode { show: Option<Rc<Show>> },
}

type Files = linear_map::LinearMap<FileFormat, FileId>;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args: Vec<_> = std::env::args().collect();
    assert!(
        args.len() == 3 || args.len() == 4,
        "Usage: {} user password [helper_script] < tracks_file",
        args[0]
    );

    let mut core = Core::new().unwrap();
    let session_config = SessionConfig::default();
    let credentials = Credentials::with_password(args[1].to_owned(), args[2].to_owned());
    info!("Connecting ...");
    let session = core
        .run(Session::connect(
            session_config,
            credentials,
            None,
            core.handle(),
        ))
        .unwrap();
    info!("Connected!");

    let mut threadpool = Pool::new(1);

    let re = Regex::new(r"(playlist|track|album|episode|show)[/:]([a-zA-Z0-9]+)").unwrap();

    // As opposed to HashMaps, IndexMaps preserve insertion order.
    let mut ids = IndexMap::new();

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
                let spotify_type = spotify_match.get(1).unwrap().as_str();
                let spotify_id =
                    SpotifyId::from_base62(spotify_match.get(2).unwrap().as_str()).unwrap();

                match spotify_type {
                    "playlist" => ids.extend(
                        core.run(Playlist::get(&session, spotify_id))
                            .unwrap()
                            .tracks
                            .into_iter()
                            .map(|id| (id, IndexedTy::Track { album: None })),
                    ),

                    "album" => {
                        let album = Rc::new(core.run(Album::get(&session, spotify_id)).unwrap());
                        ids.extend(album.tracks.iter().map(|&id| {
                            (
                                id,
                                IndexedTy::Track {
                                    album: Some(album.clone()),
                                },
                            )
                        }));
                    }

                    "show" => {
                        let show = Rc::new(core.run(Show::get(&session, spotify_id)).unwrap());
                        // Since Spotify returns the IDs of episodes in a show in reverse order,
                        // we have to reverse it ourselves again.
                        ids.extend(show.episodes.iter().rev().map(|&id| {
                            (
                                id,
                                IndexedTy::Episode {
                                    show: Some(show.clone()),
                                },
                            )
                        }));
                    }

                    "track" => {
                        ids.insert(spotify_id, IndexedTy::Track { album: None });
                    }

                    "episode" => {
                        ids.insert(spotify_id, IndexedTy::Episode { show: None });
                    }

                    _ => warn!("Unknown link type: {}", spotify_type),
                };
            }

            Err(e) => warn!("ERROR: {}", e),
        }
    }

    for (id, value) in ids {
        let fmtid = id.to_base62();
        match value {
            IndexedTy::Track { album } => {
                info!("Getting track {}...", fmtid);
                if let Ok(mut track) = core.run(Track::get(&session, id)) {
                    if !track.available {
                        warn!("Track {} is not available, finding alternative...", fmtid);
                        let alt_track = track
                            .alternatives
                            .iter()
                            .map(|id| {
                                core.run(Track::get(&session, *id))
                                    .expect("Cannot get track metadata")
                            })
                            .find(|alt_track| alt_track.available);
                        track = match alt_track {
                            Some(x) => {
                                warn!("Found track alternative {} -> {}", fmtid, x.id.to_base62());
                                x
                            }
                            None => {
                                panic!("Could not find alternative for track {}", fmtid);
                            }
                        };
                    }
                    let album = album.unwrap_or_else(|| {
                        Rc::new(
                            core.run(Album::get(&session, track.album))
                                .expect("Cannot get album"),
                        )
                    });
                    let artists_strs: Vec<_> = track
                        .artists
                        .iter()
                        .map(|id| {
                            core.run(Artist::get(&session, *id))
                                .expect("Cannot get artist metadata")
                                .name
                        })
                        .collect();
                    handle_entry(
                        &mut core,
                        &mut threadpool,
                        &session,
                        &args[..],
                        track.id,
                        &album.covers,
                        &track.files,
                        &track.name,
                        &album.name,
                        &artists_strs,
                    );
                }
            }

            IndexedTy::Episode { show } => {
                info!("Getting episode {}...", fmtid);
                if let Ok(episode) = core.run(Episode::get(&session, id)) {
                    if !episode.available {
                        warn!("Episode {} is not available.", fmtid);
                    }
                    let show = show.unwrap_or_else(|| {
                        Rc::new(
                            core.run(Show::get(&session, episode.show))
                                .expect("Cannot get show"),
                        )
                    });
                    handle_entry(
                        &mut core,
                        &mut threadpool,
                        &session,
                        &args[..],
                        episode.id,
                        &show.covers,
                        &episode.files,
                        &episode.name,
                        &show.name,
                        &[show.publisher.clone()],
                    );
                }
            }
        }
    }
}

fn handle_entry(
    core: &mut Core,
    threadpool: &mut Pool,
    session: &Session,
    args: &[String],
    entry_id: SpotifyId,
    covers: &Vec<FileId>,
    files: &Files,
    entry_name: &str,
    group_name: &str,
    origins: &[String],
) {
    let path = env::var("PATH_DIR").unwrap_or("".to_string());
    let fmtid = entry_id.to_base62();
    let cover_id = covers.last().unwrap().to_base16();
    let mut fname = sanitize_filename::sanitize(format!("{} - {}.ogg", origins.join(", "), entry_name));
    fname = format!("{}{}", path, fname);
    if Path::new(&fname).exists() {
        info!("File {} already exists.", fname);
        return;
    }
    debug!(
        "File formats:{}",
        files.keys().fold(String::new(), |mut acc, filetype| {
            acc.push(' ');
            acc += &format!("{:?}", filetype);
            acc
        })
    );
    let url = format!("https://i.scdn.co/image/{}", cover_id);
    let mut image_file = reqwest::get(&url).unwrap();
    let mut file = File::create(format!("{}cover.jpg", path)).expect("failed to create file");
    io::copy(&mut image_file, &mut file).expect("failed to copy content");
    let file_id = *files
    .get(&FileFormat::OGG_VORBIS_320)
    .or_else(|| files.get(&FileFormat::OGG_VORBIS_160))
    .or_else(|| files.get(&FileFormat::OGG_VORBIS_96))
        .expect("Could not find a OGG_VORBIS format for the track.");
    let key = core
        .run(session.audio_key().request(entry_id, file_id))
        .expect("Cannot get audio key");
    let mut encrypted_file = core
        .run(AudioFile::open(&session, file_id, 320, true))
        .unwrap();
    let mut buffer = Vec::new();
    {
        use std::sync::atomic::{AtomicBool, Ordering};
        let dur = std::time::Duration::from_millis(100);
        let fetched = AtomicBool::new(false);
        let mut read_all = Ok(0);
        threadpool.scoped(|scope| {
            scope.execute(|| {
                read_all = encrypted_file.read_to_end(&mut buffer);
                fetched.store(true, Ordering::Release);
            });
            while !fetched.load(Ordering::Acquire) {
                core.turn(Some(dur));
            }
        });
        read_all.expect("Cannot read file stream");
    }
    let mut decrypted_buffer = Vec::new();
    AudioDecrypt::new(key, &buffer[..])
        .read_to_end(&mut decrypted_buffer)
        .expect("Cannot decrypt stream");
    let decrypted_buffer = &decrypted_buffer[0xa7..];
    if args.len() == 3 {
        std::fs::write(&fname, decrypted_buffer).expect("Cannot write decrypted audio stream");
        info!("Filename: {}", fname);
    } else {
        let mut cmd = Command::new(&args[3]);
        cmd.stdin(Stdio::piped());
        cmd.arg(fmtid)
            .arg(entry_name)
            .arg(group_name)
            .args(origins.iter().map(|i| i.as_str()));
        let mut child = cmd.spawn().expect("Could not run helper program");
        let pipe = child.stdin.as_mut().expect("Could not open helper stdin");
        pipe.write_all(decrypted_buffer)
            .expect("Failed to write to stdin");
        assert!(
            child
                .wait()
                .expect("Out of ideas for error messages")
                .success(),
            "Helper script returned an error"
        );
    }
}
