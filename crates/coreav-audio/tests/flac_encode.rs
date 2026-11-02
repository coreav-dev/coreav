//! Complete FLAC encoder roundtrips, resource limits and terminal failures.
#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

mod support;

use coreav_audio::flac::encode::FlacSink;
use coreav_audio::{Error, LimitKind, Limits, OutputFormat, SampleFormat, Sink};

fn format(channels: u16, bits: u16, rate: u32) -> OutputFormat {
    OutputFormat {
        channels,
        sample_rate: rate,
        format: if bits == 16 {
            SampleFormat::S16
        } else {
            SampleFormat::S24
        },
        channel_mask: [0x4, 0x3, 0x7, 0x33, 0x37, 0x3F, 0x70F, 0x63F][usize::from(channels - 1)],
    }
}

fn encode(format: OutputFormat, samples: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut sink = FlacSink::new(&mut bytes, Limits::default());
    sink.start(&format).unwrap();
    for chunk in samples.chunks(17 * usize::from(format.channels) * format.format.bytes()) {
        sink.write(chunk).unwrap();
    }
    let header = sink.finish().unwrap();
    assert_eq!(header.len(), 42);
    bytes[..header.len()].copy_from_slice(&header);
    bytes
}

#[test]
fn complete_roundtrips_include_all_channels_widths_and_block_tails() {
    for bits in [16, 24] {
        for channels in 1..=8u16 {
            for (case, frames) in [0usize, 1, 15, 16, 4095, 4096, 4097, 8243]
                .into_iter()
                .enumerate()
            {
                let format = format(channels, bits, [8000, 44100, 48000, 96000, 12345][case % 5]);
                let bound = 1i32 << (bits - 1);
                let values: Vec<i32> = (0..frames)
                    .flat_map(|i| {
                        (0..channels).map(move |c| match case % 8 {
                            0 => 0,
                            1 => -bound,
                            2 => bound - 1,
                            3 => (i as i32 * 17 + i32::from(c) * 11) % bound,
                            4 => {
                                if i % 2 == 0 {
                                    -bound
                                } else {
                                    bound - 1
                                }
                            }
                            5 => (i as i32 * 7919 + i32::from(c) * 101) % (2 * bound) - bound,
                            6 => {
                                if i % 71 == 0 {
                                    bound - 1
                                } else {
                                    0
                                }
                            }
                            _ => ((i as f64 * 0.03).sin() * f64::from(bound - 1) * 0.7) as i32,
                        })
                    })
                    .collect();
                let samples: Vec<u8> = values
                    .iter()
                    .flat_map(|value| value.to_le_bytes().into_iter().take(usize::from(bits / 8)))
                    .collect();
                let bytes = encode(format, &samples);
                let decoded = support::decode_raw(&bytes).unwrap();
                assert_eq!(decoded.samples, frames as u64, "{bits}/{channels}/{frames}");
                assert_eq!(decoded.info.total_samples, frames as u64);
                assert_eq!(decoded.info.bits_per_sample, u8::try_from(bits).unwrap());
                assert_eq!(decoded.info.channels, channels as u8);
                assert_eq!(decoded.info.sample_rate, format.sample_rate);
                assert_eq!(decoded.info.md5, support::md5(&samples));
                assert_eq!(decoded.md5, decoded.info.md5, "{bits}/{channels}/{frames}");
                assert_eq!(decoded.crc_failures, 0);
                assert_eq!(decoded.first, values[..values.len().min(64)]);
                assert_eq!(decoded.frames, frames.div_ceil(4096) as u64);
            }
        }
    }
}

