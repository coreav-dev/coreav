use super::{Command, DecodeError, Decoded, Pass, Plan, Refusal, Stop, audio_failure};
use crate::output::{self, WriteFailure};
use coreav_audio::{Error, OutputFormat, Sink};
use coreav_isobmff::{FourCc, Limits};
use coreav_isobmff_write::mux::{
    Edit, FileType, MovieHeader, Muxer, RawBox, Sample, TrackHeader, TrackMedia, audio_sample_entry,
};
use std::io::Write;
use std::path::Path;

pub(super) fn write(
    command: &Command,
    plan: &Plan,
    before: &Pass,
    content: Content,
    force: bool,
) -> Result<serde_json::Value, Stop> {
    let declared = before.converted.as_ref().map(|s| s.output_frames).or({
        if let Decoded::Encoded(summary) = &before.decoded {
            Some(summary.samples)
        } else {
            None
        }
    });
    let frames = frame_count(content.bytes, declared).ok_or_else(|| {
        Refusal::not_native(
            "PCM in MP4 requires a qualified nonempty frame count and a 32-bit mdat",
        )
    })?;
    let bytes = u64::from(frames) * 2;
    let mut muxer = Muxer::new(
        FileType {
            major: FourCc(*b"isom"),
            minor: 512,
            compatible: vec![FourCc(*b"isom"), FourCc(*b"iso2"), FourCc(*b"mp41")],
        },
        MovieHeader {
            timescale: 48_000,
            creation_time: 0,
            boxes: vec![
                encoder_metadata(),
                RawBox {
                    kind: FourCc(*b"free"),
                    payload: Vec::new(),
                },
            ],
        },
        &Limits::default(),
    )
    .map_err(WriteFailure::from)?;
    muxer
        .contiguous_audio_chunks()
        .map_err(WriteFailure::from)?;
    let entry = audio_sample_entry(
        FourCc(*b"ipcm"),
        1,
        16,
        48_000,
        &[
            RawBox {
                kind: FourCc(*b"pcmC"),
                payload: vec![0, 0, 0, 0, 1, 16],
            },
            RawBox {
                kind: FourCc(*b"btrt"),
                payload: [0u32, 768_000, 768_000].map(u32::to_be_bytes).concat(),
            },
        ],
    )
    .map_err(WriteFailure::from)?;
    let track = muxer
        .add_track(TrackHeader {
            media: TrackMedia::Audio { volume: 0x0100 },
            timescale: 48_000,
            language: *b"und",
            creation_time: 0,
            media_creation_time: 0,
            flags: 3,
            layer: 0,
            alternate_group: 1,
            edits: vec![Edit {
                duration: u64::from(frames),
                media_time: 0,
                rate: Edit::UNIT_RATE,
            }],
            duration: None,
            media_duration: None,
            sample_entry: entry,
            sample_table_boxes: Vec::new(),
            boxes: Vec::new(),
        })
        .map_err(WriteFailure::from)?;
    for _ in 0..frames {
        muxer
            .push_sample(
                track,
                Sample {
                    size: 2,
                    duration: 1,
                    sync: true,
                    composition_offset: 0,
                },
            )
            .map_err(WriteFailure::from)?;
    }
    let mut header = Vec::new();
    let head = muxer.write_head(&mut header).map_err(WriteFailure::from)?;
    if head.data_bytes != bytes || head.data_start != header.len() as u64 {
        return Err(Stop::Failed(audio_failure(Error::Invalid(
            "PCM in MP4 table extent",
        ))));
    }
    output::to_file(Path::new(&command.output), force, |out| {
        out.write_all(&header)
            .map_err(|error| audio_failure(Error::Output(error)))?;
        let mut payload = Tally::new(out, Some(bytes));
        let after = super::decode(command, plan, &mut payload).map_err(DecodeError::terminal)?;
        if before != &after
            || payload.content() != content
            || payload.content().bytes != head.data_bytes
        {
            return Err(audio_failure(Error::Invalid(
                "PCM in MP4 changed between preflight and execution",
            )));
        }
        Ok(serde_json::Value::Null)
    })
    .map_err(Stop::Failed)
}

fn frame_count(bytes: u64, declared: Option<u64>) -> Option<u32> {
    u32::try_from(bytes / 2).ok().filter(|frames| {
        *frames != 0
            && bytes % 2 == 0
            && bytes <= u64::from(u32::MAX) - 8
            && declared.is_none_or(|count| count == u64::from(*frames))
    })
}

