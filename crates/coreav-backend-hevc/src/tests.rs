use super::*;
use coreav_decode::{CodecConfig, ConfigKind, Policy};
use coreav_frame::{ColorInfo, Pixels};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[test]
fn reconstruction_panics_are_backend_failures_not_media_limits() {
    assert_eq!(
        codec_error(coreav_hevc::Error::Internal("reconstruction_panic")).kind,
        ErrorKind::Backend
    );
}

#[path = "../../coreav-hevc/test-support/pcm.rs"]
mod pcm;
#[path = "../../coreav-hevc/test-support/raster.rs"]
mod raster;
#[path = "../../coreav-hevc/test-support/writer.rs"]
mod writer;

fn access_unit(nals: impl IntoIterator<Item = impl AsRef<[u8]>>, length: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    for nal in nals {
        let nal = nal.as_ref();
        if length {
            bytes.extend_from_slice(&(nal.len() as u32).to_be_bytes());
        } else {
            bytes.extend_from_slice(&[0, 0, 0, 1]);
        }
        bytes.extend_from_slice(nal);
    }
    bytes
}

fn configuration(sets: &[Vec<u8>; 3], depth: u32) -> CodecConfig {
    let mut data = vec![1, if depth == 10 { 2 } else { 1 }];
    data.resize(16, 0);
    data.extend_from_slice(&[1, (depth - 8) as u8, (depth - 8) as u8, 0, 0]);
    data.extend_from_slice(&[3, 3]);
    for (kind, nal) in (32..=34).zip(sets) {
        data.push(kind);
        data.extend_from_slice(&[0, 1]);
        data.extend_from_slice(&(nal.len() as u16).to_be_bytes());
        data.extend_from_slice(nal);
    }
    CodecConfig {
        kind: ConfigKind::Hvcc,
        data,
    }
}

fn packet(data: &[u8], pts: Option<i64>, discard: bool) -> Packet<'_> {
    Packet {
        data,
        pts,
        dts: pts,
        keyframe: true,
        discard,
    }
}

fn terminal(decoder: &mut dyn Decoder) {
    assert!(decoder.receive_frame().is_err());
    assert!(decoder.send_eof().is_err());
    assert!(decoder.send_packet(&packet(&[], None, false)).is_err());
    assert_eq!(decoder.info().backend, BackendKind::Native);
    assert_eq!(decoder.info().isolation, Isolation::InProcess);
}

#[test]
fn native_frames_preserve_depth_pts_and_discard_through_eof() -> Result<()> {
    for depth in [8, 10] {
        let fixture = pcm::pcm_fixture(depth, depth, false)?;
        for length in [false, true] {
            for pts in [None, Some(-9001)] {
                let mut stream = StreamConfig::new(Codec::Hevc);
                if length {
                    stream.config = Some(configuration(&fixture.sets, depth));
                }
                let mut decoder = NativeHevc::new().open(&stream, &Limits::default())?;
                let first = if length {
                    access_unit([&fixture.nal], true)
                } else {
                    access_unit(fixture.sets.iter().chain([&fixture.nal]), false)
                };
                let next = access_unit([&fixture.nal], length);
                let mut frames = Vec::new();
                for (bytes, discard) in [(&first, false), (&next, true), (&next, false)] {
                    decoder.send_packet(&packet(bytes, pts, discard))?;
                    while let Some(frame) = decoder.receive_frame()? {
                        frames.push(frame);
                    }
                }
                decoder.send_eof()?;
                decoder.send_eof()?;
                while let Some(frame) = decoder.receive_frame()? {
                    frames.push(frame);
                }
                assert!(decoder.receive_frame()?.is_none());
                assert_eq!(frames.len(), 2);
                for frame in frames {
                    assert_eq!(frame.pts, pts);
                    let Pixels::Cpu(picture) = frame.pixels else {
                        return Err("not CPU output".into());
                    };
                    assert_eq!(
                        (picture.width(), picture.height(), picture.format().depth()),
                        (8, 8, depth as u8)
                    );
                    for (plane, samples) in picture.planes().iter().zip(&fixture.planes) {
                        let expected: Vec<u8> = samples
                            .iter()
                            .flat_map(|value| {
                                value
                                    .to_le_bytes()
                                    .into_iter()
                                    .take(if depth == 8 { 1 } else { 2 })
                            })
                            .collect();
                        assert_eq!(plane.data(), expected);
                    }
                }
            }
        }
    }
    Ok(())
}

