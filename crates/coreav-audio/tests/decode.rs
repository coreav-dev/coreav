//! The pipeline on Matroska files built here: the output limit against a
//! flood of constant subframes (RFC 9639 section 11's amplification),
//! concealment of a damaged frame, a truncated file, refusals, and the
//! container/codec cross-check.

#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

use coreav_audio::{
    Codec, Error, LimitKind, Limits, Options, OutputFormat, SampleFormat, Sink, Warning, decode,
};
use coreav_isobmff::SliceSource;

/// A sink that keeps everything.
#[derive(Default)]
struct Keep {
    format: Option<OutputFormat>,
    bytes: Vec<u8>,
}

impl Sink for Keep {
    fn start(&mut self, format: &OutputFormat) -> Result<(), Error> {
        self.format = Some(*format);
        Ok(())
    }
    fn write(&mut self, samples: &[u8]) -> Result<(), Error> {
        self.bytes.extend_from_slice(samples);
        Ok(())
    }
}

fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &b in data {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ 0x07
            } else {
                crc << 1
            };
        }
    }
    crc
}

fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &b in data {
        crc ^= u16::from(b) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x8005
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// A big-endian bit writer.
#[derive(Default)]
struct BitWriter {
    bytes: Vec<u8>,
    bits: u32,
}

impl BitWriter {
    fn put(&mut self, value: u64, n: u32) {
        for i in (0..n).rev() {
            if self.bits % 8 == 0 {
                self.bytes.push(0);
            }
            let bit = ((value >> i) & 1) as u8;
            *self.bytes.last_mut().unwrap() |= bit << (7 - self.bits % 8);
            self.bits += 1;
        }
    }
}

/// The 34-byte streaminfo body.
fn streaminfo(rate: u32, channels: u32, bits: u32) -> Vec<u8> {
    let mut w = BitWriter::default();
    w.put(16, 16);
    w.put(65535, 16);
    w.put(0, 24);
    w.put(0, 24);
    w.put(u64::from(rate), 20);
    w.put(u64::from(channels - 1), 3);
    w.put(u64::from(bits - 1), 5);
    w.put(0, 36);
    w.put(0, 64);
    w.put(0, 64);
    w.bytes
}

/// A frame of constant subframes: 44.1 kHz, `channels` independent
/// channels of `bits` bits (16 or 32), `block` samples, frame number
/// `number` (below 128).
fn constant_frame(number: u8, block: u16, channels: u8, bits: u32, value: i64) -> Vec<u8> {
    let depth_code = if bits == 32 { 7 } else { 4 };
    let mut frame = vec![
        0xFF,
        0xF8,
        0x79,
        ((channels - 1) << 4) | (depth_code << 1),
        number,
    ];
    frame.extend_from_slice(&(block - 1).to_be_bytes());
    frame.push(crc8(&frame));
    let mut w = BitWriter {
        bytes: frame,
        bits: 0,
    };
    w.bits = w.bytes.len() as u32 * 8;
    for _ in 0..channels {
        w.put(0, 8);
        w.put(value as u64 & ((1u64 << bits) - 1), bits);
    }
    let mut frame = w.bytes;
    let crc = crc16(&frame);
    frame.extend_from_slice(&crc.to_be_bytes());
    frame
}

/// An EBML element with an 8-byte size.
fn element(id: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut out = id.to_vec();
    out.push(0x01);
    out.extend_from_slice(&(payload.len() as u64).to_be_bytes()[1..]);
    out.extend_from_slice(payload);
    out
}

fn uint(id: &[u8], value: u64) -> Vec<u8> {
    element(id, &value.to_be_bytes())
}

/// A Matroska file with one audio track and one SimpleBlock per frame.
fn matroska(
    codec_id: &str,
    private: &[u8],
    rate: f64,
    channels: u64,
    frames: &[Vec<u8>],
) -> Vec<u8> {
    let mut header = Vec::new();
    header.extend(uint(&[0x42, 0x86], 1));
    header.extend(uint(&[0x42, 0xF7], 1));
    header.extend(uint(&[0x42, 0xF2], 4));
    header.extend(uint(&[0x42, 0xF3], 8));
    header.extend(element(&[0x42, 0x82], b"matroska"));
    header.extend(uint(&[0x42, 0x87], 4));
    header.extend(uint(&[0x42, 0x85], 2));
    let mut file = element(&[0x1A, 0x45, 0xDF, 0xA3], &header);
    let info = uint(&[0x2A, 0xD7, 0xB1], 1_000_000);
    let mut audio = element(&[0xB5], &rate.to_be_bytes());
    audio.extend(uint(&[0x9F], channels));
    let mut entry = uint(&[0xD7], 1);
    entry.extend(uint(&[0x73, 0xC5], 1));
    entry.extend(uint(&[0x83], 2));
    entry.extend(element(&[0x86], codec_id.as_bytes()));
    entry.extend(element(&[0x63, 0xA2], private));
    entry.extend(element(&[0xE1], &audio));
    let tracks = element(&[0x16, 0x54, 0xAE, 0x6B], &element(&[0xAE], &entry));
    let mut cluster = uint(&[0xE7], 0);
    for frame in frames {
        let mut block = vec![0x81, 0x00, 0x00, 0x80];
        block.extend_from_slice(frame);
        cluster.extend(element(&[0xA3], &block));
    }
    let mut segment = element(&[0x15, 0x49, 0xA9, 0x66], &info);
    segment.extend(tracks);
    segment.extend(element(&[0x1F, 0x43, 0xB6, 0x75], &cluster));
    file.extend(element(&[0x18, 0x53, 0x80, 0x67], &segment));
    file
}

fn flac_private(rate: u32, channels: u32, bits: u32) -> Vec<u8> {
    let mut private = b"fLaC".to_vec();
    private.extend_from_slice(&[0x80, 0x00, 0x00, 0x22]);
    private.extend(streaminfo(rate, channels, bits));
    private
}

fn options(format: SampleFormat) -> Options {
    Options {
        format,
        ..Options::default()
    }
}

#[test]
fn decodes_a_matroska_flac_track() {
    let frames: Vec<_> = (0..3)
        .map(|n| constant_frame(n, 4096, 2, 16, -1234))
        .collect();
    let file = matroska("A_FLAC", &flac_private(44_100, 2, 16), 44_100.0, 2, &frames);
    let mut sink = Keep::default();
    let summary = decode(
        SliceSource::new(&file),
        &options(SampleFormat::S16),
        &Limits::default(),
        &mut sink,
    )
    .unwrap();
    assert_eq!(summary.stream.codec, Codec::Flac);
    assert_eq!(summary.samples, 3 * 4096);
    assert_eq!(summary.packets, 3);
    assert!(summary.warnings.is_empty(), "{:?}", summary.warnings);
    assert_eq!(sink.format.unwrap().channels, 2);
    assert_eq!(sink.bytes.len(), 3 * 4096 * 2 * 2);
    assert!(
        sink.bytes
            .chunks_exact(2)
            .all(|s| i16::from_le_bytes([s[0], s[1]]) == -1234)
    );
}

/// RFC 9639 section 11: a 50-byte frame codes 2 MiB of PCM. The output
/// limit stops a flood of them before the sink gets more than the limit.
#[test]
fn a_flood_of_constant_subframes_stops_at_the_output_limit() {
    let frames: Vec<_> = (0..100)
        .map(|n| constant_frame(n, 65535, 8, 32, 7))
        .collect();
    assert!(frames[0].len() < 64);
    let file = matroska("A_FLAC", &flac_private(44_100, 8, 32), 44_100.0, 8, &frames);
    let limits = Limits {
        max_output_samples: 10_000_000,
        ..Limits::default()
    };
    let mut sink = Keep::default();
    let result = decode(
        SliceSource::new(&file),
        &options(SampleFormat::S32),
        &limits,
        &mut sink,
    );
    assert!(
        matches!(result, Err(Error::LimitExceeded(LimitKind::OutputSamples))),
        "{result:?}"
    );
    assert!(sink.bytes.len() / 4 <= 10_000_000);
}

#[test]
fn a_damaged_frame_is_replaced_by_silence_of_its_length() {
    let mut frames: Vec<_> = (0..5)
        .map(|n| constant_frame(n, 1024, 1, 16, 500))
        .collect();
    // Break frame 2's header CRC-8.
    frames[2][7] ^= 0xFF;
    let file = matroska("A_FLAC", &flac_private(44_100, 1, 16), 44_100.0, 1, &frames);
    let mut sink = Keep::default();
    let summary = decode(
        SliceSource::new(&file),
        &options(SampleFormat::S16),
        &Limits::default(),
        &mut sink,
    )
    .unwrap();
    assert_eq!(summary.samples, 5 * 1024);
    let samples: Vec<i16> = sink
        .bytes
        .chunks_exact(2)
        .map(|s| i16::from_le_bytes([s[0], s[1]]))
        .collect();
    assert!(samples[..2048].iter().all(|&s| s == 500));
    assert!(samples[2048..3072].iter().all(|&s| s == 0));
    assert!(samples[3072..].iter().all(|&s| s == 500));
    assert!(
        matches!(
            &summary.warnings[..],
            [Warning::Concealed { packets: 1, .. }]
        ),
        "{:?}",
        summary.warnings
    );
}

#[test]
fn too_many_damaged_frames_fail() {
    let frames: Vec<_> = (0..5).map(|_| vec![0u8; 20]).collect();
    let file = matroska("A_FLAC", &flac_private(44_100, 1, 16), 44_100.0, 1, &frames);
    let limits = Limits {
        max_decode_errors: 3,
        ..Limits::default()
    };
    let result = decode(
        SliceSource::new(&file),
        &options(SampleFormat::S16),
        &limits,
        &mut Keep::default(),
    );
    assert!(
        matches!(result, Err(Error::LimitExceeded(LimitKind::DecodeErrors))),
        "{result:?}"
    );
}

#[test]
fn a_truncated_file_keeps_what_was_decoded() {
    let frames: Vec<_> = (0..4).map(|n| constant_frame(n, 1024, 1, 16, 9)).collect();
    let mut file = matroska("A_FLAC", &flac_private(44_100, 1, 16), 44_100.0, 1, &frames);
    // Cut into the last block.
    file.truncate(file.len() - 5);
    let mut sink = Keep::default();
    let summary = decode(
        SliceSource::new(&file),
        &options(SampleFormat::S16),
        &Limits::default(),
        &mut sink,
    )
    .unwrap();
    assert_eq!(summary.samples, 3 * 1024);
    assert!(
        summary
            .warnings
            .iter()
            .any(|w| matches!(w, Warning::DemuxStopped { .. })),
        "{:?}",
        summary.warnings
    );
}

#[test]
fn a_codec_that_is_not_decoded_is_named() {
    let file = matroska(
        "A_VORBIS",
        &[2, 1, 1, 0, 0, 0],
        44_100.0,
        2,
        &[vec![0u8; 4]],
    );
    let result = decode(
        SliceSource::new(&file),
        &options(SampleFormat::S16),
        &Limits::default(),
        &mut Keep::default(),
    );
    assert!(
        matches!(&result, Err(Error::CodecNotDecoded(name)) if name == "vorbis"),
        "{result:?}"
    );
    let not_audio = b"\x00\x00\x00\x18ftypisom\x00\x00\x00\x00isommp41";
    let result = decode(
        SliceSource::new(not_audio),
        &options(SampleFormat::S16),
        &Limits::default(),
        &mut Keep::default(),
    );
    assert!(result.is_err());
    let result = decode(
        SliceSource::new(b"RIFF\x24\x00\x00\x00WAVE"),
        &options(SampleFormat::S16),
        &Limits::default(),
        &mut Keep::default(),
    );
    assert!(matches!(result, Err(Error::UnknownContainer)), "{result:?}");
}

#[test]
fn the_codec_configuration_wins_over_the_container() {
    let frames = vec![constant_frame(0, 512, 2, 16, 3)];
    let file = matroska("A_FLAC", &flac_private(44_100, 2, 16), 48_000.0, 1, &frames);
    let mut sink = Keep::default();
    let summary = decode(
        SliceSource::new(&file),
        &options(SampleFormat::S16),
        &Limits::default(),
        &mut sink,
    )
    .unwrap();
    let format = sink.format.unwrap();
    assert_eq!((format.sample_rate, format.channels), (44_100, 2));
    assert_eq!(
        summary.warnings,
        vec![
            Warning::ContainerMismatch {
                field: "sample_rate",
                container: 48_000,
                codec: 44_100
            },
            Warning::ContainerMismatch {
                field: "channels",
                container: 1,
                codec: 2
            },
        ]
    );
}

#[test]
fn a_missing_stream_is_refused() {
    let frames = vec![constant_frame(0, 512, 1, 16, 3)];
    let file = matroska("A_FLAC", &flac_private(44_100, 1, 16), 44_100.0, 1, &frames);
    let options = Options {
        stream: Some(5),
        ..Options::default()
    };
    let result = decode(
        SliceSource::new(&file),
        &options,
        &Limits::default(),
        &mut Keep::default(),
    );
    assert!(matches!(result, Err(Error::NoSuchStream)), "{result:?}");
}

/// Parallel decoding writes the same bytes as one thread, with the same
/// concealment, trimming and limits.
#[test]
fn threads_write_the_same_bytes() {
    let mut frames: Vec<_> = (0..120)
        .map(|n| {
            constant_frame(
                n % 100,
                1000 + u16::from(n % 7) * 13,
                2,
                16,
                i64::from(n) * 37 - 2000,
            )
        })
        .collect();
    frames[50][7] ^= 0xFF;
    let file = matroska("A_FLAC", &flac_private(44_100, 2, 16), 44_100.0, 2, &frames);
    let run = |threads| {
        let mut sink = Keep::default();
        let options = Options {
            threads,
            ..options(SampleFormat::S24)
        };
        let summary = decode(
            SliceSource::new(&file),
            &options,
            &Limits::default(),
            &mut sink,
        )
        .unwrap();
        (summary, sink.bytes)
    };
    let (one, bytes) = run(1);
    for threads in [2, 3, 8, 0] {
        let (summary, other) = run(threads);
        assert_eq!(summary, one, "{threads} threads");
        assert!(other == bytes, "{threads} threads");
    }
    assert!(matches!(
        &one.warnings[..],
        [Warning::Concealed { packets: 1, .. }]
    ));
}

/// Layer III frames, joint stereo, whose granules take random Huffman data
/// from the bit reservoir: each frame's main data starts up to 511 bytes
/// (255 for MPEG-2) back, in the frames before it. MPEG-1 frames are 44.1
/// kHz, 128 kbit/s, two granules; MPEG-2 (`lsf`) frames 22.05 kHz, one
/// granule, at 32 or 160 kbit/s at random, so that a large frame's main
/// data can reach back past a small one's.
fn mp3_frames(count: usize, seed: u64, lsf: bool, mono: impl Fn(usize) -> bool) -> Vec<Vec<u8>> {
    let mut state = seed;
    let mut next = |bound: u64| {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 33) % bound
    };
    // Granules, main_data_begin bits.
    let (granules, begin_bits) = if lsf { (1, 8) } else { (2, 9) };
    let mut frames = Vec::new();
    // Main data bytes of earlier frames that no granule used.
    let mut unused = 0usize;
    for index in 0..count {
        let channels = if mono(index) { 1 } else { 2 };
        // Side information bytes (ISO/IEC 11172-3 2.4.1.7; 13818-3 2.4.1.7).
        let side = match (lsf, channels) {
            (false, 1) => 17,
            (false, _) => 32,
            (true, 1) => 9,
            (true, _) => 17,
        };
        // MPEG-2 bit rate indices 4 (32 kbit/s) and 14 (160 kbit/s).
        let (size, rate_bits): (usize, u64) = match (lsf, next(2)) {
            (false, _) => (417, 0x90),
            (true, 0) => (104, 0x40),
            (true, _) => (522, 0xE0),
        };
        let main = size - 4 - side;
        let begin = next(unused.min((1 << begin_bits) - 1) as u64 + 1) as usize;
        let available = 8 * (begin + main);
        let mut lengths = vec![0usize; channels * granules];
        let mut used = 0;
        for length in &mut lengths {
            *length = (next(900) as usize + 100).min(available - used);
            used += *length;
        }
        unused = (available - used) / 8;
        let mut w = BitWriter::default();
        // Sync, layer III, no CRC, the bit rate, 22.05 kHz (MPEG-2) or
        // 44.1 kHz (MPEG-1), joint stereo with middle/side or mono.
        let sync: u64 = if lsf { 0xFFF3 } else { 0xFFFB };
        let mode: u64 = if channels == 1 { 0xC4 } else { 0x64 };
        w.put((sync << 16) | (rate_bits << 8) | mode, 32);
        w.put(begin as u64, begin_bits);
        // Private bits, and MPEG-1's scfsi.
        w.put(
            0,
            match (lsf, channels) {
                (false, 1) => 5 + 4,
                (false, _) => 3 + 8,
                (true, 1) => 1,
                (true, _) => 2,
            },
        );
        for length in lengths {
            w.put(length as u64, 12);
            w.put(next(289), 9);
            w.put(110 + next(40), 8);
            // scalefac_compress: 4 bits, or 9 for MPEG-2.
            if lsf {
                w.put(next(512), 9);
            } else {
                w.put(next(16), 4);
            }
            w.put(0, 1);
            for _ in 0..3 {
                w.put(next(32), 5);
            }
            w.put(next(16), 4);
            w.put(next(8), 3);
            // preflag (MPEG-1 only), scalefac_scale, count1table_select.
            if !lsf {
                w.put(next(2), 1);
            }
            w.put(next(2), 1);
            w.put(next(2), 1);
        }
        assert_eq!(w.bytes.len(), 4 + side);
        let mut frame = w.bytes;
        frame.extend((0..main).map(|_| next(256) as u8));
        assert_eq!(frame.len(), size);
        frames.push(frame);
    }
    frames
}

