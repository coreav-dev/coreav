//! AAC LC through the pipeline, with the opt-in feature `aac`
//! (`docs/design/audio-decode.md`, "AAC-LC"), on Matroska files built
//! here from frames written here: noise bands (whose samples depend on
//! the packet and its position), damaged packets, thread counts, limits
//! and the refusals of HE-AAC.

#![cfg(feature = "aac")]
#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

use coreav_audio::{
    Codec, Error, LimitKind, Limits, Options, OutputFormat, SampleFormat, Sink, Summary, Warning,
    decode,
};
use coreav_isobmff::SliceSource;

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

/// Writes bits most significant first.
#[derive(Default)]
struct Bits {
    bytes: Vec<u8>,
    used: u32,
}

impl Bits {
    fn put(&mut self, value: u32, n: u32) {
        for i in (0..n).rev() {
            if self.used % 8 == 0 {
                self.bytes.push(0);
            }
            let bit = ((value >> i) & 1) as u8;
            *self.bytes.last_mut().unwrap() |= bit << (7 - self.used % 8);
            self.used += 1;
        }
    }
}

/// A small generator for the test streams.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as u32
    }
}

/// An `AudioSpecificConfig`: AAC LC at 44.1 kHz, frames of 1,024.
fn config(object_type: u32, channel_configuration: u32) -> Vec<u8> {
    let mut bits = Bits::default();
    bits.put(object_type, 5);
    bits.put(4, 4);
    bits.put(channel_configuration, 4);
    bits.put(0, 3);
    bits.bytes
}

/// One long-window `individual_channel_stream()`: `noise` bands coded as
/// noise (codebook 13), each band's energy the first's, or no bands.
fn ics(bits: &mut Bits, rng: &mut Rng, noise: u32) {
    bits.put(100, 8); // global_gain
    bits.put(0, 1); // ics_reserved_bit
    bits.put(0, 2); // ONLY_LONG_SEQUENCE
    bits.put(rng.next() & 1, 1); // window_shape
    bits.put(noise, 6); // max_sfb
    bits.put(0, 1); // predictor_data_present
    if noise > 0 {
        bits.put(13, 4); // sect_cb
        let mut left = noise;
        while left >= 31 {
            bits.put(31, 5);
            left -= 31;
        }
        bits.put(left, 5);
        // The first energy, 9 bits; the others the same (Huffman 0).
        bits.put(280 + rng.next() % 16, 9);
        for _ in 1..noise {
            bits.put(0, 1);
        }
    }
    bits.put(0, 3); // pulse, TNS and gain control data absent
}

/// A `raw_data_block()` of one SCE (one channel) or one CPE (two), its
/// channels' bands noise or silent, then optionally a fill element whose
/// payload is SBR data.
fn frame(rng: &mut Rng, channels: u32, noise: bool, sbr: bool) -> Vec<u8> {
    let mut bits = Bits::default();
    let bands = if noise { 40 } else { 0 };
    if channels == 1 {
        bits.put(0, 3); // ID_SCE
        bits.put(0, 4);
        ics(&mut bits, rng, bands);
    } else {
        bits.put(1, 3); // ID_CPE
        bits.put(0, 4);
        bits.put(0, 1); // common_window
        ics(&mut bits, rng, bands);
        ics(&mut bits, rng, bands);
    }
    if sbr {
        bits.put(6, 3); // ID_FIL
        bits.put(2, 4); // count
        bits.put(0xd, 4); // EXT_SBR_DATA
        bits.put(0, 12);
    }
    bits.put(7, 3); // ID_END
    bits.bytes
}

/// `count` frames: mostly noise, some silent, every 97th damaged (cut
/// short or overwritten).
fn stream(count: usize, channels: u32, seed: u64) -> Vec<Vec<u8>> {
    let mut rng = Rng(seed);
    (0..count)
        .map(|i| {
            let mut frame = frame(&mut rng, channels, i % 7 != 3, false);
            if i % 97 == 50 {
                if i % 2 == 0 {
                    frame.truncate(3);
                } else {
                    for b in frame.iter_mut().skip(2) {
                        *b = rng.next() as u8;
                    }
                }
            }
            frame
        })
        .collect()
}

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

