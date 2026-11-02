use super::*;
use std::collections::VecDeque;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

struct Fake {
    calls: Arc<AtomicUsize>,
    drops: DropCounter,
    outputs: VecDeque<RawOutput>,
    consumed: Option<usize>,
    error: Option<Error>,
    trim: (u32, u32),
    cookie: Vec<u8>,
}

#[derive(Default)]
struct DropCounter(Arc<AtomicUsize>);

impl Drop for DropCounter {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

impl Default for Fake {
    fn default() -> Self {
        Self {
            calls: Arc::default(),
            drops: DropCounter::default(),
            outputs: VecDeque::new(),
            consumed: None,
            error: None,
            trim: (0, 24),
            cookie: vec![0x11, 0x90],
        }
    }
}

impl Backend for Fake {
    fn close(&mut self) -> Result<(), Error> {
        self.error.map_or(Ok(()), Err)
    }
    fn append(&mut self, pcm: &[f32]) -> Result<usize, Error> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if let Some(error) = self.error {
            return Err(error);
        }
        Ok(self.consumed.unwrap_or(pcm.len() / 2))
    }
    fn eof(&mut self) -> Result<(), Error> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    fn produce(&mut self, _: usize) -> Result<RawOutput, Error> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.outputs.pop_front().ok_or(Error::Output)
    }
    fn cookie(&mut self, _: usize) -> Result<Vec<u8>, Error> {
        Ok(self.cookie.clone())
    }
    fn trim(&mut self) -> Result<(u32, u32), Error> {
        Ok(self.trim)
    }
    fn identity(&self) -> Identity {
        Identity {
            kind: 1,
            subtype: 2,
            manufacturer: 3,
            version: 4,
        }
    }
}

fn settings() -> Result<Settings, Error> {
    Settings::new(48_000, 2, 192_000, 127)
}
fn limits() -> Limits {
    Limits {
        max_chunk_frames: 8192,
        max_total_frames: 32_768,
        max_output_bytes: 65_536,
        max_packets: 64,
        max_packet_bytes: 4096,
        max_cookie_bytes: 256,
        max_trim_frames: 4096,
    }
}
fn control() -> Control {
    Control::new(Instant::now() + Duration::from_secs(30))
}
fn session(fake: Fake) -> Result<Session<Fake>, Error> {
    Session::new(fake, settings()?, limits(), control())
}
fn raw(status: u32, count: u32) -> RawOutput {
    RawOutput {
        data: if count == 0 { vec![] } else { vec![1, 2] },
        count,
        description: PacketDescription {
            start_offset: 0,
            byte_size: 2,
            variable_frames: 0,
        },
        status,
    }
}

#[test]
fn settings_refuse_conversion_or_undefined_quality() {
    assert_eq!(Settings::new(48_001, 2, 192_000, 127), Err(Error::Input));
    assert_eq!(Settings::new(48_000, 0, 192_000, 127), Err(Error::Input));
    assert_eq!(Settings::new(48_000, 3, 192_000, 127), Err(Error::Input));
    assert_eq!(Settings::new(48_000, 2, 0, 127), Err(Error::Input));
    assert_eq!(Settings::new(48_000, 2, 192_000, 128), Err(Error::Input));
}

#[test]
fn partial_append_has_explicit_frame_identity() -> Result<(), Error> {
    let mut encoder = session(Fake {
        consumed: Some(7),
        ..Fake::default()
    })?;
    assert_eq!(encoder.append(&[0.0; 20], 0)?, 7);
    assert_eq!(encoder.submitted, 7);
    assert_eq!(encoder.append(&[0.0; 6], 7), Err(Error::Output));
    assert_eq!(encoder.append(&[0.0; 20], 7), Err(Error::State));
    Ok(())
}

#[test]
fn invalid_input_fails_before_the_framework_and_poison_is_sticky() -> Result<(), Error> {
    for (pcm, first) in [
        (vec![], 0),
        (vec![0.0], 0),
        (vec![f32::NAN; 2], 0),
        (vec![f32::INFINITY; 2], 0),
        (vec![0.0; 2], 1),
    ] {
        let backend = Fake::default();
        let calls = backend.calls.clone();
        let mut encoder = session(backend)?;
        assert_eq!(encoder.append(&pcm, first), Err(Error::Input));
        assert_eq!(encoder.start_drain(), Err(Error::State));
        assert_eq!(calls.load(Ordering::Relaxed), 0);
    }
    Ok(())
}

