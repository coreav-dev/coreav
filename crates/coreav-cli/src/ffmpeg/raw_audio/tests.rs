use super::*;

#[test]
fn opus_admission_uses_actual_config_not_the_informational_input_rate() {
    let mut report = Report::default();
    for channels in [1u8, 2] {
        for input_rate in [0u32, 8000, 44_100, 48_000, 192_000] {
            let mut config = vec![0, channels, 1, 56];
            config.extend_from_slice(&input_rate.to_be_bytes());
            config.extend_from_slice(&[0, 0, 0]);
            report.setups = vec![crate::ffprobe::Setup {
                config: Some(config.clone()),
                ..Default::default()
            }];
            assert!(opus_mapping0(&report, 0, u16::from(channels), 48_000));
            assert!(!opus_mapping0(&report, 0, u16::from(channels), 44_100));
            assert!(!opus_mapping0(&report, 1, u16::from(channels), 48_000));
            for (field, value) in [(0, 1), (1, 3), (8, 255), (9, 1), (10, 1), (10, 255)] {
                let mut changed = config.clone();
                if let Some(byte) = changed.get_mut(field) {
                    *byte = value;
                }
                report.setups = vec![crate::ffprobe::Setup {
                    config: Some(changed),
                    ..Default::default()
                }];
                assert!(!opus_mapping0(&report, 0, u16::from(channels), 48_000));
            }
            for family in [1, 255] {
                let mut mapped = config.clone();
                if let Some(value) = mapped.last_mut() {
                    *value = family;
                }
                mapped.extend_from_slice(if channels == 1 {
                    &[1, 0, 0]
                } else {
                    &[1, 1, 0, 1]
                });
                assert!(coreav_audio::OpusHead::from_config(&mapped).is_ok());
                report.setups = vec![crate::ffprobe::Setup {
                    config: Some(mapped),
                    ..Default::default()
                }];
                assert!(!opus_mapping0(&report, 0, u16::from(channels), 48_000));
            }
        }
    }
    report.setups.clear();
    assert!(!opus_mapping0(&report, 0, 1, 48_000));
}

#[test]
fn probe_refusal_preserves_malformed_io_and_limit_failures() {
    use crate::error::Failure;
    assert!(matches!(
        super::super::probe_failure(Failure::Unsupported("WAVE codec".to_owned())),
        Stop::Refused(Refusal::NotNative(_))
    ));
    for failure in [
        Failure::Invalid("broken WAVE header"),
        Failure::Io(std::io::ErrorKind::PermissionDenied),
        Failure::Library(coreav_isobmff::Error::LimitExceeded {
            limit: coreav_isobmff::LimitKind::SampleWork,
            offset: 44,
        }),
    ] {
        assert!(matches!(
            super::super::probe_failure(failure),
            Stop::Failed(_)
        ));
    }
}

#[test]
fn mpeg1_admission_checks_every_complete_frame() {
    let mut frame = vec![0; 417];
    if let Some(header) = frame.get_mut(..4) {
        header.copy_from_slice(&[0xff, 0xfb, 0x90, 0x64]);
    }
    assert!(mpeg1_frames(&frame));
    let mut pair = frame.clone();
    pair.extend_from_slice(&frame);
    assert!(mpeg1_frames(&pair));
    pair.extend_from_slice(&[0xff, 0xf3, 0x80, 0x64]);
    assert!(!mpeg1_frames(&pair));
    frame.pop();
    assert!(!mpeg1_frames(&frame));
    assert!(!mpeg1_frames(&[]));
    assert!(!mpeg1_frames(&[0xff, 0xfb, 0, 0]));
}

#[test]
fn thread_counts_outside_the_native_limit_are_fallback_not_usage() {
    for count in ["17", "32", "4294967295"] {
        let args = [
            "-threads",
            count,
            "-i",
            "missing.mka",
            "-f",
            "f32le",
            "pipe:",
        ]
        .map(str::to_owned);
        let command = args::parse(&args);
        assert!(matches!(
            command.and_then(|command| command.decoder_threads()),
            Err(Refusal::NotNative(_))
        ));
    }
}