#[test]
fn limits_and_invalid_lifecycle_are_terminal() {
    let format = format(2, 16, 44100);
    let mut bytes = Vec::new();
    let mut before = FlacSink::new(&mut bytes, Limits::default());
    assert!(before.write(&[0; 4]).is_err());
    assert!(before.start(&format).is_err());
    assert!(before.finish().is_err());
    assert!(bytes.is_empty());

    let mut repeated = FlacSink::new(Vec::new(), Limits::default());
    repeated.start(&format).unwrap();
    assert!(repeated.start(&format).is_err());
    assert!(repeated.write(&[0; 4]).is_err());
    assert!(repeated.finish().is_err());

    let mut short = FlacSink::new(Vec::new(), Limits::default());
    short.start(&format).unwrap();
    assert!(short.write(&[0; 3]).is_err());
    assert!(short.write(&[0; 4]).is_err());
    assert!(short.finish().is_err());

    let mut limited = FlacSink::new(
        Vec::new(),
        Limits {
            max_output_samples: 2,
            ..Limits::default()
        },
    );
    limited.start(&format).unwrap();
    limited.write(&[0; 4]).unwrap();
    assert!(matches!(
        limited.write(&[0; 4]),
        Err(Error::LimitExceeded(LimitKind::OutputSamples))
    ));
    assert!(limited.finish().is_err());
    let mut channels = FlacSink::new(
        Vec::new(),
        Limits {
            max_channels: 1,
            ..Limits::default()
        },
    );
    assert!(matches!(
        channels.start(&format),
        Err(Error::LimitExceeded(LimitKind::Channels))
    ));
    let mut packet = FlacSink::new(
        Vec::new(),
        Limits {
            max_packet_bytes: 1,
            ..Limits::default()
        },
    );
    packet.start(&format).unwrap();
    packet.write(&[0; 4]).unwrap();
    assert!(matches!(
        packet.finish(),
        Err(Error::LimitExceeded(LimitKind::PacketBytes))
    ));
}

#[test]
fn unsupported_formats_and_output_failures_are_not_successes() {
    let ordinary = format(2, 16, 44100);
    for format in [
        OutputFormat {
            channels: 3,
            channel_mask: 0xB,
            ..ordinary
        },
        OutputFormat {
            channels: 4,
            channel_mask: 0x107,
            ..ordinary
        },
        OutputFormat {
            channels: 5,
            channel_mask: 0x607,
            ..ordinary
        },
        OutputFormat {
            channels: 6,
            channel_mask: 0x60F,
            ..ordinary
        },
        OutputFormat {
            sample_rate: 0,
            ..ordinary
        },
        OutputFormat {
            sample_rate: 1 << 20,
            ..ordinary
        },
        OutputFormat {
            channels: 0,
            ..ordinary
        },
        OutputFormat {
            channels: 9,
            ..ordinary
        },
        OutputFormat {
            channel_mask: 0,
            ..ordinary
        },
        OutputFormat {
            format: SampleFormat::S32,
            ..ordinary
        },
        OutputFormat {
            format: SampleFormat::F32,
            ..ordinary
        },
    ] {
        let mut bytes = Vec::new();
        let mut sink = FlacSink::new(&mut bytes, Limits::default());
        assert!(sink.start(&format).is_err());
        assert!(sink.finish().is_err());
        assert!(bytes.is_empty());
    }
    struct Broken;
    impl std::io::Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut sink = FlacSink::new(Broken, Limits::default());
    assert!(matches!(sink.start(&ordinary), Err(Error::Output(_))));
    assert!(sink.write(&[0; 4]).is_err());
    assert!(sink.finish().is_err());
}

#[test]
fn an_output_failure_after_the_header_cannot_resume() {
    struct HeaderOnly(bool);
    impl std::io::Write for HeaderOnly {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.0 {
                return Err(std::io::ErrorKind::BrokenPipe.into());
            }
            self.0 = true;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut during = FlacSink::new(HeaderOnly(false), Limits::default());
    during.start(&format(1, 16, 44100)).unwrap();
    assert!(matches!(
        during.write(&vec![0; 4096 * 2]),
        Err(Error::Output(_))
    ));
    assert!(during.write(&[0; 2]).is_err());
    assert!(during.finish().is_err());
    let mut at_end = FlacSink::new(HeaderOnly(false), Limits::default());
    at_end.start(&format(1, 16, 44100)).unwrap();
    at_end.write(&[0; 2]).unwrap();
    assert!(matches!(at_end.finish(), Err(Error::Output(_))));
}