/// MP3 frames depend on the ones before them; each thread's job starts
/// with copies of the packets before it, so that the bytes are those of one
/// thread.
#[test]
fn mp3_threads_write_the_same_bytes() {
    for (lsf, rate, samples) in [(false, 44_100.0, 1152), (true, 22_050.0, 576)] {
        let frames = mp3_frames(1400, 5, lsf, |_| false);
        let file = matroska("A_MPEG/L3", &[], rate, 2, &frames);
        let run = |threads| {
            let mut sink = Keep::default();
            let options = Options {
                threads,
                ..options(SampleFormat::F32)
            };
            let summary = decode(
                SliceSource::new(&file),
                &options,
                &Limits::default(),
                &mut sink,
            )
            .unwrap();
            (summary, sink.bytes)
        };
        let (one, bytes) = run(1);
        assert_eq!(one.samples, 1400 * samples);
        assert!(one.warnings.is_empty(), "{:?}", one.warnings);
        // Not silence: the granules decode the random data.
        assert!(bytes.iter().filter(|&&b| b != 0).count() > bytes.len() / 2);
        for threads in [2, 3, 8] {
            let (summary, other) = run(threads);
            assert_eq!(summary, one, "{threads} threads, lsf {lsf}");
            assert!(other == bytes, "{threads} threads, lsf {lsf}");
        }
    }
}