#[test]
fn stored_terminal_zero_padding_follows_the_annex_b_rule_in_configuration_and_packets() -> Result<()>
{
    let fixture = pcm::pcm_fixture(8, 8, false)?;
    let padded = |nal: &Vec<u8>| {
        let mut nal = nal.clone();
        nal.push(0);
        nal
    };
    let sets = [
        padded(&fixture.sets[0]),
        padded(&fixture.sets[1]),
        padded(&fixture.sets[2]),
    ];
    let mut stream = StreamConfig::new(Codec::Hevc);
    stream.config = Some(configuration(&sets, 8));
    let mut decoder = NativeHevc::new().open(&stream, &Limits::default())?;
    decoder.send_packet(&packet(
        &access_unit([padded(&fixture.nal)], true),
        Some(3),
        false,
    ))?;
    decoder.send_eof()?;
    let frame = decoder.receive_frame()?.ok_or("no frame")?;
    assert_eq!(frame.pts, Some(3));
    let Pixels::Cpu(picture) = frame.pixels else {
        return Err("not CPU output".into());
    };
    for (plane, samples) in picture.planes().iter().zip(&fixture.planes) {
        let expected: Vec<u8> = samples.iter().map(|value| *value as u8).collect();
        assert_eq!(plane.data(), expected);
    }
    assert!(decoder.receive_frame()?.is_none());
    Ok(())
}

#[test]
fn malformed_or_multi_picture_access_units_close_after_valid_output() -> Result<()> {
    let fixture = pcm::pcm_fixture(8, 8, false)?;
    for (bad, outputs) in [
        (access_unit([&fixture.nal, &fixture.nal], false), 1),
        (
            access_unit([fixture.nal.as_slice(), &[0, 0, 0x80]], false),
            0,
        ),
    ] {
        let mut decoder =
            NativeHevc::new().open(&StreamConfig::new(Codec::Hevc), &Limits::default())?;
        decoder.send_packet(&packet(&access_unit(&fixture.sets, false), None, false))?;
        assert!(decoder.send_packet(&packet(&bad, Some(7), false)).is_err());
        for _ in 0..outputs {
            assert_eq!(
                decoder.receive_frame()?.ok_or("lost prior frame")?.pts,
                Some(7)
            );
        }
        terminal(&mut *decoder);
    }
    Ok(())
}

#[test]
fn polling_keeps_deferred_entropy_errors_ahead_of_later_packet_framing_errors() -> Result<()> {
    let mut fixture = pcm::pcm_fixture(8, 8, false)?;
    fixture.nal.pop();
    for length in [false, true] {
        let mut stream = StreamConfig::new(Codec::Hevc);
        if length {
            stream.config = Some(configuration(&fixture.sets, 8));
        }
        let data = if length {
            access_unit([&fixture.nal], true)
        } else {
            access_unit(fixture.sets.iter().chain([&fixture.nal]), false)
        };
        let mut serial = NativeHevc::with_options(NativeOptions { threads: Some(1) })
            .open(&stream, &Limits::default())?;
        let expected = serial.send_packet(&packet(&data, None, false)).unwrap_err();
        let mut scheduled = NativeHevc::with_options(NativeOptions { threads: Some(2) })
            .open(&stream, &Limits::default())?;
        let actual = match scheduled.send_packet(&packet(&data, None, false)) {
            Err(error) => error,
            Ok(()) => {
                assert!(scheduled.receive_frame()?.is_none());
                scheduled
                    .send_packet(&packet(&[0, 0, 0, 10, 2], None, false))
                    .unwrap_err()
            }
        };
        assert_eq!(actual.kind, expected.kind);
        assert_eq!(actual.to_string(), expected.to_string());
        terminal(&mut *scheduled);
    }
    Ok(())
}

#[test]
fn a_packet_after_eof_fails_and_keeps_validated_frames() -> Result<()> {
    let fixture = pcm::pcm_fixture(8, 8, false)?;
    let data = access_unit(fixture.sets.iter().chain([&fixture.nal]), false);
    let mut decoder =
        NativeHevc::new().open(&StreamConfig::new(Codec::Hevc), &Limits::default())?;
    decoder.send_packet(&packet(&data, Some(17), false))?;
    decoder.send_eof()?;
    assert!(
        decoder
            .send_packet(&packet(&data, Some(18), false))
            .is_err()
    );
    assert_eq!(
        decoder.receive_frame()?.ok_or("lost EOF frame")?.pts,
        Some(17)
    );
    terminal(&mut *decoder);
    Ok(())
}

