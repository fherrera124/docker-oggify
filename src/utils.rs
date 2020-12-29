use librespot_core::spotify_id::FileId;
use librespot_metadata::FileFormat;
use std::fmt;
use std::io::Write;
use std::process::{Command, Stdio};

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