/// Decodes `file` with `threads` threads to 32-bit floats.
fn decode_with(
    file: &[u8],
    threads: usize,
    limits: &Limits,
) -> Result<(coreav_audio::Summary, Vec<u8>), Error> {
    let mut sink = Keep::default();
    let options = Options {
        threads,
        ..options(SampleFormat::F32)
    };
    let summary = decode(SliceSource::new(file), &options, limits, &mut sink)?;
    Ok((summary, sink.bytes))
}

/// The same bytes on every thread count for MP3 streams whose frames
/// switch between one and two channels, or that are cut and joined (frames
/// whose main data reaches into the other stream's), and for frames that
/// carry no main data at all (priming is bounded there).
#[test]
fn hard_mp3_streams_write_the_same_bytes_on_threads() {
    let modes = mp3_frames(700, 11, false, |i| (i / 3) % 2 == 1);
    let a = mp3_frames(700, 21, false, |_| false);
    let b = mp3_frames(700, 22, false, |_| false);
    let spliced: Vec<Vec<u8>> = (0..700)
        .map(|i| {
            if (i / 2) % 2 == 0 {
                a[i].clone()
            } else {
                b[i].clone()
            }
        })
        .collect();
    let lsf_a = mp3_frames(700, 23, true, |_| false);
    let lsf_b = mp3_frames(700, 24, true, |i| i % 5 == 0);
    let lsf_spliced: Vec<Vec<u8>> = (0..700)
        .map(|i| {
            if (i / 3) % 2 == 0 {
                lsf_a[i].clone()
            } else {
                lsf_b[i].clone()
            }
        })
        .collect();
    // Free format, mono, no main data: 4 bytes of header, 17 of side
    // information.
    let mut empty = vec![0xFF, 0xFB, 0x00, 0xC0];
    empty.resize(21, 0);
    let empty = vec![empty; 4000];
    for (name, frames, rate) in [
        ("modes", &modes, 44_100.0),
        ("spliced", &spliced, 44_100.0),
        ("lsf spliced", &lsf_spliced, 22_050.0),
        ("no main data", &empty, 44_100.0),
    ] {
        let file = matroska("A_MPEG/L3", &[], rate, 2, frames);
        let (one, bytes) = decode_with(&file, 1, &Limits::default()).unwrap();
        for threads in [2, 3, 8] {
            let (summary, other) = decode_with(&file, threads, &Limits::default()).unwrap();
            assert_eq!(summary, one, "{name}, {threads} threads");
            assert!(other == bytes, "{name}, {threads} threads");
        }
    }
}