#[test]
fn checked_packet_refusal_settles_earlier_entropy_failure() -> Result<()> {
    let mut fixture = pcm::pcm_fixture(8, 8, false)?;
    fixture.nal.pop();
    let mut deferred = 0;
    for length in [false, true] {
        let mut stream = StreamConfig::new(Codec::Hevc);
        if length {
            stream.config = Some(configuration(&fixture.sets, 8));
        }
        let data = if length {
            access_unit([&fixture.nal], true)
        } else {
            access_unit(fixture.sets.iter().chain([&fixture.nal]), false)
        };
        let limits = Limits {
            max_packet_bytes: data.len() as u64 + 8,
            ..Limits::default()
        };
        let serial = NativeHevc::with_options(NativeOptions { threads: Some(1) });
        let mut serial = coreav_decode::open(&stream, &Policy::default(), &limits, &[&serial])?;
        let expected = serial
            .decoder
            .send_packet(&packet(&data, None, false))
            .unwrap_err();
        assert_eq!(expected.kind, ErrorKind::InvalidData);
        let oversized = vec![0; limits.max_packet_bytes as usize + 1];
        for threads in [1, 2, 4, 8] {
            let native = NativeHevc::with_options(NativeOptions {
                threads: Some(threads),
            });
            let mut opened = coreav_decode::open(&stream, &Policy::default(), &limits, &[&native])?;
            let actual = match opened.decoder.send_packet(&packet(&data, None, false)) {
                Err(error) => error,
                Ok(()) => {
                    deferred += 1;
                    assert!(opened.decoder.receive_frame()?.is_none());
                    opened
                        .decoder
                        .send_packet(&packet(&oversized, None, false))
                        .unwrap_err()
                }
            };
            assert_eq!(actual.kind, expected.kind);
            assert_eq!(actual.to_string(), expected.to_string());
            terminal(&mut *opened.decoder);
        }
    }
    if std::thread::available_parallelism().is_ok_and(|workers| workers.get() > 1) {
        assert!(deferred > 0);
    }
    Ok(())
}

#[test]
fn terminal_failures_preserve_validated_frames_through_public_open() -> Result<()> {
    let mut serial_errors = std::collections::BTreeMap::new();
    for (depth, length, receive_early) in [
        (8, false, false),
        (8, false, true),
        (8, true, false),
        (8, true, true),
        (10, false, false),
        (10, false, true),
        (10, true, false),
        (10, true, true),
    ] {
        let fixture = pcm::pcm_fixture(depth, depth, false)?;
        let sample_bytes = if depth == 8 { 1 } else { 2 };
        let mut stream = StreamConfig::new(Codec::Hevc);
        if length {
            stream.config = Some(configuration(&fixture.sets, depth));
        }
        let first = if length {
            access_unit([&fixture.nal], true)
        } else {
            access_unit(fixture.sets.iter().chain([&fixture.nal]), false)
        };
        let next = access_unit([&fixture.nal], length);
        for threads in [1, 2, 4, 8] {
            for checked in [false, true] {
                for fault in 0..5 {
                    let limits = Limits {
                        max_packet_bytes: first.len() as u64 + 8,
                        max_decoded_bytes: if fault == 2 {
                            96 * sample_bytes as u64
                        } else {
                            4 << 30
                        },
                        ..Limits::default()
                    };
                    let native = NativeHevc::with_options(NativeOptions {
                        threads: Some(threads),
                    });
                    let mut decoder = if checked {
                        coreav_decode::open(&stream, &Policy::default(), &limits, &[&native])?
                            .decoder
                    } else {
                        native.open(&stream, &limits)?
                    };
                    decoder.send_packet(&packet(&first, Some(11), false))?;
                    decoder.send_packet(&packet(&next, Some(22), false))?;
                    let mut frames = Vec::new();
                    if receive_early {
                        let early = decoder.receive_frame()?;
                        if threads == 1 {
                            assert!(early.is_some(), "missing serial early frame");
                        }
                        if let Some(frame) = early {
                            frames.push(frame);
                        }
                    }
                    if fault == 3 {
                        decoder.send_eof()?;
                    }
                    let bad = match fault {
                        0 => access_unit([[0, 0, 0x80]], length),
                        1 => vec![0; limits.max_packet_bytes as usize + 1],
                        4 => access_unit([&fixture.nal[..fixture.nal.len() - 1]], length),
                        _ => next.clone(),
                    };
                    let error = match decoder.send_packet(&packet(&bad, Some(33), false)) {
                        Err(error) => error,
                        Ok(()) if fault == 4 => decoder.send_eof().unwrap_err(),
                        Ok(()) => return Err("terminal fixture did not fail".into()),
                    };
                    assert_eq!(
                        error.kind,
                        if matches!(fault, 1 | 2) {
                            ErrorKind::Limit
                        } else {
                            ErrorKind::InvalidData
                        }
                    );
                    assert_eq!(
                        serial_errors
                            .entry((depth, length, fault))
                            .or_insert_with(|| error.clone()),
                        &error
                    );
                    let count = if matches!(fault, 3 | 4) { 2 } else { 1 };
                    while frames.len() < count {
                        frames.push(decoder.receive_frame()?.ok_or("lost terminal frame")?);
                    }
                    for (frame, pts) in frames.into_iter().zip([11, 22]) {
                        assert_eq!(frame.pts, Some(pts));
                        assert_eq!((frame.width(), frame.height()), (8, 8));
                        for (plane, samples) in frame.to_cpu()?.planes().iter().zip(&fixture.planes)
                        {
                            let expected: Vec<u8> = samples
                                .iter()
                                .flat_map(|value| {
                                    value.to_le_bytes().into_iter().take(sample_bytes)
                                })
                                .collect();
                            assert_eq!(plane.data(), expected);
                        }
                    }
                    assert_eq!(decoder.receive_frame().unwrap_err(), error);
                    assert_eq!(decoder.send_eof().unwrap_err(), error);
                    assert_eq!(
                        decoder
                            .send_packet(&packet(&next, None, false))
                            .unwrap_err(),
                        error
                    );
                }
            }
        }
    }
    Ok(())
}

