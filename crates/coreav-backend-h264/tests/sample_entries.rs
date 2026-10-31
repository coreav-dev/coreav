//! An MP4 track whose samples change sample description (ISO/IEC 14496-12
//! 8.7.4) at IDR pictures, each description with its own parameter sets,
//! through the thumbnail video reader and the native backend: every frame
//! agrees with the pinned FFmpeg's `framemd5` of the file under V1's flags
//! (`docs/design/compat-cli.md`), and the changes CoreAV does not decode are
//! refused (`docs/design/decode.md`, "Several sample descriptions").
//!
//! `tests/data/two-sample-entries.mp4` and its reference are made by
//! `scripts/stsd_craft.py --fixture`: a 128x96 part, an 80x48 part and the
//! first part again.
// Test-only sources outside `#[test]` functions.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use coreav_backend_h264::NativeH264;
use coreav_decode::{Backend, Decoder, Packet, Policy};
use coreav_isobmff::{Rational, SliceSource};
use coreav_thumbnail::{Error, ThumbnailOptions, first_packets, video_stream};
use md5::{Digest, Md5};

const FIXTURE: &[u8] = include_bytes!("data/two-sample-entries.mp4");
const REFERENCE: &str = include_str!("data/two-sample-entries.framemd5");

fn options() -> ThumbnailOptions {
    ThumbnailOptions::at(Rational::from_int(0))
}

/// The size and MD5 of each frame of the reference, in presentation order.
fn reference() -> Vec<(usize, String)> {
    REFERENCE
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .map(|line| {
            let fields: Vec<&str> = line.split(',').map(str::trim).collect();
            (fields[4].parse().expect("size"), fields[5].to_owned())
        })
        .collect()
}

/// Every frame of `file`'s video stream as the native backend decodes it
/// from the packets the thumbnail reader gives: size and MD5 of the planes
/// packed without padding, in presentation order.
fn decode(file: &[u8]) -> Vec<(usize, String)> {
    let (_, stream, packets) =
        first_packets(SliceSource::new(file), &options(), usize::MAX).expect("packets");
    let backend = NativeH264::new();
    let mut decoder = coreav_decode::open(
        &stream,
        &Policy::default(),
        &coreav_decode::Limits::default(),
        &[&backend as &dyn Backend],
    )
    .expect("opens")
    .decoder;
    let mut frames = Vec::new();
    let mut drain = |decoder: &mut Box<dyn Decoder>| {
        while let Some(frame) = decoder.receive_frame().expect("frame") {
            let mut packed = Vec::new();
            frame
                .to_cpu()
                .expect("picture")
                .write_packed(&mut packed)
                .expect("packed");
            let md5: String = Md5::digest(&packed)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            frames.push((frame.pts, packed.len(), md5));
        }
    };
    for p in &packets {
        decoder
            .send_packet(&Packet {
                data: &p.data,
                pts: Some(p.pts),
                dts: p.dts,
                keyframe: p.keyframe,
                discard: false,
            })
            .expect("decodes");
        drain(&mut decoder);
    }
    decoder.send_eof().expect("flushes");
    drain(&mut decoder);
    frames.sort_by_key(|f| f.0);
    frames
        .into_iter()
        .map(|(_, size, md5)| (size, md5))
        .collect()
}

#[test]
fn every_frame_follows_its_sample_description() {
    let frames = decode(FIXTURE);
    assert_eq!(frames.len(), 30);
    assert_eq!(frames, reference());
}

#[test]
fn a_whole_stream_reads_as_the_oracle_reads_it() {
    let (_, _, packets) =
        first_packets(SliceSource::new(FIXTURE), &options(), usize::MAX).expect("packets");
    let mut source = SliceSource::new(FIXTURE);
    let stream = video_stream(&mut source, &options()).expect("stream");
    assert_eq!(stream.packets.len(), packets.len());
    for (index, packet) in packets.iter().enumerate() {
        let data = stream
            .read_packet(&mut source, index, u64::MAX)
            .expect("read");
        assert_eq!(data, packet.data, "packet {index}");
    }
}

/// The offset of the box of type `kind` that is the only one in the file.
fn find(file: &[u8], kind: &[u8; 4]) -> usize {
    let at: Vec<usize> = file
        .windows(4)
        .enumerate()
        .filter(|(_, w)| w == kind)
        .map(|(i, _)| i - 4)
        .collect();
    assert_eq!(at.len(), 1, "one {}", String::from_utf8_lossy(kind));
    at[0]
}

fn be32(file: &[u8], at: usize) -> usize {
    u32::from_be_bytes(file[at..at + 4].try_into().unwrap()) as usize
}

/// The offset of the second sample entry of the sample description box.
fn second_entry(file: &[u8]) -> usize {
    let first = find(file, b"stsd") + 16;
    first + be32(file, first)
}

fn refusal(file: &[u8]) -> String {
    let refused = |result: Result<(), Error>| match result {
        Err(Error::Unsupported(why)) => why,
        other => panic!("not refused: {other:?}"),
    };
    let oracle = refused(first_packets(SliceSource::new(file), &options(), usize::MAX).map(|_| ()));
    let stream = refused(video_stream(SliceSource::new(file), &options()).map(|_| ()));
    assert_eq!(oracle, stream);
    oracle
}

#[test]
fn a_change_at_a_picture_that_is_not_idr_is_refused() {
    let mut file = FIXTURE.to_vec();
    // The second chunk's first sample: its IDR slices become non-IDR ones.
    let stco = find(&file, b"stco");
    let stsz = find(&file, b"stsz");
    let (mut at, size) = (be32(&file, stco + 20), be32(&file, stsz + 20 + 4 * 10));
    let end = at + size;
    while at < end {
        let len = be32(&file, at);
        if file[at + 4] & 0x1f == 5 {
            file[at + 4] = (file[at + 4] & 0xe0) | 1;
        }
        at += 4 + len;
    }
    let why = refusal(&file);
    assert!(
        why.contains("packet 10, which holds no IDR picture"),
        "{why}"
    );
}

#[test]
fn another_codec_in_a_later_description_is_refused() {
    let mut file = FIXTURE.to_vec();
    let entry = second_entry(&file);
    file[entry + 4..entry + 8].copy_from_slice(b"hvc1");
    let why = refusal(&file);
    assert!(
        why.contains("changes from avc1 to hvc1 at packet 10"),
        "{why}"
    );
}

#[test]
fn another_nal_unit_length_is_refused() {
    let mut file = FIXTURE.to_vec();
    let entry = second_entry(&file);
    let avcc = entry + find(&file[entry..], b"avcC");
    // lengthSizeMinusOne (ISO/IEC 14496-15 5.3.3.1): 4 bytes become 2.
    file[avcc + 12] = (file[avcc + 12] & !3) | 1;
    let why = refusal(&file);
    assert!(
        why.contains("packet 10 to a configuration record with another NAL unit length"),
        "{why}"
    );
}
