#[macro_use]
extern crate log;

use std::io::Write;
use std::io::{self, BufRead, Read, Result};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::{env, panic};

use env_logger::{Builder, Env};
use indexmap::map::IndexMap;
use librespot_audio::{AudioDecrypt, AudioFile};
use librespot_core::authentication::Credentials;
use librespot_core::config::SessionConfig;
use librespot_core::session::Session;
use librespot_core::spotify_id::{FileId, SpotifyId};
use librespot_metadata::{Album, Artist, Episode, FileFormat, Metadata, Playlist, Show, Track};
use regex::Regex;
use scoped_threadpool::Pool;
use tokio_core::reactor::Core;

enum IndexedTy {
    Track,
    Episode,
}

use IndexedTy::*;

fn get_usable_file_id(files: &linear_map::LinearMap<FileFormat, FileId>) -> &FileId {
    files
        .get(&FileFormat::OGG_VORBIS_320)
        .or_else(|| files.get(&FileFormat::OGG_VORBIS_160))
        .or_else(|| files.get(&FileFormat::OGG_VORBIS_96))
        .expect("Could not find a OGG_VORBIS format for the track.")
}

fn main() {
    Builder::from_env(Env::default().default_filter_or("info")).init();

    let args: Vec<_> = env::args().collect();
    assert!(
        args.len() == 3 || args.len() == 4,
        "Usage: {} user password [helper_script] < tracks_file",
        args[0]
    );

    let mut core = Core::new().unwrap();
    let handle = core.handle();
    let session_config = SessionConfig::default();
    let credentials = Credentials::with_password(args[1].to_owned(), args[2].to_owned());
    info!("Connecting ...");
    let session = core
        .run(Session::connect(session_config, credentials, None, handle))
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
                let spotify_captures = re.captures(line);
                let spotify_match = match spotify_captures {
                    None => continue,
                    Some(x) => x,
                };
                let spotify_type = spotify_match.get(1).unwrap().as_str();
                let spotify_id =
                    SpotifyId::from_base62(spotify_match.get(2).unwrap().as_str()).unwrap();

                match spotify_type {
                    "playlist" => {
                        let playlist = core.run(Playlist::get(&session, spotify_id)).unwrap();
                        ids.extend(playlist.tracks.into_iter().map(|id| (id, Track)));
                    }

                    "album" => {
                        let album = core.run(Album::get(&session, spotify_id)).unwrap();
                        ids.extend(album.tracks.into_iter().map(|id| (id, Track)));
                    }

                    "show" => {
                        let show = core.run(Show::get(&session, spotify_id)).unwrap();
                        // Since Spotify returns the IDs of episodes in a show in reverse order,
                        // we have to reverse it ourselves again.
                        ids.extend(show.episodes.into_iter().rev().map(|id| (id, Episode)));
                    }

                    "track" => {
                        ids.insert(spotify_id, Track);
                    }

                    "episode" => {
                        ids.insert(spotify_id, Episode);
                    }

                    _ => warn!("Unknown link type."),
                };
            }

            Err(e) => warn!("ERROR: {}", e),
        }
    }

    for (id, value) in ids {
        match value {
            Track => {
                let fmtid = id.to_base62();
                info!("Getting track {}...", id.to_base62());
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
                    let artists_strs: Vec<_> = track
                        .artists
                        .iter()
                        .map(|id| {
                            core.run(Artist::get(&session, *id))
                                .expect("Cannot get artist metadata")
                                .name
                        })
                        .collect();
                    debug!(
                        "File formats: {}",
                        track
                            .files
                            .keys()
                            .map(|filetype| format!("{:?}", filetype))
                            .collect::<Vec<_>>()
                            .join(" ")
                    );
                    let file_id = get_usable_file_id(&track.files);
                    let key = core
                        .run(session.audio_key().request(track.id, *file_id))
                        .expect("Cannot get audio key");
                    let mut encrypted_file = core
                        .run(AudioFile::open(&session, *file_id, 320, true))
                        .unwrap();
                    let mut buffer = Vec::new();
                    let mut read_all: Result<usize> = Ok(0);
                    let fname = sanitize_filename::sanitize(format!(
                        "{} - {}.ogg",
                        artists_strs.join(", "),
                        track.name
                    ));

                    if Path::new(&fname).exists() {
                        info!("File {} already exists.", fname);
                    } else {
                        let fetched = AtomicBool::new(false);
                        threadpool.scoped(|scope| {
                            scope.execute(|| {
                                read_all = encrypted_file.read_to_end(&mut buffer);
                                fetched.store(true, Ordering::Release);
                            });
                            while !fetched.load(Ordering::Acquire) {
                                core.turn(Some(Duration::from_millis(100)));
                            }
                        });
                        read_all.expect("Cannot read file stream");
                        let mut decrypted_buffer = Vec::new();
                        AudioDecrypt::new(key, &buffer[..])
                            .read_to_end(&mut decrypted_buffer)
                            .expect("Cannot decrypt stream");
                        if args.len() == 3 {
                            let fname = sanitize_filename::sanitize(format!(
                                "{} - {}.ogg",
                                artists_strs.join(", "),
                                track.name
                            ));
                            if Path::new(&fname).exists() {
                                info!("File {} already exists.", fname);
                            } else {
                                std::fs::write(&fname, &decrypted_buffer[0xa7..])
                                    .expect("Cannot write decrypted track");
                                info!("Filename: {}", fname);
                            }
                        } else {
                            let album = core
                                .run(Album::get(&session, track.album))
                                .expect("Cannot get album metadata");
                            let mut cmd = Command::new(args[3].to_owned());
                            cmd.stdin(Stdio::piped());
                            cmd.arg(id.to_base62())
                                .arg(track.name)
                                .arg(album.name)
                                .args(artists_strs.iter());
                            let mut child = cmd.spawn().expect("Could not run helper program");
                            let pipe = child.stdin.as_mut().expect("Could not open helper stdin");
                            pipe.write_all(&decrypted_buffer[0xa7..])
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
                }
            }

            Episode => {
                let fmtid = id.to_base62();
                info!("Getting episode {}...", fmtid);
                if let Ok(episode) = core.run(Episode::get(&session, id)) {
                    if !episode.available {
                        warn!("Episode {} is not available.", fmtid);
                    }
                    let show = core
                        .run(Show::get(&session, episode.show))
                        .expect("Cannot get show");
                    debug!(
                        "File formats: {}",
                        episode
                            .files
                            .keys()
                            .map(|filetype| format!("{:?}", filetype))
                            .collect::<Vec<_>>()
                            .join(" ")
                    );
                    let file_id = get_usable_file_id(&episode.files);
                    let key = core
                        .run(session.audio_key().request(episode.id, *file_id))
                        .expect("Cannot get audio key");
                    let mut encrypted_file = core
                        .run(AudioFile::open(&session, *file_id, 320, true))
                        .unwrap();
                    let mut buffer = Vec::new();
                    let mut read_all: Result<usize> = Ok(0);
                    let fname = format!("{} - {}.ogg", show.publisher, episode.name);
                    if Path::new(&fname).exists() {
                        info!("File {} already exists.", fname);
                    } else {
                        let fetched = AtomicBool::new(false);
                        threadpool.scoped(|scope| {
                            scope.execute(|| {
                                read_all = encrypted_file.read_to_end(&mut buffer);
                                fetched.store(true, Ordering::Release);
                            });
                            while !fetched.load(Ordering::Acquire) {
                                core.turn(Some(Duration::from_millis(100)));
                            }
                        });
                        read_all.expect("Cannot read file stream");
                        let mut decrypted_buffer = Vec::new();
                        AudioDecrypt::new(key, &buffer[..])
                            .read_to_end(&mut decrypted_buffer)
                            .expect("Cannot decrypt stream");
                        if args.len() == 3 {
                            if Path::new(&fname).exists() {
                                info!("File {} already exists.", fname);
                            } else {
                                std::fs::write(&fname, &decrypted_buffer[0xa7..])
                                    .expect("Cannot write decrypted episode");
                                info!("Filename: {}", fname);
                            }
                        } else {
                            let mut cmd = Command::new(args[3].to_owned());
                            cmd.stdin(Stdio::piped());
                            cmd.arg(id.to_base62())
                                .arg(episode.name)
                                .arg(show.name)
                                .arg(show.publisher);
                            let mut child = cmd.spawn().expect("Could not run helper program");
                            let pipe = child.stdin.as_mut().expect("Could not open helper stdin");
                            pipe.write_all(&decrypted_buffer[0xa7..])
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
                }
            }
        }
    }
}