fn native_with_limits_for_test(limits: &Limits, threads: usize) -> Result<NativeDecoder> {
    let name = DECODER_NAME.to_owned();
    let output = Output::new(
        limits.clone(),
        limits.deadline(),
        Default::default(),
        name.capacity(),
    )?;
    let mut core = PictureDecoder::new(DecodeLimits {
        max_memory_bytes: limits.max_memory_bytes as usize,
        ..DecodeLimits::default()
    })
    .with_threads(threads);
    core.set_external_memory(output.retained_bytes()?)?;
    Ok(NativeDecoder {
        decoder: Some(core),
        info: DecoderInfo {
            backend: BackendKind::Native,
            isolation: Isolation::InProcess,
            decoder: name,
        },
        framing: Framing::AnnexB,
        output,
        state: State::Active,
        next_origin: 0,
    })
}

#[test]
fn native_row_receives_are_bounded_serial_prefixes_for_every_short_cadence() -> Result<()> {
    let fixture = pcm::pcm_fixture(8, 8, false)?;
    let first = access_unit(fixture.sets.iter().chain([&fixture.nal]), false);
    let next = access_unit([&fixture.nal], false);
    for threads in [1usize, 2, 4, 8] {
        let effective = threads.min(std::thread::available_parallelism().map_or(1, usize::from));
        for cadence in 0u32..64 {
            let limits = Limits {
                max_packet_bytes: first.len() as u64 + 8,
                max_pending_bytes: 6 * 96,
                max_memory_bytes: 128 << 20,
                ..Limits::default()
            };
            let mut decoder = native_with_limits_for_test(&limits, threads)?;
            let mut returned = 0usize;
            let mut serial_received = 0usize;
            let mut ring_capacity = None;
            for index in 0..6 {
                decoder.send_packet(&packet(
                    if index == 0 { &first } else { &next },
                    Some(index as i64),
                    false,
                ))?;
                assert_eq!(
                    decoder
                        .decoder
                        .as_ref()
                        .is_some_and(PictureDecoder::has_native_rows),
                    effective > 1
                );
                if effective > 1 {
                    let capacity = decoder.output.ready.capacity();
                    assert!(capacity >= 6 + effective);
                    assert_eq!(*ring_capacity.get_or_insert(capacity), capacity);
                }
                if cadence & (1 << index) != 0 {
                    serial_received += usize::from(serial_received < index);
                    if let Some(frame) = decoder.receive_frame()? {
                        assert_eq!(frame.pts, Some(returned as i64));
                        for (plane, samples) in frame.to_cpu()?.planes().iter().zip(&fixture.planes)
                        {
                            assert_eq!(
                                plane.data(),
                                samples.iter().map(|n| *n as u8).collect::<Vec<_>>()
                            );
                        }
                        returned += 1;
                    }
                    assert!(returned <= serial_received);
                    assert!(
                        serial_received - returned < effective,
                        "threads={threads}, cadence={cadence}, index={index}"
                    );
                }
            }
            decoder.send_eof()?;
            while let Some(frame) = decoder.receive_frame()? {
                assert_eq!(frame.pts, Some(returned as i64));
                returned += 1;
            }
            assert_eq!(returned, 6, "threads={threads}, cadence={cadence}");
            assert!(decoder.output.ready.is_empty());
            assert!(decoder.decoder.is_none());
            assert!(decoder.receive_frame()?.is_none());
        }
    }
    Ok(())
}

