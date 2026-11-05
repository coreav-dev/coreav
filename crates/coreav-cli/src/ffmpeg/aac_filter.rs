use crate::error::Failure;
use crate::output::WriteFailure;
use coreav_isobmff::{Allowlist, Budget, Codec, FileSource, Limits, Source, TrackKind, probe};

pub(super) fn validate(file: &std::fs::File) -> Result<(), WriteFailure> {
    let source = FileSource::from_file(
        file.try_clone()
            .map_err(|e| WriteFailure::Rejected(Failure::io(e)))?,
    )?;
    identity(source, &Limits::default())
}

fn identity(mut source: impl Source, limits: &Limits) -> Result<(), WriteFailure> {
    let movie = probe(&mut source, limits, &Allowlist::listing())?;
    if movie
        .dropped
        .iter()
        .any(|track| track.kind == TrackKind::Audio)
    {
        return unsupported();
    }
    let mut budget = Budget::new(limits);
    for track in movie
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Audio)
    {
        if track.description.codec != Codec::Aac
            || track.descriptions != 1
            || track.description.aac_frame_length.is_none()
            || track.description.configured_sample_rate.is_none()
        {
            return unsupported();
        }
        let mut packets = movie.packets(track.index, &mut source, limits, &mut budget)?;
        while let Some(packet) = packets.next() {
            let packet = packet?;
            if packet.size < 2 {
                return unsupported();
            }
            let mut prefix = [0u8; 2];
            packets
                .source_mut()
                .read_exact_at(packet.offset, &mut prefix)?;
            if matches!(prefix, [0xff, byte] if byte & 0xf0 == 0xf0) {
                return unsupported();
            }
        }
    }
    Ok(())
}

fn unsupported<T>() -> Result<T, WriteFailure> {
    Err(WriteFailure::Rejected(Failure::Unsupported(
        "AAC ADTS-to-ASC filtering that is not an identity on the copied audio".to_owned(),
    )))
}

#[cfg(test)]
#[path = "../../../coreav-isobmff/tests/common/text_movie.rs"]
mod movies;

#[cfg(test)]
mod tests {
    use super::*;
    use coreav_isobmff::SliceSource;

    fn audio(samples: Vec<Vec<u8>>, kind: &[u8; 4], configured: bool) -> Vec<u8> {
        let mut entry = vec![0; 28];
        entry[7] = 1;
        entry[16..18].copy_from_slice(&2u16.to_be_bytes());
        entry[18..20].copy_from_slice(&16u16.to_be_bytes());
        entry[24..28].copy_from_slice(&(48_000u32 << 16).to_be_bytes());
        if configured {
            entry.extend(movies::full(
                b"esds",
                0,
                0,
                &[
                    3, 25, 0, 0, 0, 4, 17, 0x40, 0x15, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5, 2, 0x11,
                    0x90, 6, 1, 2,
                ],
            ));
        }
        let track = movies::TrackSpec {
            handler: *b"soun",
            entry: movies::bx(kind, &entry),
            timescale: 48_000,
            samples: samples.into_iter().map(|bytes| (bytes, 1024)).collect(),
            ..movies::TrackSpec::qt_text(1, Vec::new())
        };
        movies::movie(&[track])
    }

    #[test]
    fn filter_identity_checks_every_packet_and_the_stored_configuration() {
        let plain = audio(vec![vec![0x21, 0x10], vec![0xff, 0xef]], b"mp4a", true);
        assert!(identity(SliceSource::new(&plain), &Limits::default()).is_ok());
        for source in [
            audio(vec![vec![0x21, 0x10], vec![0xff, 0xf1]], b"mp4a", true),
            audio(vec![vec![0xff, 0xf0]], b"mp4a", true),
            audio(vec![vec![0x21]], b"mp4a", true),
            audio(vec![Vec::new()], b"mp4a", true),
            audio(vec![vec![0x21, 0x10]], b"mp4a", false),
            audio(vec![vec![0x21, 0x10]], b".mp3", true),
        ] {
            assert!(matches!(
                identity(SliceSource::new(&source), &Limits::default()),
                Err(WriteFailure::Rejected(Failure::Unsupported(_)))
            ));
        }
        let video = movies::movie(&[movies::TrackSpec::video(1, &[40])]);
        assert!(identity(SliceSource::new(&video), &Limits::default()).is_ok());
    }

    #[test]
    fn identity_validation_retains_the_packet_work_limit() {
        let bytes = audio(vec![vec![0x21, 0x10]; 3], b"mp4a", true);
        let limits = Limits {
            max_sample_work: 1,
            ..Limits::default()
        };
        assert!(matches!(
            identity(SliceSource::new(&bytes), &limits),
            Err(WriteFailure::Rejected(Failure::Library(
                coreav_isobmff::Error::LimitExceeded { .. }
            )))
        ));
    }
}
