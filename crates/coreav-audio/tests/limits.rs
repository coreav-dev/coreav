//! Each field of `Limits` at its boundary, the default values, and the
//! demuxer limits passing through to the demuxers.
#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

use coreav_audio::mp3::Mp3Decoder;
use coreav_audio::{Error, LimitKind, Limits, Options, OutputFormat, Pcm, Sink, decode};
use coreav_isobmff::SliceSource;

const RATE: u32 = 44_100;
const BLOCK: u16 = 1000;

#[derive(Default)]
struct Keep(Vec<u8>);

impl Sink for Keep {
    fn start(&mut self, _: &OutputFormat) -> Result<(), Error> {
        Ok(())
    }
    fn write(&mut self, samples: &[u8]) -> Result<(), Error> {
        self.0.extend_from_slice(samples);
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

fn streaminfo(channels: u32) -> Vec<u8> {
    let mut w = BitWriter::default();
    w.put(16, 16);
    w.put(65535, 16);
    w.put(0, 24);
    w.put(0, 24);
    w.put(u64::from(RATE), 20);
    w.put(u64::from(channels - 1), 3);
    w.put(15, 5);
    w.put(0, 36);
    w.put(0, 64);
    w.put(0, 64);
    w.bytes
}

fn flac_private(channels: u32) -> Vec<u8> {
    let mut private = b"fLaC".to_vec();
    private.extend_from_slice(&[0x80, 0x00, 0x00, 0x22]);
    private.extend(streaminfo(channels));
    private
}

/// A FLAC frame of `channels` constant 16-bit subframes of `BLOCK` samples.
fn frame(number: u8, channels: u8) -> Vec<u8> {
    let mut frame = vec![0xFF, 0xF8, 0x79, ((channels - 1) << 4) | (4 << 1), number];
    frame.extend_from_slice(&(BLOCK - 1).to_be_bytes());
    frame.push(crc8(&frame));
    for _ in 0..channels {
        frame.extend_from_slice(&[0, 0x03, 0xE8]);
    }
    let crc = crc16(&frame);
    frame.extend_from_slice(&crc.to_be_bytes());
    frame
}

fn frames(count: u8, channels: u8) -> Vec<Vec<u8>> {
    (0..count).map(|n| frame(n, channels)).collect()
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

fn matroska(channels: u32, packets: &[Vec<u8>]) -> Vec<u8> {
    matroska_track(b"A_FLAC", &flac_private(channels), channels, packets)
}

/// A Matroska file with one audio track of `codec_id` and one SimpleBlock
/// per entry of `packets`.
fn matroska_track(codec_id: &[u8], private: &[u8], channels: u32, packets: &[Vec<u8>]) -> Vec<u8> {
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
    let mut audio = element(&[0xB5], &f64::from(RATE).to_be_bytes());
    audio.extend(uint(&[0x9F], u64::from(channels)));
    let mut entry = uint(&[0xD7], 1);
    entry.extend(uint(&[0x73, 0xC5], 1));
    entry.extend(uint(&[0x83], 2));
    entry.extend(element(&[0x86], codec_id));
    entry.extend(element(&[0x63, 0xA2], private));
    entry.extend(element(&[0xE1], &audio));
    let tracks = element(&[0x16, 0x54, 0xAE, 0x6B], &element(&[0xAE], &entry));
    let mut cluster = uint(&[0xE7], 0);
    for packet in packets {
        let mut block = vec![0x81, 0x00, 0x00, 0x80];
        block.extend_from_slice(packet);
        cluster.extend(element(&[0xA3], &block));
    }
    let mut segment = element(&[0x15, 0x49, 0xA9, 0x66], &info);
    segment.extend(tracks);
    segment.extend(element(&[0x1F, 0x43, 0xB6, 0x75], &cluster));
    file.extend(element(&[0x18, 0x53, 0x80, 0x67], &segment));
    file
}

/// A silent MPEG-1 Layer III frame at 44.1 kHz and 128 kbit/s, joint
/// stereo or mono (ISO/IEC 11172-3 2.4.1): the header, side information of
/// zeros (no main data before the frame, no bits in any granule) and 417
/// bytes in all.
fn mp3_frame(channels: u8) -> Vec<u8> {
    let (mode, side) = if channels == 2 {
        (0x64, 32)
    } else {
        (0xC4, 17)
    };
    let mut frame = vec![0xFF, 0xFB, 0x90, mode];
    frame.resize(4 + side, 0);
    frame.resize(417, 0);
    frame
}

fn bx(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}

/// A mono 16-bit MP4 FLAC file with one sample per entry of `packets`.
fn mp4(packets: &[Vec<u8>]) -> Vec<u8> {
    let count = packets.len();
    let ftyp = bx(b"ftyp", b"isom\0\0\0\0isom");
    let mdat = bx(b"mdat", &packets.concat());
    let offset = (ftyp.len() + 8) as u32;
    let media = (count * usize::from(BLOCK)) as u32;
    let matrix: Vec<u8> = [0x0001_0000u32, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000]
        .iter()
        .flat_map(|v| v.to_be_bytes())
        .collect();
    let mut mvhd = vec![0u8; 12];
    mvhd.extend_from_slice(&RATE.to_be_bytes());
    mvhd.extend_from_slice(&0u32.to_be_bytes());
    mvhd.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    mvhd.extend_from_slice(&0x0100u16.to_be_bytes());
    mvhd.extend_from_slice(&[0; 10]);
    mvhd.extend_from_slice(&matrix);
    mvhd.extend_from_slice(&[0; 24]);
    mvhd.extend_from_slice(&2u32.to_be_bytes());
    let mut tkhd = vec![0, 0, 0, 3];
    tkhd.extend_from_slice(&[0; 8]);
    tkhd.extend_from_slice(&1u32.to_be_bytes());
    tkhd.extend_from_slice(&[0; 4]);
    tkhd.extend_from_slice(&0u32.to_be_bytes());
    tkhd.extend_from_slice(&[0; 8]);
    tkhd.extend_from_slice(&[0, 0, 0, 0, 0x01, 0x00, 0, 0]);
    tkhd.extend_from_slice(&matrix);
    tkhd.extend_from_slice(&[0; 8]);
    let mut trak = bx(b"tkhd", &tkhd);
    let mut mdhd = vec![0u8; 12];
    mdhd.extend_from_slice(&RATE.to_be_bytes());
    mdhd.extend_from_slice(&media.to_be_bytes());
    mdhd.extend_from_slice(&[0x55, 0xC4, 0, 0]);
    let mut hdlr = vec![0u8; 8];
    hdlr.extend_from_slice(b"soun");
    hdlr.extend_from_slice(&[0; 13]);
    let mut streaminfo = Vec::new();
    streaminfo.extend_from_slice(&BLOCK.to_be_bytes());
    streaminfo.extend_from_slice(&BLOCK.to_be_bytes());
    streaminfo.extend_from_slice(&[0; 6]);
    let packed: u64 = (u64::from(RATE) << 44) | (15 << 36) | media as u64;
    streaminfo.extend_from_slice(&packed.to_be_bytes());
    streaminfo.extend_from_slice(&[0; 16]);
    let mut dfla = vec![0, 0, 0, 0, 0x80, 0, 0, 34];
    dfla.extend_from_slice(&streaminfo);
    let mut entry = vec![0u8; 6];
    entry.extend_from_slice(&1u16.to_be_bytes());
    entry.extend_from_slice(&[0; 8]);
    entry.extend_from_slice(&1u16.to_be_bytes());
    entry.extend_from_slice(&16u16.to_be_bytes());
    entry.extend_from_slice(&[0; 4]);
    entry.extend_from_slice(&(RATE << 16).to_be_bytes());
    entry.extend(bx(b"dfLa", &dfla));
    let mut stsd = vec![0, 0, 0, 0, 0, 0, 0, 1];
    stsd.extend(bx(b"fLaC", &entry));
    let mut stts = vec![0, 0, 0, 0, 0, 0, 0, 1];
    stts.extend_from_slice(&(count as u32).to_be_bytes());
    stts.extend_from_slice(&u32::from(BLOCK).to_be_bytes());
    let mut stsc = vec![0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1];
    stsc.extend_from_slice(&(count as u32).to_be_bytes());
    stsc.extend_from_slice(&1u32.to_be_bytes());
    let mut stsz = vec![0u8; 8];
    stsz.extend_from_slice(&(count as u32).to_be_bytes());
    for packet in packets {
        stsz.extend_from_slice(&(packet.len() as u32).to_be_bytes());
    }
    let mut stco = vec![0, 0, 0, 0, 0, 0, 0, 1];
    stco.extend_from_slice(&offset.to_be_bytes());
    let stbl = [
        bx(b"stsd", &stsd),
        bx(b"stts", &stts),
        bx(b"stsc", &stsc),
        bx(b"stsz", &stsz),
        bx(b"stco", &stco),
    ]
    .concat();
    let dinf = bx(
        b"dinf",
        &bx(
            b"dref",
            &[&[0u8, 0, 0, 0, 0, 0, 0, 1][..], &bx(b"url ", &[0, 0, 0, 1])].concat(),
        ),
    );
    let minf = [bx(b"smhd", &[0; 8]), dinf, bx(b"stbl", &stbl)].concat();
    let mdia = [bx(b"mdhd", &mdhd), bx(b"hdlr", &hdlr), bx(b"minf", &minf)].concat();
    trak.extend(bx(b"mdia", &mdia));
    let moov = [bx(b"mvhd", &mvhd), bx(b"trak", &trak)].concat();
    [ftyp, mdat, bx(b"moov", &moov)].concat()
}

struct Decoded {
    samples: u64,
    channels: u16,
    bytes: Vec<u8>,
    concealed: u64,
}

fn run(file: &[u8], options: &Options, limits: &Limits) -> Result<Decoded, Error> {
    let mut sink = Keep::default();
    let summary = decode(SliceSource::new(file), options, limits, &mut sink)?;
    let concealed = summary
        .warnings
        .iter()
        .map(|w| match w {
            coreav_audio::Warning::Concealed { packets, .. } => *packets,
            _ => 0,
        })
        .sum();
    Ok(Decoded {
        samples: summary.samples,
        channels: summary.output.channels,
        bytes: sink.0,
        concealed,
    })
}

fn limit_of(result: Result<Decoded, Error>) -> LimitKind {
    match result {
        Err(Error::LimitExceeded(kind)) => kind,
        Err(other) => panic!("expected a limit error, got {other:?}"),
        Ok(decoded) => panic!(
            "expected a limit error, decoded {} samples",
            decoded.samples
        ),
    }
}

#[test]
fn defaults_match_the_documented_values() {
    assert_eq!(
        Limits::default(),
        Limits {
            isobmff: coreav_isobmff::Limits::default(),
            matroska: coreav_mkv::Limits::default(),
            max_channels: 32,
            max_packet_bytes: 16 << 20,
            max_output_samples: 1 << 33,
            max_decode_errors: 100,
        }
    );
}

#[test]
fn the_isobmff_limits_reach_the_mp4_demuxer() {
    let file = mp4(&frames(2, 1));
    assert!(run(&file, &Options::default(), &Limits::default()).is_ok());
    let limits = Limits {
        isobmff: coreav_isobmff::Limits {
            max_tracks: 0,
            ..coreav_isobmff::Limits::default()
        },
        ..Limits::default()
    };
    assert!(matches!(
        run(&file, &Options::default(), &limits),
        Err(Error::Isobmff(coreav_isobmff::Error::LimitExceeded {
            limit: coreav_isobmff::LimitKind::Tracks,
            ..
        }))
    ));
}

#[test]
fn the_matroska_limits_reach_the_matroska_demuxer() {
    let file = matroska(1, &frames(2, 1));
    assert!(run(&file, &Options::default(), &Limits::default()).is_ok());
    let limits = Limits {
        matroska: coreav_mkv::Limits {
            max_tracks: 0,
            ..coreav_mkv::Limits::default()
        },
        ..Limits::default()
    };
    assert!(matches!(
        run(&file, &Options::default(), &limits),
        Err(Error::Matroska(coreav_mkv::Error::LimitExceeded {
            limit: coreav_mkv::LimitKind::Tracks,
            ..
        }))
    ));
}

#[test]
fn a_stream_of_exactly_max_channels_decodes_and_one_more_is_refused() {
    let file = matroska(8, &frames(2, 8));
    let limits = |max_channels| Limits {
        max_channels,
        ..Limits::default()
    };
    assert_eq!(
        run(&file, &Options::default(), &limits(8))
            .unwrap()
            .channels,
        8
    );
    assert_eq!(
        limit_of(run(&file, &Options::default(), &limits(7))),
        LimitKind::Channels
    );
}

#[test]
fn an_mp3_stream_of_exactly_max_channels_decodes_and_one_more_is_refused() {
    let file = matroska_track(b"A_MPEG/L3", &[], 2, &[mp3_frame(2), mp3_frame(2)]);
    let limits = |max_channels| Limits {
        max_channels,
        ..Limits::default()
    };
    assert_eq!(
        run(&file, &Options::default(), &limits(2))
            .unwrap()
            .channels,
        2
    );
    assert_eq!(
        limit_of(run(&file, &Options::default(), &limits(1))),
        LimitKind::Channels
    );
}

/// The MP3 decoder on its own: the channels it is given, or else its first
/// frame's.
#[test]
fn an_mp3_decoder_refuses_more_than_max_channels_given_or_found_in_the_first_frame() {
    let limits = |max_channels| Limits {
        max_channels,
        ..Limits::default()
    };
    assert!(Mp3Decoder::new(Some(1), &limits(1)).is_ok());
    assert!(matches!(
        Mp3Decoder::new(Some(2), &limits(1)),
        Err(Error::LimitExceeded(LimitKind::Channels))
    ));
    let mut pcm = Pcm::new();
    let mut decoder = Mp3Decoder::new(None, &limits(1)).unwrap();
    assert!(decoder.decode(&mp3_frame(1), &mut pcm).is_ok());
    let mut decoder = Mp3Decoder::new(None, &limits(1)).unwrap();
    assert!(matches!(
        decoder.decode(&mp3_frame(2), &mut pcm),
        Err(Error::LimitExceeded(LimitKind::Channels))
    ));
    let mut decoder = Mp3Decoder::new(None, &limits(2)).unwrap();
    assert!(decoder.decode(&mp3_frame(2), &mut pcm).is_ok());
}

#[test]
fn a_matroska_packet_of_exactly_max_packet_bytes_is_decoded_and_one_more_is_refused() {
    let packets = frames(2, 1);
    let size = packets[1].len() as u64;
    let file = matroska(1, &packets);
    let limits = |max_packet_bytes| Limits {
        max_packet_bytes,
        ..Limits::default()
    };
    assert_eq!(
        run(&file, &Options::default(), &limits(size))
            .unwrap()
            .samples,
        2 * u64::from(BLOCK)
    );
    assert_eq!(
        limit_of(run(&file, &Options::default(), &limits(size - 1))),
        LimitKind::PacketBytes
    );
}

#[test]
fn an_mp4_packet_of_exactly_max_packet_bytes_is_decoded_and_one_more_is_refused() {
    let packets = frames(2, 1);
    let size = packets[1].len() as u64;
    let file = mp4(&packets);
    let limits = |max_packet_bytes| Limits {
        max_packet_bytes,
        ..Limits::default()
    };
    assert_eq!(
        run(&file, &Options::default(), &limits(size))
            .unwrap()
            .samples,
        2 * u64::from(BLOCK)
    );
    assert_eq!(
        limit_of(run(&file, &Options::default(), &limits(size - 1))),
        LimitKind::PacketBytes
    );
}

#[test]
fn exactly_max_output_samples_are_written_and_one_fewer_is_refused() {
    let mono = matroska(1, &frames(3, 1));
    let stereo = matroska(2, &frames(3, 2));
    let cases = [
        ("mono as stored", &mono, Options::default()),
        ("stereo as stored", &stereo, Options::default()),
        (
            "stereo downmixed",
            &stereo,
            Options {
                channels: Some(1),
                ..Options::default()
            },
        ),
        (
            "mono upsampled",
            &mono,
            Options {
                sample_rate: Some(88_200),
                ..Options::default()
            },
        ),
        (
            "mono downsampled",
            &mono,
            Options {
                sample_rate: Some(22_050),
                ..Options::default()
            },
        ),
    ];
    for (name, file, options) in cases {
        let stored = run(file, &Options::default(), &Limits::default()).unwrap();
        let all = run(file, &options, &Limits::default()).unwrap();
        let stream_total = stored.samples * u64::from(stored.channels);
        let output_total = all.samples * u64::from(all.channels);
        let needed = stream_total.max(output_total);
        let limits = |max_output_samples| Limits {
            max_output_samples,
            ..Limits::default()
        };
        let exact = run(file, &options, &limits(needed)).unwrap();
        assert_eq!(exact.bytes, all.bytes, "{name}");
        let mut kept = Keep::default();
        let refused = decode(
            SliceSource::new(file),
            &options,
            &limits(needed - 1),
            &mut kept,
        );
        assert!(
            matches!(refused, Err(Error::LimitExceeded(LimitKind::OutputSamples))),
            "{name}: {:?}",
            refused.map(|s| s.samples)
        );
        assert!(kept.0.len() < all.bytes.len(), "{name}");
    }
}

#[test]
fn exactly_max_decode_errors_are_concealed_and_one_more_fails() {
    let with_damaged = |damaged: usize| {
        let mut packets = frames(1, 1);
        packets.extend(vec![vec![0u8; 20]; damaged]);
        matroska(1, &packets)
    };
    let limits = Limits {
        max_decode_errors: 3,
        ..Limits::default()
    };
    for threads in [1, 4] {
        let options = Options {
            threads,
            ..Options::default()
        };
        let kept = run(&with_damaged(3), &options, &limits).unwrap();
        assert_eq!(kept.concealed, 3, "{threads} threads");
        assert_eq!(
            limit_of(run(&with_damaged(4), &options, &limits)),
            LimitKind::DecodeErrors,
            "{threads} threads"
        );
    }
}