#[test]
fn later_geometry_can_reduce_future_lag_to_zero_and_retire_old_debt() -> Result<()> {
    if std::thread::available_parallelism().map_or(1, usize::from) < 2 {
        return Ok(());
    }
    let tiny = pcm::pcm_fixture(8, 8, false)?;
    let first = access_unit(tiny.sets.iter().chain([&tiny.nal]), false);
    let next = access_unit([&tiny.nal], false);
    let tall = raster::stream(false)?;
    let grown = access_unit(&tall[..35], false);
    let grown_next = access_unit(&tall[35..], false);
    let mut zero_lag = false;
    for memory in (12u64 << 20..28u64 << 20).step_by(256 << 10) {
        let limits = Limits {
            max_packet_bytes: grown.len() as u64,
            max_pending_bytes: 4 * 16 * 512 * 3 / 2,
            max_memory_bytes: memory,
            ..Limits::default()
        };
        let mut decoder = native_with_limits_for_test(&limits, 2)?;
        decoder.send_packet(&packet(&first, Some(0), false))?;
        decoder.send_packet(&packet(&next, Some(1), false))?;
        if !decoder
            .decoder
            .as_ref()
            .is_some_and(PictureDecoder::has_native_rows)
        {
            continue;
        }
        assert!(decoder.receive_frame()?.is_none());
        assert!(decoder.receive_frame()?.is_none());
        decoder.send_packet(&packet(&grown, Some(2), false))?;
        for pts in [0, 1] {
            let frame = decoder.receive_frame()?.ok_or("lost old-geometry output")?;
            assert_eq!(
                (frame.pts, frame.width(), frame.height()),
                (Some(pts), 8, 8)
            );
            for (plane, samples) in frame.to_cpu()?.planes().iter().zip(&tiny.planes) {
                assert_eq!(
                    plane.data(),
                    samples.iter().map(|n| *n as u8).collect::<Vec<_>>()
                );
            }
        }
        assert!(decoder.receive_frame()?.is_none());
        decoder.send_packet(&packet(&grown_next, Some(3), false))?;
        let retired = !decoder
            .decoder
            .as_ref()
            .is_some_and(PictureDecoder::has_native_rows);
        decoder.send_eof()?;
        for picture in 0..2 {
            let frame = decoder.receive_frame()?.ok_or("lost new-geometry output")?;
            assert_eq!(
                (frame.pts, frame.width(), frame.height()),
                (Some(picture + 2), 16, 512)
            );
            for (component, plane) in frame.to_cpu()?.planes().iter().enumerate() {
                let band = if component == 0 { 256 } else { 64 };
                for (row, bytes) in plane.data().chunks_exact(band).enumerate() {
                    assert!(
                        bytes
                            .iter()
                            .all(|byte| *byte == 37 + picture as u8 * 5 + row as u8)
                    );
                }
            }
        }
        assert!(decoder.receive_frame()?.is_none());
        if retired {
            zero_lag = true;
            break;
        }
    }
    assert!(zero_lag, "no active continuation reduced lag to zero");
    Ok(())
}

