use librespot_audio::{AudioDecrypt, AudioFile};
use librespot_core::session::Session;
use librespot_core::spotify_id::{FileId, SpotifyId};
use librespot_metadata::FileFormat;
use std::fmt;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use tokio_core::reactor::Core;

pub enum IndexedTy {
    Track,
    Episode,
}

pub use IndexedTy::*;

pub type Files = linear_map::LinearMap<FileFormat, FileId>;

pub fn get_usable_file_id(files: &Files) -> &FileId {
    files
        .get(&FileFormat::OGG_VORBIS_320)
        .or_else(|| files.get(&FileFormat::OGG_VORBIS_160))
        .or_else(|| files.get(&FileFormat::OGG_VORBIS_96))
        .expect("Could not find a OGG_VORBIS format for the track.")
}

pub fn print_file_formats(files: &Files) {
    debug!(
        "File formats:{}",
        files.keys().fold(String::new(), |mut acc, filetype| {
            acc.push(' ');
            acc += &format!("{:?}", filetype);
            acc
        })
    );
}

pub fn run_helper<'a>(
    helper_path: &'a str,
    fmtid: &'a str,
    element: &'a str,
    group: &'a str,
    origins: impl Iterator<Item = &'a str>,
    decrypted_buffer: &[u8],
) {
    let mut cmd = Command::new(helper_path);
    cmd.stdin(Stdio::piped());
    cmd.arg(fmtid).arg(element).arg(group).args(origins);
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

pub fn write_to_disk<'a, 'c, GG, GR>(
    core: &'c mut Core,
    session: &'c Session,
    args: &'a [String],
    track_id: SpotifyId,
    files: &Files,
    fmtid: &'a str,
    element: &'a str,
    group_getter: GG,
    origins: &[String],
) where
    GG: FnOnce(&'c mut Core) -> GR,
    GR: AsRef<str>,
{
    let fname = sanitize_filename::sanitize(format!("{} - {}.ogg", origins.join(", "), element));
    if Path::new(&fname).exists() {
        info!("File {} already exists.", fname);
        return;
    }
    print_file_formats(files);
    let file_id = *get_usable_file_id(files);
    let key = core
        .run(session.audio_key().request(track_id, file_id))
        .expect("Cannot get audio key");
    let mut encrypted_file = core
        .run(AudioFile::open(&session, file_id, 320, true))
        .unwrap();
    let mut buffer = Vec::new();
    encrypted_file
        .read_to_end(&mut buffer)
        .expect("Cannot read file stream");
    let mut decrypted_buffer = Vec::new();
    AudioDecrypt::new(key, &buffer[..])
        .read_to_end(&mut decrypted_buffer)
        .expect("Cannot decrypt stream");
    let decrypted_buffer = &decrypted_buffer[0xa7..];
    if args.len() == 3 {
        if Path::new(&fname).exists() {
            info!("File {} already exists.", fname);
        } else {
            std::fs::write(&fname, decrypted_buffer).expect("Cannot write decrypted audio stream");
            info!("Filename: {}", fname);
        }
    } else {
        run_helper(
            &args[3],
            fmtid,
            element,
            group_getter(core).as_ref(),
            origins.iter().map(|i| i.as_str()),
            decrypted_buffer,
        );
    }
}

impl fmt::Display for IndexedTy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Track => "track",
                Episode => "episode",
            }
        )
    }
}