/// A Matroska file with one audio track (`CodecPrivate` when `private` is
/// given) and one SimpleBlock per frame.
fn matroska(codec_id: &str, private: Option<&[u8]>, channels: u64, frames: &[Vec<u8>]) -> Vec<u8> {
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
    let mut audio = element(&[0xB5], &44_100f64.to_be_bytes());
    audio.extend(uint(&[0x9F], channels));
    let mut entry = uint(&[0xD7], 1);
    entry.extend(uint(&[0x73, 0xC5], 1));
    entry.extend(uint(&[0x83], 2));
    entry.extend(element(&[0x86], codec_id.as_bytes()));
    if let Some(private) = private {
        entry.extend(element(&[0x63, 0xA2], private));
    }
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

fn aac_file(channels: u32, frames: &[Vec<u8>]) -> Vec<u8> {
    matroska(
        "A_AAC",
        Some(&config(2, channels)),
        u64::from(channels),
        frames,
    )
}

fn decode_with(file: &[u8], threads: usize, limits: &Limits) -> Result<(Summary, Vec<u8>), Error> {
    let mut sink = Keep::default();
    let options = Options {
        threads,
        format: SampleFormat::F32,
        ..Options::default()
    };
    let summary = decode(SliceSource::new(file), &options, limits, &mut sink)?;
    Ok((summary, sink.bytes))
}

fn samples(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

#[test]
fn decodes_noise_bands_and_conceals_damaged_packets() {
    let frames = stream(400, 2, 1);
    let (summary, bytes) = decode_with(&aac_file(2, &frames), 1, &Limits::default()).unwrap();
    assert_eq!(summary.stream.codec, Codec::Aac);
    assert_eq!(summary.stream.sample_rate, 44_100);
    assert_eq!(summary.output.channel_mask, 0x3);
    assert_eq!(summary.packets, 400);
    assert_eq!(summary.samples, 400 * 1024);
    let damaged = (0..400).filter(|i| i % 97 == 50).count() as u64;
    assert!(
        summary
            .warnings
            .iter()
            .any(|w| matches!(w, Warning::Concealed { packets, .. } if *packets == damaged)),
        "{:?}",
        summary.warnings
    );
    let x = samples(&bytes);
    let loud = x.iter().filter(|v| v.abs() > 1e-4).count();
    assert!(loud > x.len() / 4, "{loud} of {}", x.len());
}

/// Noise bands depend on the packet's position as well as its bytes: the
/// same packet frame after frame decodes to different noise, not to a
/// periodic signal.
#[test]
fn a_repeated_packet_does_not_repeat_its_noise() {
    let mut rng = Rng(7);
    let packet = frame(&mut rng, 1, true, false);
    let frames = vec![packet; 6];
    let (_, bytes) = decode_with(&aac_file(1, &frames), 1, &Limits::default()).unwrap();
    let x = samples(&bytes);
    // Blocks 2 and 3 are each one frame's output plus its predecessor's
    // overlap, from identical packets.
    assert_ne!(x[2048..3072], x[3072..4096]);
}

#[test]
fn threads_write_the_same_bytes() {
    for (channels, seed) in [(1, 2), (2, 3)] {
        let file = aac_file(channels, &stream(1500, channels, seed));
        let (one, bytes) = decode_with(&file, 1, &Limits::default()).unwrap();
        for threads in [2, 3, 8] {
            let (summary, other) = decode_with(&file, threads, &Limits::default()).unwrap();
            assert_eq!(summary, one, "{channels} channels, {threads} threads");
            assert!(other == bytes, "{channels} channels, {threads} threads");
        }
    }
}

/// Every frame codes 1,024 samples per channel, so the output limit
/// bounds the work of any number of frames.
#[test]
fn a_flood_of_frames_stops_at_the_output_limit() {
    let mut rng = Rng(5);
    let frames: Vec<_> = (0..2000).map(|_| frame(&mut rng, 2, true, false)).collect();
    let limits = Limits {
        max_output_samples: 1_000_000,
        ..Limits::default()
    };
    for threads in [1, 4] {
        let mut sink = Keep::default();
        let options = Options {
            threads,
            format: SampleFormat::F32,
            ..Options::default()
        };
        let result = decode(
            SliceSource::new(&aac_file(2, &frames)),
            &options,
            &limits,
            &mut sink,
        );
        assert!(
            matches!(result, Err(Error::LimitExceeded(LimitKind::OutputSamples))),
            "{result:?}"
        );
        assert!(sink.bytes.len() / 4 <= 1_000_000);
    }
}

#[test]
fn a_configuration_past_max_channels_is_refused() {
    // Channel configuration 7: eight channels.
    let file = matroska("A_AAC", Some(&config(2, 7)), 8, &[vec![0xE0]]);
    let limits = Limits {
        max_channels: 6,
        ..Limits::default()
    };
    let result = decode_with(&file, 1, &limits);
    assert!(
        matches!(result, Err(Error::LimitExceeded(LimitKind::Channels))),
        "{result:?}"
    );
}

#[test]
fn he_aac_and_other_object_types_are_refused_by_name() {
    let mut rng = Rng(9);
    let frames = vec![frame(&mut rng, 2, true, false)];
    let named = |file: &[u8], want: &str| {
        let result = decode_with(file, 1, &Limits::default());
        assert!(
            matches!(&result, Err(Error::CodecNotDecoded(name)) if name == want),
            "{want}: {result:?}"
        );
    };
    // SBR signalled explicitly: object type 5.
    named(
        &matroska("A_AAC", Some(&config(5, 2)), 2, &frames),
        "HE-AAC (SBR)",
    );
    // Implicitly: an SBR payload in the first packet.
    let sbr = vec![frame(&mut rng, 2, true, true)];
    named(&aac_file(2, &sbr), "HE-AAC (SBR)");
    named(
        &matroska("A_AAC", Some(&config(1, 2)), 2, &frames),
        "AAC Main",
    );
    // Legacy codec IDs without a configuration.
    named(
        &matroska("A_AAC/MPEG4/LC/SBR", None, 2, &frames),
        "HE-AAC (SBR)",
    );
    named(&matroska("A_AAC/MPEG2/MAIN", None, 2, &frames), "AAC Main");
    // An SBR payload in a later packet ends the request.
    let mut later = stream(10, 2, 4);
    later.push(frame(&mut rng, 2, true, true));
    let result = decode_with(&aac_file(2, &later), 1, &Limits::default());
    assert!(matches!(result, Err(Error::Unsupported(_))), "{result:?}");
}

/// A legacy codec ID without `CodecPrivate`: the configuration comes from
/// the track's rate and channels.
#[test]
fn a_legacy_codec_id_decodes_from_the_tracks_rate_and_channels() {
    let frames = stream(20, 2, 6);
    let legacy = matroska("A_AAC/MPEG4/LC", None, 2, &frames);
    let (summary, bytes) = decode_with(&legacy, 1, &Limits::default()).unwrap();
    assert_eq!(summary.stream.channels, 2);
    assert_eq!(summary.stream.sample_rate, 44_100);
    let (_, configured) = decode_with(&aac_file(2, &frames), 1, &Limits::default()).unwrap();
    assert!(bytes == configured);
}