fn encoder_metadata() -> RawBox {
    fn boxed(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        [
            (payload.len() as u32 + 8).to_be_bytes().as_slice(),
            kind,
            payload,
        ]
        .concat()
    }
    let data = boxed(
        b"data",
        &[
            &[0, 0, 0, 1, 0, 0, 0, 0],
            super::super::MUXER_ENCODER.as_bytes(),
        ]
        .concat(),
    );
    let item = boxed(b"\xa9too", &data);
    let items = boxed(b"ilst", &item);
    let handler = boxed(b"hdlr", b"\0\0\0\0\0\0\0\0mdirappl\0\0\0\0\0\0\0\0\0");
    RawBox {
        kind: FourCc(*b"udta"),
        payload: boxed(
            b"meta",
            &[&[0; 4], handler.as_slice(), items.as_slice()].concat(),
        ),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Content {
    bytes: u64,
    fingerprint: u64,
}

pub(super) struct Tally<'a> {
    out: &'a mut dyn Write,
    maximum: Option<u64>,
    content: Content,
}

impl<'a> Tally<'a> {
    pub(super) fn new(out: &'a mut dyn Write, maximum: Option<u64>) -> Self {
        Self {
            out,
            maximum,
            content: Content {
                bytes: 0,
                fingerprint: 0xcbf29ce484222325,
            },
        }
    }

    pub(super) fn content(&self) -> Content {
        self.content
    }
}

impl Sink for Tally<'_> {
    fn start(&mut self, _: &OutputFormat) -> Result<(), Error> {
        Ok(())
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let count = self
            .content
            .bytes
            .checked_add(bytes.len() as u64)
            .filter(|count| self.maximum.is_none_or(|maximum| *count <= maximum))
            .ok_or(Error::Invalid("PCM in MP4 exceeds its declared extent"))?;
        self.out.write_all(bytes).map_err(Error::Output)?;
        for byte in bytes {
            self.content.fingerprint =
                (self.content.fingerprint ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
        }
        self.content.bytes = count;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_counts_refuse_empty_partial_unqualified_and_large_extents() {
        assert_eq!(frame_count(105_600, Some(52_800)), Some(52_800));
        assert_eq!(frame_count(105_600, None), Some(52_800));
        for (bytes, declared) in [
            (0, None),
            (3, None),
            (4, Some(3)),
            (u64::from(u32::MAX) - 7, None),
            (u64::MAX, None),
        ] {
            assert_eq!(frame_count(bytes, declared), None);
        }
        let maximum = u64::from(u32::MAX) - 9;
        assert_eq!(frame_count(maximum, None), u32::try_from(maximum / 2).ok());
    }

    #[test]
    fn changed_content_truncation_and_late_io_are_terminal_and_preserve_the_destination() {
        use coreav_isobmff::{FileSource, Source};
        let dir = std::env::temp_dir().join(format!("pcm-mp4-late-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("source.wav");
        let output = dir.join("output.mp4");
        let mut original =
            coreav_audio::wav::header(coreav_audio::SampleFormat::S16, 1, 48_000, 4, Some(6144));
        original.extend_from_slice(&[0x31; 6144]);
        for change in [0, 1, 2] {
            std::fs::write(&input, &original).unwrap();
            std::fs::write(&output, b"destination").unwrap();
            let arguments: Vec<String> = [
                "-y",
                "-i",
                input.to_str().unwrap(),
                "-ac",
                "1",
                "-ar",
                "48000",
                "-c:a",
                "pcm_s16le",
                "-vn",
                output.to_str().unwrap(),
            ]
            .into_iter()
            .map(str::to_owned)
            .collect();
            let command = crate::ffmpeg::args::parse(&arguments).unwrap();
            let mut source = FileSource::open(&input).unwrap();
            let length = source.len().unwrap();
            let probed = crate::ffprobe::probe(&mut source, length, input.to_str().unwrap(), None)
                .unwrap_or_else(|_| panic!("probe failed"));
            let plan =
                crate::ffmpeg::raw_audio::plan(&command, &probed.report, &[0], false).unwrap();
            let mut sink = std::io::sink();
            let mut tally = Tally::new(&mut sink, None);
            let before = match crate::ffmpeg::raw_audio::decode(&command, &plan, &mut tally) {
                Ok(before) => before,
                Err(_) => panic!("preflight failed"),
            };
            match change {
                0 => {
                    let mut changed = original.clone();
                    *changed.last_mut().unwrap() ^= 1;
                    std::fs::write(&input, changed).unwrap();
                }
                1 => {
                    std::fs::write(&input, &original[..original.len() - 2]).unwrap();
                }
                _ => {
                    std::fs::remove_file(&input).unwrap();
                }
            }
            assert!(matches!(
                write(&command, &plan, &before, tally.content(), true),
                Err(Stop::Failed(_))
            ));
            assert_eq!(std::fs::read(&output).unwrap(), b"destination");
            assert_eq!(
                std::fs::read_dir(&dir).unwrap().count(),
                if change == 2 { 1 } else { 2 }
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn content_detects_short_long_and_same_length_changes_independently_of_chunks() {
        fn content(chunks: &[&[u8]], maximum: Option<u64>) -> Content {
            let mut out = Vec::new();
            let mut tally = Tally::new(&mut out, maximum);
            for chunk in chunks {
                tally.write(chunk).unwrap();
            }
            tally.content()
        }
        let before = content(&[b"abcdef"], None);
        assert_eq!(before, content(&[b"ab", b"c", b"def"], Some(6)));
        assert_ne!(before, content(&[b"abcde"], Some(6)));
        assert_ne!(before, content(&[b"abcdeg"], Some(6)));
        let mut out = Vec::new();
        let mut tally = Tally::new(&mut out, Some(6));
        tally.write(b"abcde").unwrap();
        assert!(matches!(tally.write(b"fg"), Err(Error::Invalid(_))));
        assert_eq!(tally.content().bytes, 5);
        assert_eq!(out, b"abcde");
        let mut out = std::io::sink();
        let mut tally = Tally::new(&mut out, None);
        tally.content.bytes = u64::MAX;
        assert!(matches!(tally.write(b"x"), Err(Error::Invalid(_))));
    }
}
