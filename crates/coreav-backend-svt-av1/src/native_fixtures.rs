//! Isolated native-case entry for the registered correctness driver.

use std::{
    env,
    fs::{self, File},
    io::{Cursor, Read, Write},
    path::Path,
    sync::atomic::Ordering,
};

use super::{
    HEADER_DROPS, LIVE_SESSIONS, OPEN_ATTEMPTS, PEAK_SESSIONS, PLANE_DROPS, RETAINED_SETS, Session,
};
use crate::{
    Error, frame, model,
    output::{self, Output, Sample},
    protocol::{Decoder, InputReader, Record, Request},
    sequence::{ChromaPosition, ColorRange},
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
const INJECTED_STATUS: u32 = 0x8000_1000;

#[test]
fn registered_native_case() -> TestResult {
    let Ok(mode) = env::var("COREAV_SVT_CASE_MODE") else {
        return Ok(());
    };
    let outcome = env::var("COREAV_SVT_CASE_OUTCOME")?;
    let expected_frames: u64 = env::var("COREAV_SVT_CASE_FRAMES")?.parse()?;
    let expected_capacity: u32 = env::var("COREAV_SVT_CASE_CAPACITY")?.parse()?;
    let directory = env::var_os("COREAV_SVT_CASE_DIRECTORY").ok_or("missing case directory")?;
    let directory = Path::new(&directory);
    fs::create_dir_all(directory)?;
    let input = std::io::stdin();
    let mut input = input.lock();
    let request = Request::read(&mut input)?;
    let mut prefix = Vec::new();
    request.write(&mut prefix)?;
    let input = Cursor::new(prefix).chain(input);
    let plan = crate::worker::retention(request);
    if outcome == "retained-limit" {
        assert_eq!(expected_capacity, 0);
        assert!(matches!(plan, Err(Error::Limit(_))));
    } else {
        let plan = plan?;
        assert_eq!(plan.pictures(), expected_capacity);
        for count in 1..=3.min(request.settings.keyint()) {
            let boundary = plan.fixed_bytes() + u64::from(count) * plan.picture_bytes();
            for (budget, expected) in [
                (boundary - 1, count - 1),
                (boundary, count),
                (boundary + 1, count),
            ] {
                let measured = super::retention(request.settings, budget);
                if expected == 0 {
                    assert!(matches!(measured, Err(Error::Limit(_))));
                } else {
                    assert_eq!(measured?.pictures(), expected);
                }
            }
        }
        assert_eq!(OPEN_ATTEMPTS.load(Ordering::SeqCst), 0);
        fs::write(
            directory.join("layout.json"),
            format!(
                "{{\"B\":{},\"F\":{},\"P\":{},\"S\":{},\"K\":{},\"slot\":{}}}\n",
                plan.budget(),
                plan.fixed_bytes(),
                plan.picture_bytes(),
                plan.pictures(),
                request.settings.keyint(),
                size_of::<super::Input>()
            ),
        )?;
    }
    match mode.as_str() {
        "encode" => {
            let wire = directory.join("wire.bin");
            let mut file = File::create(&wire)?;
            let result = crate::worker::encode(input, &mut file);
            file.flush()?;
            match outcome.as_str() {
                "complete" => {
                    result?;
                    validate(request, &wire, directory, expected_frames)?;
                }
                "empty-input" | "retained-limit" => {
                    assert!(matches!(
                        (&result, outcome.as_str()),
                        (Err(Error::Output(_)), "empty-input")
                            | (Err(Error::Limit(_)), "retained-limit")
                    ));
                    assert_eq!(OPEN_ATTEMPTS.load(Ordering::SeqCst), 0);
                    assert_eq!(HEADER_DROPS.load(Ordering::SeqCst), 0);
                    assert_eq!(PLANE_DROPS.load(Ordering::SeqCst), 0);
                    assert_eq!(PEAK_SESSIONS.load(Ordering::SeqCst), 0);
                    fs::write(
                        directory.join("complete.json"),
                        format!("{{\"outcome\":\"{outcome}\",\"open_attempts\":0,\"frames\":0}}\n"),
                    )?;
                }
                _ => return Err("unexpected encode outcome".into()),
            }
        }
        "abandon-input"
        | "abandon-output"
        | "admission-cap"
        | "inject-picture-error"
        | "inject-eof-error"
        | "inject-drain-error"
        | "inject-destroy-error"
        | "deadline-control" => {
            lifecycle(input, request, directory, &mode, &outcome, expected_frames)?;
        }
        _ => return Err("unknown registered native mode".into()),
    }
    Ok(())
}

fn lifecycle(
    input: impl Read,
    request: Request,
    directory: &Path,
    mode: &str,
    outcome: &str,
    expected_frames: u64,
) -> TestResult {
    let mut input = InputReader::new(input)?;
    let mut session = Session::new(request.settings, request.limits.retained_bytes())?;
    let mut submitted = 0;
    let mut calls = 0;
    while input.next_frame_header()?.is_some() {
        let picture = input.read_picture()?;
        if mode == "inject-picture-error" {
            let result = session.submit_with(picture, |_, _| {
                calls += 1;
                INJECTED_STATUS
            });
            assert!(matches!(result, Err(Error::Native(INJECTED_STATUS))));
        } else {
            session.submit(picture)?;
        }
        submitted += 1;
    }
    assert_eq!(submitted, expected_frames);
    assert_eq!(OPEN_ATTEMPTS.load(Ordering::SeqCst), 1);
    assert_eq!(HEADER_DROPS.load(Ordering::SeqCst), 0);
    assert_eq!(PLANE_DROPS.load(Ordering::SeqCst), 0);
    assert_eq!(RETAINED_SETS.load(Ordering::SeqCst), 1);
    let peak = session.retained_bytes()?;
    match mode {
        "deadline-control" => {
            assert_eq!(outcome, "deadline-terminated");
            let pid = std::process::id();
            fs::write(
                directory.join("retained-before-deadline.json"),
                format!(
                    "{{\"pid\":{pid},\"frames\":{submitted},\"peak\":{peak},\"header_drops\":0,\"plane_drops\":0,\"live_sessions\":1}}\n"
                ),
            )?;
            loop {
                std::thread::park();
            }
        }
        "abandon-input" => {}
        "inject-picture-error" => assert_eq!(session.submitted(), 1),
        "admission-cap" => {
            assert_eq!(session.submitted(), session.plan().pictures());
            assert!(matches!(session.submit(Vec::new()), Err(Error::Limit(_))));
            assert_eq!(session.submitted(), session.plan().pictures());
        }
        "abandon-output" => {
            session.end_input()?;
            let packet = session
                .receive(model::MAX_PACKET_BYTES)?
                .ok_or("drain returned no first packet")?;
            assert!(!packet.data.is_empty());
            assert!(
                !session.ended(),
                "the abandonment point must precede native EOS"
            );
            fs::write(directory.join("partial.obu"), &packet.data)?;
        }
        "inject-eof-error" => {
            let result = session.end_input_with(|_, _| {
                calls += 1;
                INJECTED_STATUS
            });
            assert!(matches!(result, Err(Error::Native(INJECTED_STATUS))));
        }
        "inject-drain-error" => {
            session.end_input()?;
            let result = session.receive_with(model::MAX_PACKET_BYTES, |_, _, _| {
                calls += 1;
                INJECTED_STATUS
            });
            assert!(matches!(result, Err(Error::Native(INJECTED_STATUS))));
        }
        "inject-destroy-error" => {
            let result = session.close_with(|_| INJECTED_STATUS);
            assert!(matches!(result, Err(Error::Native(INJECTED_STATUS))));
            assert_eq!(HEADER_DROPS.load(Ordering::SeqCst), 0);
            assert_eq!(PLANE_DROPS.load(Ordering::SeqCst), 0);
            assert_eq!(LIVE_SESSIONS.load(Ordering::SeqCst), 1);
            assert_eq!(RETAINED_SETS.load(Ordering::SeqCst), 1);
            assert_eq!(PEAK_SESSIONS.load(Ordering::SeqCst), 1);
            assert_eq!(outcome, "retained-until-exit");
            fs::write(
                directory.join("complete.json"),
                format!(
                    "{{\"outcome\":\"retained-until-exit\",\"frames\":{submitted},\"peak\":{peak},\"header_drops\":0,\"plane_drops\":0,\"peak_sessions\":1}}\n"
                ),
            )?;
            return Ok(());
        }
        _ => return Err("unknown lifecycle mode".into()),
    }
    if mode.starts_with("inject-") {
        let result = session.submit_with(vec![0; request.settings.raw_bytes()], |_, _| {
            calls += 1;
            INJECTED_STATUS
        });
        assert!(matches!(result, Err(Error::Invalid(_))));
        let result = session.end_input_with(|_, _| {
            calls += 1;
            INJECTED_STATUS
        });
        assert!(matches!(result, Err(Error::Invalid(_))));
        let result = session.receive_with(model::MAX_PACKET_BYTES, |_, _, _| {
            calls += 1;
            INJECTED_STATUS
        });
        assert!(matches!(result, Err(Error::Invalid(_))));
        assert_eq!(calls, 1);
        assert_eq!(outcome, "terminal-native-error");
    } else {
        assert_eq!(outcome, "destroyed");
    }
    assert_eq!(session.retained_bytes()?, peak);
    assert_eq!(HEADER_DROPS.load(Ordering::SeqCst), 0);
    assert_eq!(PLANE_DROPS.load(Ordering::SeqCst), 0);
    session.destroy()?;
    let headers = HEADER_DROPS.load(Ordering::SeqCst);
    let planes = PLANE_DROPS.load(Ordering::SeqCst);
    assert_eq!(headers as u64, submitted + 1);
    assert_eq!(planes as u64, submitted);
    assert_eq!(LIVE_SESSIONS.load(Ordering::SeqCst), 0);
    assert_eq!(RETAINED_SETS.load(Ordering::SeqCst), 0);
    assert_eq!(PEAK_SESSIONS.load(Ordering::SeqCst), 1);
    fs::write(
        directory.join("complete.json"),
        format!(
            "{{\"outcome\":\"{outcome}\",\"frames\":{submitted},\"peak\":{peak},\"header_drops\":{headers},\"plane_drops\":{planes},\"peak_sessions\":1}}\n"
        ),
    )?;
    Ok(())
}

fn sample(sample: Sample, count: &mut u64, metadata: &mut File) -> TestResult {
    assert_eq!(sample.ordinal, *count);
    writeln!(
        metadata,
        "{{\"ordinal\":{},\"sync\":{},\"coded_identity\":{},\"show_existing\":{},\"bytes\":{}}}",
        sample.ordinal,
        sample.sync,
        sample.picture_identity,
        sample.show_existing,
        sample.bytes.len()
    )?;
    *count += 1;
    Ok(())
}

fn validate(request: Request, wire: &Path, directory: &Path, expected_frames: u64) -> TestResult {
    let retention = crate::worker::retention(request)?;
    let color = request.settings.color();
    let expected = output::Expected {
        dimensions: frame::Expected {
            width: request.settings.width(),
            height: request.settings.height(),
        },
        primaries: color.primaries(),
        transfer: color.transfer(),
        matrix: color.matrix(),
        range: if color.full_range() {
            ColorRange::Full
        } else {
            ColorRange::Studio
        },
        chroma_position: match color.chroma_position() {
            0 => ChromaPosition::Unknown,
            1 => ChromaPosition::Vertical,
            2 => ChromaPosition::Colocated,
            _ => return Err("invalid checked chroma position".into()),
        },
    };
    let limit = model::MAX_PACKET_BYTES;
    let mut output = Output::new(
        expected,
        output::Limits {
            max_packet_bytes: limit,
            max_buffered_bytes: limit * 2,
            max_obu_bytes: limit,
            max_temporal_unit_bytes: limit,
            max_sequence_bytes: 4096,
            max_total_bytes: request.limits.output_bytes(),
            max_obus: request.limits.frames() * 32 + 64,
            max_frames: request.limits.frames() * 4 + 32,
            max_frames_per_temporal_unit: 16,
            max_samples: request.limits.frames(),
            max_tiles: 4096,
        },
    );
    let mut decoder = Decoder::new(request, retention)?;
    let mut wire = File::open(wire)?;
    let mut combined = File::create(directory.join("combined.obu"))?;
    let mut metadata = File::create(directory.join("samples.ndjson"))?;
    let mut segment: Option<File> = None;
    let mut samples = 0;
    let mut segments = 0;
    let mut packets = 0;
    let mut peak = 0;
    let mut completed = false;
    let mut native_pts = Vec::new();
    let mut buffer = [0; 65536];
    loop {
        let length = wire.read(&mut buffer)?;
        if length == 0 {
            break;
        }
        decoder.push(
            buffer.get(..length).ok_or("invalid read extent")?,
            |record| {
                let result: TestResult = (|| {
                    match record {
                        Record::Version(_) => {}
                        Record::Begin { base } => {
                            assert_eq!(base, samples);
                            output.begin_segment(u64::from(retention.pictures()))?;
                            segment = Some(File::create(
                                directory.join(format!("segment-{segments}.obu")),
                            )?);
                        }
                        Record::Packet { data, pts, .. } => {
                            packets += 1;
                            if !data.is_empty() {
                                native_pts.try_reserve(1)?;
                                native_pts.push(pts);
                            }
                            combined.write_all(&data)?;
                            segment
                                .as_mut()
                                .ok_or("missing segment output")?
                                .write_all(&data)?;
                            output.push(&data)?;
                            while let Some(next) = output.next_sample()? {
                                sample(next, &mut samples, &mut metadata)?;
                            }
                        }
                        Record::End {
                            submitted,
                            peak_retained_bytes,
                            ..
                        } => {
                            assert!(
                                native_pts
                                    .iter()
                                    .all(|pts| *pts >= 0 && *pts < i64::from(submitted))
                            );
                            native_pts.clear();
                            sample(
                                output.end_segment(u64::from(submitted))?,
                                &mut samples,
                                &mut metadata,
                            )?;
                            segment.take().ok_or("missing ended segment")?.flush()?;
                            segments += 1;
                            peak = peak.max(peak_retained_bytes);
                        }
                        Record::Complete {
                            frames,
                            segments: count,
                        } => {
                            assert_eq!(frames, expected_frames);
                            assert_eq!(samples, frames);
                            assert_eq!(segments, count);
                            completed = true;
                        }
                    }
                    Ok(())
                })();
                result.map_err(|error| {
                    eprintln!("native fixture validation: {error}");
                    Error::Output("native fixture output validation failed")
                })
            },
        )?;
    }
    decoder.finish()?;
    assert!(completed);
    combined.flush()?;
    metadata.flush()?;
    let mut config = File::create(directory.join("configuration.av1c"))?;
    config.write_all(
        &output
            .sequence()
            .ok_or("missing sequence")?
            .av1c_header()
            .bytes(),
    )?;
    config.write_all(output.configuration().ok_or("missing configuration OBU")?)?;
    config.flush()?;
    let headers = HEADER_DROPS.load(Ordering::SeqCst);
    let planes = PLANE_DROPS.load(Ordering::SeqCst);
    let opens = OPEN_ATTEMPTS.load(Ordering::SeqCst);
    assert_eq!(headers as u64, expected_frames + segments);
    assert_eq!(planes as u64, expected_frames);
    assert_eq!(opens as u64, segments);
    assert_eq!(RETAINED_SETS.load(Ordering::SeqCst), 0);
    assert_eq!(LIVE_SESSIONS.load(Ordering::SeqCst), 0);
    assert_eq!(PEAK_SESSIONS.load(Ordering::SeqCst), 1);
    fs::write(
        directory.join("complete.json"),
        format!(
            "{{\"outcome\":\"complete\",\"frames\":{samples},\"segments\":{segments},\"packets\":{packets},\"peak\":{peak},\"header_drops\":{headers},\"plane_drops\":{planes},\"open_attempts\":{opens},\"peak_sessions\":1}}\n"
        ),
    )?;
    Ok(())
}