#[test]
fn later_envelope_refusal_keeps_old_outputs_and_never_emits_the_refused_picture() -> Result<()> {
    if std::thread::available_parallelism().map_or(1, usize::from) < 2 {
        return Ok(());
    }
    let tiny = pcm::pcm_fixture(8, 8, false)?;
    let first = access_unit(tiny.sets.iter().chain([&tiny.nal]), false);
    let next = access_unit([&tiny.nal], false);
    let tall = raster::stream_geometry(false, 512, 16)?;
    let grown = access_unit(&tall[..515], false);
    let bytes = 16 * 8192 * 3 / 2;
    let mut refused = false;
    for memory in (12u64 << 20..32u64 << 20).step_by(64 << 10) {
        let limits = Limits {
            max_packet_bytes: grown.len() as u64,
            max_pending_bytes: bytes + 192,
            max_memory_bytes: memory,
            ..Limits::default()
        };
        let mut decoder = native_with_limits_for_test(&limits, 2)?;
        decoder.send_packet(&packet(&first, Some(0), false))?;
        decoder.send_packet(&packet(&next, Some(1), false))?;
        if !decoder
            .decoder
            .as_ref()
            .is_some_and(PictureDecoder::has_native_rows)
        {
            continue;
        }
        assert!(decoder.receive_frame()?.is_none());
        decoder.send_packet(&packet(&access_unit([&tall[1]], false), None, false))?;
        decoder.send_packet(&packet(&access_unit([&tiny.sets[1]], false), None, false))?;
        let Err(error) = decoder.send_packet(&packet(&grown, Some(2), false)) else {
            continue;
        };
        assert_eq!(error.kind, ErrorKind::Limit);
        assert!(error.message.contains("row_admission"), "{error}");
        for pts in [0, 1] {
            let frame = decoder
                .receive_frame()?
                .ok_or("lost pre-admission output")?;
            assert_eq!(
                (frame.pts, frame.width(), frame.height()),
                (Some(pts), 8, 8)
            );
            for (plane, samples) in frame.to_cpu()?.planes().iter().zip(&tiny.planes) {
                assert_eq!(
                    plane.data(),
                    samples.iter().map(|n| *n as u8).collect::<Vec<_>>()
                );
            }
        }
        assert_eq!(decoder.receive_frame().unwrap_err(), error);
        assert_eq!(decoder.send_eof().unwrap_err(), error);
        let mut serial = native_with_limits_for_test(&limits, 1)?;
        serial.send_packet(&packet(&first, Some(0), false))?;
        serial.send_packet(&packet(&next, Some(1), false))?;
        serial.send_packet(&packet(&grown, Some(2), false))?;
        serial.send_eof()?;
        for pts in 0..3 {
            assert_eq!(
                serial
                    .receive_frame()?
                    .ok_or("missing serial continuation")?
                    .pts,
                Some(pts)
            );
        }
        assert!(serial.receive_frame()?.is_none());
        let mut without_debt = native_with_limits_for_test(&limits, 2)?;
        without_debt.send_packet(&packet(&first, Some(0), false))?;
        without_debt.send_packet(&packet(&next, Some(1), false))?;
        without_debt.send_packet(&packet(&grown, Some(2), false))?;
        without_debt.send_eof()?;
        for pts in 0..3 {
            assert_eq!(
                without_debt
                    .receive_frame()?
                    .ok_or("missing zero-debt continuation")?
                    .pts,
                Some(pts)
            );
        }
        assert!(without_debt.receive_frame()?.is_none());
        refused = true;
        break;
    }
    assert!(
        refused,
        "no later continuation exceeded the admitted envelope"
    );
    Ok(())
}

#[test]
fn terminal_handoff_moves_only_packed_frames_after_decoder_teardown() -> Result<()> {
    let fixture = pcm::pcm_fixture(10, 10, false)?;
    let data = access_unit(fixture.sets.iter().chain([&fixture.nal]), false);
    let mut source = NativeHevc::new().open(&StreamConfig::new(Codec::Hevc), &Limits::default())?;
    source.send_packet(&packet(&data, Some(51), false))?;
    source.send_eof()?;
    let frame = source.receive_frame()?.ok_or("missing packed frame")?;
    let pixels = frame.to_cpu()?;
    let memory = pixels.planes().iter().map(|plane| plane.data().len()).sum();
    let first_sample = pixels.planes()[0].data().as_ptr();
    let mut output = Output::new(
        Limits::default(),
        Limits::default().deadline(),
        frame.color,
        0,
    )?;
    output
        .ready
        .push_back(coreav_decode::native_hevc_private::OutputFrame {
            frame,
            bytes: 192,
            memory,
        });
    let entries = output.ready.as_slices().0.as_ptr();
    let mut decoder = NativeDecoder {
        decoder: Some(PictureDecoder::new(DecodeLimits::default())),
        info: source.info().clone(),
        framing: Framing::AnnexB,
        output,
        state: State::Active,
        next_origin: 0,
    };
    let error = Error::new(ErrorKind::Timeout, "terminal timeout");
    assert_eq!(
        decoder.terminal_result::<()>(Err(error.clone())),
        Err(error.clone())
    );
    assert!(decoder.decoder.is_none());
    let mut terminal = decoder
        .take_native_hevc_terminal_output()
        .ok_or("no handoff")?;
    assert_eq!(terminal.frames.as_slices().0.as_ptr(), entries);
    assert!(decoder.output.is_empty());
    drop(decoder);
    assert_eq!(terminal.frames[0].bytes, 192);
    assert_eq!(terminal.frames[0].memory, memory);
    let frame = terminal.receive_frame()?.ok_or("lost packed frame")?;
    assert_eq!(frame.to_cpu()?.planes()[0].data().as_ptr(), first_sample);
    assert_eq!(frame.pts, Some(51));
    assert_eq!(terminal.receive_frame().unwrap_err(), error);
    Ok(())
}

