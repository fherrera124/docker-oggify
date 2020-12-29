#[macro_use]
extern crate log;

use std::io::{self, BufRead};
use std::path::Path;
use std::{env, panic};

use env_logger::{Builder, Env};
use indexmap::map::IndexMap;
use librespot_audio::AudioFile;
use librespot_core::authentication::Credentials;
use librespot_core::config::SessionConfig;
use librespot_core::session::Session;
use librespot_core::spotify_id::SpotifyId;
use librespot_metadata::{Album, Artist, Episode, Metadata, Playlist, Show, Track};
use regex::Regex;
use tokio_core::reactor::Core;

mod utils;
use utils::*;

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
        let fmtid = id.to_base62();
        info!("Getting {} {}...", value, fmtid);
        match value {
            Track => {
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
                    print_file_formats(&track.files);
                    let file_id = get_usable_file_id(&track.files);
                    let fname = sanitize_filename::sanitize(format!(
                        "{} - {}.ogg",
                        artists_strs.join(", "),
                        track.name
                    ));

                    if Path::new(&fname).exists() {
                        info!("File {} already exists.", fname);
                    } else {
                        let key = core
                            .run(session.audio_key().request(track.id, *file_id))
                            .expect("Cannot get audio key");
                        let encrypted_file = core
                            .run(AudioFile::open(&session, *file_id, 320, true))
                            .unwrap();
                        write_to_disk(
                            &args[..],
                            &fmtid,
                            &track.name,
                            || {
                                let album = core
                                    .run(Album::get(&session, track.album))
                                    .expect("Cannot get album metadata");
                                album.name
                            },
                            &artists_strs,
                            key,
                            encrypted_file,
                        );
                    }
                }
            }

            Episode => {
                if let Ok(episode) = core.run(Episode::get(&session, id)) {
                    if !episode.available {
                        warn!("Episode {} is not available.", fmtid);
                    }
                    let show = core
                        .run(Show::get(&session, episode.show))
                        .expect("Cannot get show");
                    print_file_formats(&episode.files);
                    let file_id = get_usable_file_id(&episode.files);
                    let fname = format!("{} - {}.ogg", show.publisher, episode.name);
                    if Path::new(&fname).exists() {
                        info!("File {} already exists.", fname);
                    } else {
                        let key = core
                            .run(session.audio_key().request(episode.id, *file_id))
                            .expect("Cannot get audio key");
                        let encrypted_file = core
                            .run(AudioFile::open(&session, *file_id, 320, true))
                            .unwrap();
                        let sname = &show.name;
                        write_to_disk(
                            &args[..],
                            &fmtid,
                            &episode.name,
                            || sname,
                            &[show.publisher],
                            key,
                            encrypted_file,
                        );
                    }
                }
            }
        }
    }
}