#[test]
fn errors_cancelled_and_expired_sessions_dispose_once() -> Result<(), Error> {
    for error in [
        Error::Framework {
            operation: Operation::Append,
            status: -1,
        },
        Error::Output,
    ] {
        let backend = Fake {
            error: Some(error),
            ..Fake::default()
        };
        let drops = backend.drops.0.clone();
        let mut encoder = session(backend)?;
        assert_eq!(encoder.append(&[0.0; 2], 0), Err(error));
        drop(encoder);
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }
    for expired in [false, true] {
        let backend = Fake::default();
        let calls = backend.calls.clone();
        let drops = backend.drops.0.clone();
        let control = if expired {
            Control::new(Instant::now())
        } else {
            let c = control();
            c.cancel();
            c
        };
        assert!(Session::new(backend, settings()?, limits(), control).is_err());
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }
    Ok(())
}

#[test]
fn cancellation_between_calls_refuses_more_native_work() -> Result<(), Error> {
    let backend = Fake::default();
    let calls = backend.calls.clone();
    let mut encoder = session(backend)?;
    encoder.append(&[0.0; 2], 0)?;
    encoder.control.cancel();
    assert_eq!(encoder.start_drain(), Err(Error::Cancelled));
    assert_eq!(encoder.pull().err(), Some(Error::State));
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    Ok(())
}

#[test]
fn eof_is_not_empty_input_or_a_need_input_status() -> Result<(), Error> {
    let mut encoder = session(Fake {
        outputs: VecDeque::from([raw(4, 0)]),
        ..Fake::default()
    })?;
    encoder.start_drain()?;
    assert_eq!(encoder.append(&[0.0; 2], 0), Err(Error::State));
    assert_eq!(encoder.start_drain(), Err(Error::State));
    assert_eq!(encoder.pull().err(), Some(Error::Output));
    assert_eq!(encoder.complete().err(), Some(Error::State));
    let mut early = session(Fake {
        outputs: VecDeque::from([raw(5, 0)]),
        ..Fake::default()
    })?;
    assert_eq!(early.pull().err(), Some(Error::Output));
    Ok(())
}

#[test]
fn final_packet_is_retained_before_checked_completion() -> Result<(), Error> {
    let mut encoder = session(Fake {
        outputs: VecDeque::from([raw(5, 1)]),
        ..Fake::default()
    })?;
    encoder.append(&[0.0; 2000], 0)?;
    encoder.start_drain()?;
    let output = encoder.pull()?;
    assert_eq!(output.flow, Flow::Drained);
    assert_eq!(output.packets.packets().collect::<Vec<_>>(), [&[1, 2][..]]);
    assert_eq!(encoder.pull().err(), Some(Error::State));
    let completion = encoder.complete()?;
    assert_eq!(completion.submitted_frames, 1000);
    assert_eq!(completion.packet_count, 1);
    assert_eq!(completion.trim.trailing_frames(), 24);
    Ok(())
}

#[test]
fn malformed_output_and_nonprogress_are_not_empty_successes() -> Result<(), Error> {
    let mut invalid = vec![
        raw(0, 0),
        raw(1, 1),
        raw(6, 1),
        raw(u32::MAX, 1),
        raw(3, 0),
        raw(3, 2),
    ];
    let mut extent = raw(3, 1);
    extent.description.byte_size = 3;
    invalid.push(extent);
    let mut negative = raw(3, 1);
    negative.description.start_offset = -1;
    invalid.push(negative);
    let mut unclaimed = raw(4, 0);
    unclaimed.data.push(1);
    invalid.push(unclaimed);
    for output in invalid {
        let mut encoder = session(Fake {
            outputs: VecDeque::from([output]),
            ..Fake::default()
        })?;
        assert!(encoder.pull().is_err());
        assert_eq!(encoder.pull().err(), Some(Error::State));
    }
    Ok(())
}

#[test]
fn cumulative_limits_refuse_excess_output() -> Result<(), Error> {
    for (packets, bytes, trim) in [(0, 8, 4096), (8, 1, 4096), (8, 8, 0)] {
        let mut encoder = session(Fake {
            outputs: VecDeque::from([raw(3, 1)]),
            ..Fake::default()
        })?;
        encoder.limits.max_packets = packets;
        encoder.limits.max_output_bytes = bytes;
        encoder.limits.max_trim_frames = trim;
        assert_eq!(encoder.pull().err(), Some(Error::Limit));
        assert_eq!(encoder.packets, 0);
    }
    Ok(())
}