#[test]
fn lowered_resource_limits_cannot_be_bypassed_by_direct_backend_open() -> Result<()> {
    let fixture = pcm::pcm_fixture(8, 8, false)?;
    let data = access_unit(fixture.sets.iter().chain([&fixture.nal]), false);
    for limits in [
        Limits {
            max_pixels: 63,
            ..Limits::default()
        },
        Limits {
            max_reference_frames: 0,
            ..Limits::default()
        },
        Limits {
            max_frame_bytes: 95,
            ..Limits::default()
        },
        Limits {
            max_decoded_bytes: 95,
            ..Limits::default()
        },
        Limits {
            max_pending_bytes: 95,
            ..Limits::default()
        },
        Limits {
            max_packet_bytes: data.len() as u64 - 1,
            ..Limits::default()
        },
        Limits {
            max_memory_bytes: 0,
            ..Limits::default()
        },
    ] {
        let opened = NativeHevc::new().open(&StreamConfig::new(Codec::Hevc), &limits);
        let mut decoder = match opened {
            Ok(decoder) => decoder,
            Err(error) => {
                assert_eq!(error.kind, ErrorKind::Limit);
                continue;
            }
        };
        let result = decoder
            .send_packet(&packet(&data, None, false))
            .and_then(|()| decoder.send_eof());
        let error = result.err().ok_or("lowered limit did not fail")?;
        assert_eq!(error.kind, ErrorKind::Limit);
        terminal(&mut *decoder);
    }
    Ok(())
}

#[test]
fn zero_session_deadline_is_typed_timeout() {
    let limits = Limits {
        timeout: std::time::Duration::ZERO,
        ..Limits::default()
    };
    let opened = NativeHevc::new().open(&StreamConfig::new(Codec::Hevc), &limits);
    assert!(matches!(
        opened,
        Err(Error {
            kind: ErrorKind::Timeout,
            ..
        })
    ));
    assert_eq!(
        codec_error(coreav_hevc::Error::Limit("decode_deadline")).kind,
        ErrorKind::Timeout
    );
}

#[test]
fn crop_and_colour_apply_once_and_keep_absent_fields() -> Result<()> {
    let fixture = pcm::pcm_fixture(10, 10, false)?;
    let mut decoder = PictureDecoder::new(DecodeLimits::default());
    let container = ColorInfo {
        primaries: Some(1),
        transfer: Some(16),
        matrix: Some(1),
        full_range: Some(true),
        ..ColorInfo::default()
    };
    let mut output = Output::new(
        Limits::default(),
        Limits::default().deadline(),
        container,
        0,
    )?;
    decoder.set_external_memory(output.retained_bytes()?)?;
    for set in &fixture.sets {
        decoder.read_nal(set, None, |_| Ok(()))?;
    }
    decoder.read_nal_with_origin(&fixture.nal, Some(99), 12, |_, _| Ok(()))?;
    decoder.finish_with_output(|mut picture, memory| {
        picture.info.visible_width = 4;
        picture.info.visible_height = 4;
        picture.info.conformance_window = [1; 4];
        picture.info.vui.colour_primaries = Some(9);
        picture.info.vui.full_range = Some(false);
        picture.info.vui.chroma_location = Some((2, 2));
        output
            .push(picture, memory)
            .map_err(|_| coreav_hevc::Error::InvalidData("test_output"))
    })?;
    let frame = output.pop().ok_or("missing cropped frame")?;
    assert_eq!(frame.pts, Some(99));
    assert_eq!(
        frame.color,
        ColorInfo {
            primaries: Some(9),
            transfer: Some(16),
            matrix: Some(1),
            full_range: Some(false),
            chroma_location: Some(coreav_frame::ChromaLocation::TopLeft)
        }
    );
    let Pixels::Cpu(picture) = frame.pixels else {
        return Err("not CPU output".into());
    };
    assert_eq!((picture.width(), picture.height()), (4, 4));
    for (component, (plane, original)) in picture.planes().iter().zip(&fixture.planes).enumerate() {
        let (side, offset, visible) = if component == 0 { (8, 2, 4) } else { (4, 1, 2) };
        let mut expected = Vec::new();
        for row in offset..offset + visible {
            let values = original
                .get(row * side + offset..row * side + offset + visible)
                .ok_or("short original plane")?;
            expected.extend(values.iter().flat_map(|value| value.to_le_bytes()));
        }
        assert_eq!(plane.data(), expected);
    }
    Ok(())
}