/// Bytes that hold no frame in a stream of cut frames are bounded work:
/// a free-format header followed by megabytes of junk ends quickly, its
/// runs of junk concealed and counted.
#[test]
fn junk_after_a_free_format_header_is_bounded() {
    let mut free = vec![0xFF, 0xFB, 0x00, 0x00];
    free.resize(44, 0);
    let mut packets = vec![free.clone(), vec![0x11, 0x11], free];
    packets.extend(std::iter::repeat_n(vec![0x11; 1000], 4000));
    let file = matroska("A_MPEG/L3", &[], 44_100.0, 2, &packets);
    let result = decode_with(&file, 1, &Limits::default());
    assert!(
        matches!(result, Err(Error::LimitExceeded(LimitKind::DecodeErrors))),
        "{result:?}"
    );
}

/// A stream cut anywhere decodes as its frames when it starts off a frame
/// (bytes before the first header), and a run of junk in it, however many
/// packets it spans, is one concealed packet.
#[test]
fn mp3_cut_anywhere_with_junk_decodes_as_frames() {
    let frames = mp3_frames(60, 13, false, |_| false);
    let reference = matroska("A_MPEG/L3", &[], 44_100.0, 2, &frames);
    let (summary, bytes) = decode_with(&reference, 1, &Limits::default()).unwrap();
    let cut = |stream: &[u8], size: usize| {
        let packets: Vec<Vec<u8>> = stream.chunks(size).map(<[u8]>::to_vec).collect();
        decode_with(
            &matroska("A_MPEG/L3", &[], 44_100.0, 2, &packets),
            1,
            &Limits::default(),
        )
        .unwrap()
    };
    // Bytes before the first frame.
    for lead in [10, 100] {
        let mut stream = vec![0u8; lead];
        stream.extend(frames.concat());
        for size in [1000, 333] {
            let (other, out) = cut(&stream, size);
            assert_eq!(
                other.samples, summary.samples,
                "lead {lead}, packets of {size}"
            );
            assert!(out == bytes, "lead {lead}, packets of {size}");
        }
    }
    // 1000 bytes of junk between frames 30 and 31, in packets of 100 and
    // 1000 bytes: one packet concealed.
    let mut stream = frames[..30].concat();
    stream.extend(vec![0u8; 1000]);
    stream.extend(frames[30..].concat());
    for size in [100, 1000] {
        let (other, _) = cut(&stream, size);
        assert!(
            matches!(&other.warnings[..], [Warning::Concealed { packets: 1, .. }]),
            "packets of {size}: {:?}",
            other.warnings
        );
        assert_eq!(other.samples, summary.samples + 1152, "packets of {size}");
    }
}

