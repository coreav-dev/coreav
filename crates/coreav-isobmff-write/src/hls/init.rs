//! Init segments: `ftyp` and a `moov` box with one track, empty sample
//! tables and a movie extends box (ISO/IEC 14496-12 8.8.1).

use super::Rendition;
use super::index::HlsIndex;
use super::segment::TRACK_ID;
use crate::ExtractError;
use crate::boxes::Node;
use crate::moov::{EditEntry, Media, MovieBox, TrackBox, TrackFlags, ftyp, mvhd, trak};
use crate::tables::Tables;
use coreav_isobmff::Error;
use std::borrow::Cow;

impl HlsIndex {
    /// The init segment of `rendition`: `ftyp`, then `moov` with the
    /// source's sample entry, the rendition's edit list if it has one, and
    /// `mvex`. Built from the index alone.
    pub fn init_segment(&self, rendition: Rendition) -> Result<Vec<u8>, ExtractError> {
        let track = self.track(rendition).ok_or(Error::NoSuchTrack)?;
        let codec_brand: Option<&[u8; 4]> = match track.codecs.get(..4) {
            Some("avc1") => Some(b"avc1"),
            Some("av01") => Some(b"av01"),
            _ => None,
        };
        let mut brands: Vec<&[u8; 4]> = vec![b"iso6", b"cmfc"];
        brands.extend(codec_brand);
        let mut edits = Vec::with_capacity(2);
        if let Some(edit) = track.edit {
            if edit.empty > 0 {
                edits.push(EditEntry {
                    duration: edit.empty,
                    media_time: -1,
                    rate: EditEntry::UNIT_RATE,
                });
            }
            edits.push(EditEntry {
                duration: edit.duration,
                media_time: edit.media_time,
                rate: EditEntry::UNIT_RATE,
            });
        }
        let media = match track.rendition {
            Rendition::Video => Media::Video {
                matrix: track.matrix,
                size: track.presentation_size,
            },
            Rendition::Audio => Media::Audio,
        };
        // Durations are 0: the samples are in the media segments, and the
        // playlists give their durations (14496-12 8.8.1).
        let empty = Tables::new(track.timescale, 0);
        let track_box = TrackBox {
            handler_name: None,
            track_id: TRACK_ID,
            media,
            header: TrackFlags::DEFAULT,
            created: track.created,
            modified: None,
            media_created: track.media_created,
            media_modified: None,
            language: track.language,
            media_timescale: track.timescale,
            media_duration: 0,
            duration: 0,
            edits: Cow::Owned(edits),
            stsd: track.stsd.clone(),
            tables: &empty,
            extra: Vec::new(),
            stbl_extra: Vec::new(),
        };
        let movie = MovieBox {
            timescale: track.timescale,
            created: track.created,
            modified: None,
            duration: 0,
            extra: Vec::new(),
        };
        let mut trex = TRACK_ID.to_be_bytes().to_vec();
        // default_sample_description_index 1; duration, size and flags 0.
        trex.extend_from_slice(&1u32.to_be_bytes());
        trex.extend_from_slice(&[0; 12]);
        let mvex = Node::parent(b"mvex", vec![Node::full(b"trex", 0, 0, &trex)]);
        let moov = Node::parent(
            b"moov",
            vec![
                mvhd(&movie, TRACK_ID.saturating_add(1)),
                trak(&track_box, empty_tables()),
                mvex,
            ],
        );
        let mut out = Vec::new();
        ftyp(b"iso6", &brands).write(&mut out)?;
        moov.write(&mut out)?;
        Ok(out)
    }
}

/// `stts`, `stsc`, `stsz` and `stco` without entries.
fn empty_tables() -> Vec<Node<'static>> {
    vec![
        Node::full(b"stts", 0, 0, &[0; 4]),
        Node::full(b"stsc", 0, 0, &[0; 4]),
        Node::full(b"stsz", 0, 0, &[0; 8]),
        Node::full(b"stco", 0, 0, &[0; 4]),
    ]
}