#[test]
fn incomplete_or_inconsistent_finalization_is_refused() -> Result<(), Error> {
    assert_eq!(
        session(Fake::default())?.complete().err(),
        Some(Error::State)
    );
    for (cookie, trim) in [
        (vec![0x12, 0x10], (0, 24)),
        (vec![0x11, 0x90], (0, 23)),
        (vec![0x11, 0x90], (0, 25)),
    ] {
        let mut encoder = session(Fake {
            outputs: VecDeque::from([raw(5, 1)]),
            cookie,
            trim,
            ..Fake::default()
        })?;
        encoder.append(&[0.0; 2000], 0)?;
        encoder.start_drain()?;
        encoder.pull()?;
        assert!(encoder.complete().is_err());
    }
    Ok(())
}

#[test]
fn disposal_failure_cannot_be_a_successful_completion() -> Result<(), Error> {
    let mut encoder = session(Fake {
        outputs: VecDeque::from([raw(5, 1)]),
        ..Fake::default()
    })?;
    encoder.append(&[0.0; 2000], 0)?;
    encoder.start_drain()?;
    encoder.pull()?;
    encoder.backend.error = Some(Error::Framework {
        operation: Operation::Dispose,
        status: -123,
    });
    assert_eq!(
        encoder.complete().err(),
        Some(Error::Framework {
            operation: Operation::Dispose,
            status: -123
        })
    );
    Ok(())
}

#[cfg(target_os = "macos")]
#[test]
fn native_four_cells_drain_and_decode_with_the_independent_rust_decoder()
-> Result<(), Box<dyn std::error::Error>> {
    for (rate, channels) in [(44_100, 1), (44_100, 2), (48_000, 1), (48_000, 2)] {
        let settings = Settings::new(
            rate,
            channels,
            if channels == 1 { 128_000 } else { 192_000 },
            127,
        )?;
        let mut encoder = Encoder::new(settings, limits(), control())?;
        let identity = encoder.identity();
        eprintln!("AAC cell {rate}/{channels}: identity {identity:?}");
        let mut pcm = vec![0.0_f32; 5000 * usize::from(channels)];
        if let Some(sample) = pcm.get_mut(1000 * usize::from(channels)) {
            *sample = 0.75;
        }
        if channels == 2 {
            if let Some(sample) = pcm.get_mut(3000 * 2 + 1) {
                *sample = -0.75;
            }
        }
        let mut accepted = 0;
        let mut packets = Vec::new();
        for _ in 0..128 {
            if accepted < 5000 {
                let remaining = pcm
                    .get(accepted * usize::from(channels)..)
                    .ok_or(Error::Input)?;
                accepted += encoder.append(remaining, accepted as u64)?;
            }
            let output = encoder.pull()?;
            packets.extend(output.packets.packets().map(<[u8]>::to_vec));
            if accepted == 5000 {
                break;
            }
        }
        assert_eq!(accepted, 5000);
        encoder.start_drain()?;
        let mut drained = false;
        for _ in 0..128 {
            let output = encoder.pull()?;
            packets.extend(output.packets.packets().map(<[u8]>::to_vec));
            if output.flow == Flow::Drained {
                drained = true;
                break;
            }
        }
        assert!(drained);
        let declared = encoder.0.backend.trim()?;
        let cookie = encoder.0.backend.cookie(256)?;
        eprintln!(
            "AAC packets {}, source 5000, declared trim {declared:?}",
            packets.len()
        );
        eprintln!("AAC final cookie {cookie:02x?}");
        let completion = encoder.complete()?;
        let config = coreav_aac::Config::parse(
            completion.config.as_bytes(),
            &coreav_aac::Limits { max_channels: 2 },
        )?;
        let mut decoder = coreav_aac::Decoder::new(&config);
        let mut decoded = vec![Vec::new(); usize::from(channels)];
        for packet in packets {
            let mut frame = vec![0.0_f32; 1024 * usize::from(channels)];
            assert_eq!(decoder.decode(&packet, &mut frame)?.samples, 1024);
            for (channel, samples) in decoded.iter_mut().enumerate() {
                samples.extend(frame.iter().skip(channel * 1024).take(1024).copied());
            }
        }
        for (channel, samples) in decoded.iter().enumerate() {
            let leading = completion.trim.leading_frames() as usize;
            let trailing = completion.trim.trailing_frames() as usize;
            let end = samples.len().checked_sub(trailing).ok_or(Error::Output)?;
            let samples = samples.get(leading..end).ok_or(Error::Output)?;
            assert_eq!(samples.len(), 5000);
            let peak = samples
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a.abs().total_cmp(&b.abs()))
                .map(|(i, _)| i)
                .ok_or(Error::Output)?;
            let expected = if channel == 0 { 1000 } else { 3000 };
            assert!(
                peak.abs_diff(expected) <= 1,
                "channel {channel} impulse at {peak}, expected {expected}"
            );
        }
    }
    Ok(())
}