/// A packet above `max_packet_bytes` ends the request with the limit,
/// also after the first packet.
#[test]
fn a_packet_limit_mid_stream_fails_the_request() {
    let mut frames = mp3_frames(20, 3, false, |_| false);
    frames.insert(10, vec![0u8; 5000]);
    let file = matroska("A_MPEG/L3", &[], 44_100.0, 2, &frames);
    let limits = Limits {
        max_packet_bytes: 4096,
        ..Limits::default()
    };
    for threads in [1, 4] {
        let result = decode_with(&file, threads, &limits);
        assert!(
            matches!(result, Err(Error::LimitExceeded(LimitKind::PacketBytes))),
            "{threads} threads: {result:?}"
        );
    }
}

/// Layers I and II are refused by their names.
#[test]
fn mp3_streams_of_layers_one_and_two_are_named() {
    for (byte, name) in [(0xFF, "mp1"), (0xFD, "mp2")] {
        let mut frame = vec![0xFF, byte, 0x90, 0x00];
        frame.resize(417, 0);
        let file = matroska("A_MPEG/L3", &[], 44_100.0, 2, &[frame]);
        let result = decode_with(&file, 1, &Limits::default());
        assert!(
            matches!(&result, Err(Error::CodecNotDecoded(n)) if n == name),
            "{result:?}"
        );
    }
}

