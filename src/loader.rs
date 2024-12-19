use std::{
    io::Read,
};
use librespot_audio::{AudioDecrypt, AudioFile};
use librespot_core::{
    spotify_id::SpotifyId,
    session::Session
};
use librespot_metadata::{
    audio::{AudioFileFormat, AudioItem}
};
use futures_util::{
    future, stream::futures_unordered::FuturesUnordered, StreamExt,
};


pub struct TrackLoader {
    pub session: Session,
}

pub struct LoadedTrackData {
    pub audio_item: AudioItem,
    pub audio_buffer: Vec<u8>,
}


impl TrackLoader {
    async fn find_available_alternative(&self, audio_item: AudioItem) -> Option<AudioItem> {
        if let Err(e) = audio_item.availability {
            error!("Track is unavailable: {}", e);
            None
        } else if !audio_item.files.is_empty() {
            Some(audio_item)
        } else if let Some(alternatives) = &audio_item.alternatives {
            let alternatives: FuturesUnordered<_> = alternatives
                .iter()
                .map(|alt_id| AudioItem::get_file(&self.session, *alt_id))
                .collect();

            alternatives
                .filter_map(|x| future::ready(x.ok()))
                .filter(|x| future::ready(x.availability.is_ok()))
                .next()
                .await
        } else {
            error!("Track should be available, but no alternatives found.");
            None
        }
    }

    fn stream_data_rate(&self, format: AudioFileFormat) -> Option<usize> {
        let kbps = match format {
            AudioFileFormat::OGG_VORBIS_96 => 12,
            AudioFileFormat::OGG_VORBIS_160 => 20,
            AudioFileFormat::OGG_VORBIS_320 => 40,
            AudioFileFormat::MP3_256 => 32,
            AudioFileFormat::MP3_320 => 40,
            AudioFileFormat::MP3_160 => 20,
            AudioFileFormat::MP3_96 => 12,
            AudioFileFormat::MP3_160_ENC => 20,
            AudioFileFormat::AAC_24 => 3,
            AudioFileFormat::AAC_48 => 6,
            AudioFileFormat::AAC_160 => 20,
            AudioFileFormat::AAC_320 => 40,
            AudioFileFormat::MP4_128 => 16,
            AudioFileFormat::OTHER5 => 40,
            AudioFileFormat::FLAC_FLAC => 112, // assume 900 kbit/s on average
            AudioFileFormat::UNKNOWN_FORMAT => {
                error!("Unknown stream data rate");
                return None;
            }
        };
        Some(kbps * 1024)
    }


    pub async fn load_track(
        &self,
        spotify_id: SpotifyId,
    ) -> Option<LoadedTrackData> {
        let audio_item = match AudioItem::get_file(&self.session, spotify_id).await {
            Ok(audio) => match self.find_available_alternative(audio).await {
                Some(audio) => audio,
                None => {
                    warn!(
                        "<{}> is not available",
                        spotify_id.to_uri().unwrap_or_default()
                    );
                    return None;
                }
            },
            Err(e) => {
                error!("Unable to load audio item: {:?}", e);
                return None;
            }
        };

        info!(
            "Loading <{}> with Spotify URI <{}>",
            audio_item.name, audio_item.uri
        );

        let formats = [
            AudioFileFormat::OGG_VORBIS_320,
            AudioFileFormat::MP3_320,
            AudioFileFormat::MP3_256,
            AudioFileFormat::OGG_VORBIS_160,
            AudioFileFormat::MP3_160,
            AudioFileFormat::OGG_VORBIS_96,
            AudioFileFormat::MP3_96,
        ];

        let (format, file_id) =
            match formats
                .iter()
                .find_map(|format| match audio_item.files.get(format) {
                    Some(&file_id) => Some((*format, file_id)),
                    _ => None,
                }) {
                Some(t) => t,
                None => {
                    warn!(
                        "<{}> is not available in any supported format",
                        audio_item.name
                    );
                    return None;
                }
            };

        let bytes_per_second = self.stream_data_rate(format)?;

        let encrypted_file = AudioFile::open(&self.session, file_id, bytes_per_second);

        let mut encrypted_file = match encrypted_file.await {
            Ok(encrypted_file) => encrypted_file,
            Err(e) => {
                error!("Unable to load encrypted file: {:?}", e);
                return None;
            }
        };

        //let is_cached = encrypted_file.is_cached();

        // Not all audio files are encrypted. If we can't get a key, try loading the track
        // without decryption. If the file was encrypted after all, the decoder will fail
        // parsing and bail out, so we should be safe from outputting ear-piercing noise.
        let key = match self.session.audio_key().request(spotify_id, file_id).await {
            Ok(key) => Some(key),
            Err(e) => {
                warn!("Unable to load key, continuing without decryption: {}", e);
                None
            }
        };
        //let mut decrypted_file = AudioDecrypt::new(key, encrypted_file);

        //let is_ogg_vorbis = AudioFiles::is_ogg_vorbis(format);
            
        let mut buffer = Vec::new();
        encrypted_file
            .read_to_end(&mut buffer)
            .expect("Cannot read file stream");
    

        let mut decrypted_buffer = Vec::new();
        AudioDecrypt::new(key, &buffer[..])
            .read_to_end(&mut decrypted_buffer)
            .expect("Failed to decrypt file");

        // Spotify inserts a custom Ogg packet at the start with custom metadata values, that you would
        // otherwise expect in Vorbis comments. This packet isn't well-formed and players may balk at it.
        let decrypted_buffer = &decrypted_buffer[0xa7..];

        info!("<{}> loaded", audio_item.name);

        return Some(LoadedTrackData {
            audio_item,
            audio_buffer: decrypted_buffer.to_vec(),
        });
    }
}