#[test]
fn invalid_configuration_and_other_codecs_are_never_admitted() {
    assert!(matches!(
        NativeHevc::new().supports(&StreamConfig::new(Codec::H264), &StreamTraits::default()),
        Support::No(_)
    ));
    for data in [vec![], vec![1; 22], vec![0; 23]] {
        let mut stream = StreamConfig::new(Codec::Hevc);
        stream.config = Some(CodecConfig {
            kind: ConfigKind::Hvcc,
            data,
        });
        assert!(NativeHevc::new().open(&stream, &Limits::default()).is_err());
    }
    let admitted = |tail: &[u8], length_size: u8| {
        let mut data = vec![0; 22];
        if let Some(first) = data.first_mut() {
            *first = 1;
        }
        if let Some(size) = data.get_mut(21) {
            *size = 0xfc | length_size;
        }
        data.extend_from_slice(tail);
        let mut stream = StreamConfig::new(Codec::Hevc);
        stream.config = Some(CodecConfig {
            kind: ConfigKind::Hvcc,
            data,
        });
        matches!(
            NativeHevc::new().supports(&stream, &StreamTraits::default()),
            Support::Yes
        )
    };
    assert!(admitted(&[0], 3));
    assert!(admitted(&[1, 0x20, 0, 1, 0, 2, 0x40, 1], 3));
    assert!(admitted(
        &[2, 0x20, 0, 1, 0, 2, 0x40, 1, 0x21, 0, 1, 0, 2, 0x42, 1],
        0
    ));
    assert!(
        !admitted(&[0], 2),
        "length size three is not an hvcC encoding"
    );
    assert!(!admitted(&[65], 3), "more than 64 arrays");
    assert!(!admitted(&[0, 0], 3), "trailing bytes");
    assert!(
        !admitted(&[2, 0x20, 0, 0, 0x20, 0, 0], 3),
        "duplicate array"
    );
    assert!(!admitted(&[1, 0x60, 0, 0], 3), "reserved array bit");
    assert!(
        !admitted(&[1, 0x21, 0, 1, 0, 2, 0x40, 1], 3),
        "array type mismatch"
    );
    assert!(
        !admitted(&[1, 0x01, 0, 1, 0, 2, 0x02, 1], 3),
        "slice data in hvcC"
    );
    assert!(
        !admitted(&[1, 0x20, 0, 2, 0, 2, 0x40, 1], 3),
        "truncated array"
    );
    let mut many = vec![1, 0x20, 0x10, 0x01];
    for _ in 0..4097 {
        many.extend_from_slice(&[0, 2, 0x40, 1]);
    }
    assert!(!admitted(&many, 3), "more than 4096 configuration NALs");
}

#[test]
fn selected_native_errors_never_open_a_fallback() -> Result<()> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Spy(AtomicUsize);
    impl Backend for Spy {
        fn kind(&self) -> BackendKind {
            BackendKind::Fallback
        }
        fn supports(&self, _: &StreamConfig, _: &StreamTraits) -> Support {
            Support::Yes
        }
        fn open(
            &self,
            _: &StreamConfig,
            _: &Limits,
        ) -> std::result::Result<Box<dyn Decoder>, Error> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Err(invalid("fallback spy was opened"))
        }
    }
    let spy = Spy(AtomicUsize::new(0));
    let native = NativeHevc::new();
    let mut opened = coreav_decode::open(
        &StreamConfig::new(Codec::Hevc),
        &Policy::default(),
        &Limits::default(),
        &[&native, &spy],
    )?;
    assert!(
        opened
            .decoder
            .send_packet(&packet(&[0, 0, 1, 0, 0], None, false))
            .is_err()
    );
    terminal(&mut *opened.decoder);
    assert_eq!(spy.0.load(Ordering::Relaxed), 0);
    Ok(())
}