/// MP3 in samples of any size, as some MOV and MP4 writers store it: the
/// bytes are cut at frame headers again, and decode as the frames do.
#[test]
fn mp3_in_packets_of_any_size_decodes_as_frames() {
    let frames = mp3_frames(60, 9, false, |_| false);
    let decode_file = |packets: &[Vec<u8>]| {
        let file = matroska("A_MPEG/L3", &[], 44_100.0, 2, packets);
        let mut sink = Keep::default();
        let summary = decode(
            SliceSource::new(&file),
            &options(SampleFormat::F32),
            &Limits::default(),
            &mut sink,
        )
        .unwrap();
        (summary.samples, summary.warnings, sink.bytes)
    };
    let (samples, warnings, bytes) = decode_file(&frames);
    assert_eq!(samples, 60 * 1152);
    assert!(warnings.is_empty());
    let stream: Vec<u8> = frames.concat();
    for size in [100, 417, 1000, 7] {
        let packets: Vec<Vec<u8>> = stream.chunks(size).map(<[u8]>::to_vec).collect();
        let (other_samples, other_warnings, other) = decode_file(&packets);
        assert_eq!(other_samples, samples, "{size}");
        assert!(other_warnings.is_empty(), "{size}: {other_warnings:?}");
        assert!(other == bytes, "{size}");
    }
}

/// A frame that changes the stream's format is a refusal, not damage: the
/// request fails instead of writing silence for it.
#[test]
fn a_frame_that_changes_the_format_fails_the_request() {
    let mut frames: Vec<_> = (0..3).map(|n| constant_frame(n, 1024, 1, 16, 9)).collect();
    // Frame 1 at 48 kHz: new sample rate bits, new CRCs.
    let frame = &mut frames[1];
    frame[2] = 0x7A;
    frame[7] = crc8(&frame[..7]);
    let body = frame.len() - 2;
    let crc = crc16(&frame[..body]);
    frame[body..].copy_from_slice(&crc.to_be_bytes());
    let file = matroska("A_FLAC", &flac_private(44_100, 1, 16), 44_100.0, 1, &frames);
    let result = decode(
        SliceSource::new(&file),
        &options(SampleFormat::S16),
        &Limits::default(),
        &mut Keep::default(),
    );
    assert!(matches!(result, Err(Error::Unsupported(_))), "{result:?}");
}

/// Decodes `file` with `options` into bytes.
fn convert(
    file: &[u8],
    options: &Options,
    limits: &Limits,
) -> Result<(coreav_audio::Summary, Vec<u8>), Error> {
    let mut sink = Keep::default();
    let summary = decode(SliceSource::new(file), options, limits, &mut sink)?;
    Ok((summary, sink.bytes))
}

/// The stream's own rate and channels, asked for, change nothing.
#[test]
fn the_streams_rate_and_channels_write_the_same_bytes() {
    let file = matroska(
        "A_MPEG/L3",
        &[],
        44_100.0,
        2,
        &mp3_frames(60, 3, false, |_| false),
    );
    for format in [SampleFormat::S16, SampleFormat::F32] {
        let plain = convert(&file, &options(format), &Limits::default()).unwrap();
        let asked = Options {
            sample_rate: Some(44_100),
            channels: Some(2),
            ..options(format)
        };
        assert_eq!(convert(&file, &asked, &Limits::default()).unwrap(), plain);
    }
}

/// Resampling and downmixing write the same bytes on any number of
/// threads, ceil(samples × to / from) samples, in the output's format.
#[test]
fn conversion_writes_the_same_bytes_on_threads() {
    let file = matroska(
        "A_MPEG/L3",
        &[],
        44_100.0,
        2,
        &mp3_frames(400, 9, false, |n| n % 50 == 7),
    );
    let native = convert(&file, &options(SampleFormat::F32), &Limits::default())
        .unwrap()
        .0;
    for (rate, channels) in [
        (Some(16_000), Some(1)),
        (Some(48_000), None),
        (Some(44_056), Some(1)),
        (None, Some(1)),
    ] {
        let run = |threads| {
            let options = Options {
                threads,
                sample_rate: rate,
                channels,
                ..options(SampleFormat::S16)
            };
            convert(&file, &options, &Limits::default()).unwrap()
        };
        let (one, bytes) = run(1);
        let to = u64::from(rate.unwrap_or(44_100));
        assert_eq!(
            one.samples,
            (native.samples * to).div_ceil(44_100),
            "{rate:?}"
        );
        assert_eq!(one.output.sample_rate as u64, to);
        assert_eq!(one.output.channels, channels.unwrap_or(2));
        assert_eq!(
            one.output.channel_mask,
            if channels == Some(1) { 0x4 } else { 0x3 }
        );
        assert_eq!(one.stream, native.stream);
        assert_eq!(
            bytes.len() as u64,
            one.samples * u64::from(one.output.channels) * 2
        );
        assert!(bytes.iter().any(|&b| b != 0));
        for threads in [2, 8] {
            assert_eq!(
                run(threads),
                (one.clone(), bytes.clone()),
                "{rate:?} {channels:?} {threads}"
            );
        }
    }
}

/// Stereo to mono at the stream's rate is ½ (L + R), exactly.
#[test]
fn stereo_to_mono_is_the_mean() {
    let file = matroska(
        "A_MPEG/L3",
        &[],
        44_100.0,
        2,
        &mp3_frames(40, 4, false, |_| false),
    );
    let floats = |bytes: &[u8]| -> Vec<f32> {
        bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect()
    };
    let stereo = floats(
        &convert(&file, &options(SampleFormat::F32), &Limits::default())
            .unwrap()
            .1,
    );
    let mono_options = Options {
        channels: Some(1),
        ..options(SampleFormat::F32)
    };
    let (summary, mono) = convert(&file, &mono_options, &Limits::default()).unwrap();
    let mono = floats(&mono);
    assert_eq!(summary.output.channels, 1);
    assert_eq!(mono.len() * 2, stereo.len());
    for (m, lr) in mono.iter().zip(stereo.chunks_exact(2)) {
        assert_eq!(m.to_bits(), (0.5 * lr[0] + 0.5 * lr[1]).to_bits());
    }
}

#[test]
fn conversions_outside_the_supported_ones_are_refused() {
    let file = matroska(
        "A_MPEG/L3",
        &[],
        44_100.0,
        2,
        &mp3_frames(10, 1, false, |_| false),
    );
    for (rate, channels) in [
        (Some(999), None),
        (Some(768_001), None),
        (None, Some(3)),
        (None, Some(0)),
    ] {
        let options = Options {
            sample_rate: rate,
            channels,
            ..Options::default()
        };
        let result = convert(&file, &options, &Limits::default());
        assert!(
            matches!(result, Err(Error::Unsupported(_))),
            "{rate:?} {channels:?}: {result:?}"
        );
    }
    // Upsampled, the output reaches its limit counted in output samples:
    // 4,096 samples at 44.1 kHz are 71,332 at 768 kHz.
    let frames = vec![constant_frame(0, 4096, 1, 16, 1000)];
    let file = matroska("A_FLAC", &flac_private(44_100, 1, 16), 44_100.0, 1, &frames);
    let options = Options {
        sample_rate: Some(768_000),
        ..Options::default()
    };
    let limits = Limits {
        max_output_samples: 50_000,
        ..Limits::default()
    };
    let result = convert(&file, &options, &limits);
    assert!(
        matches!(result, Err(Error::LimitExceeded(LimitKind::OutputSamples))),
        "{result:?}"
    );
}
