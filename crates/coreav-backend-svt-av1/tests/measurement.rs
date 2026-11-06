//! Measurement-only continuous-session retention boundaries.

#![cfg(feature = "measurement")]

use coreav_backend_svt_av1::{
    Error,
    model::{Color, Retention, Settings},
    protocol::{Decoder, Limits, Request},
    worker,
};

#[test]
fn continuous_retention_keeps_the_production_envelope_unchanged()
-> Result<(), Box<dyn std::error::Error>> {
    let settings = Settings::new(1920, 1080, (30, 1), 8, 32, Color::default())?;
    let normal = Retention::new(settings, 8 << 30, 200, 32, 136)?;
    assert_eq!(normal.budget(), 1 << 30);
    assert_eq!(normal.pictures(), settings.keyint());
    let continuous = Retention::measurement(settings, 8 << 30, 200, 32, 136)?;
    assert_eq!(continuous.budget(), 4 << 30);
    assert!(continuous.pictures() >= 900);
    assert!(continuous.peak_bytes(900)? <= continuous.budget());
    assert!(continuous.peak_bytes(continuous.pictures() + 1).is_err());
    assert!(Limits::new(4 << 30, 1 << 20, 900, 1 << 20).is_err());
    let request = Request {
        settings,
        limits: Limits::measurement(4 << 30, 1 << 20, 900, 1 << 20)?,
    };
    assert!(Decoder::new(request, continuous).is_err());
    assert!(Decoder::measurement(request, continuous).is_ok());
    Ok(())
}

#[test]
fn an_oversized_job_refuses_before_starting_a_session_or_writing_output()
-> Result<(), Box<dyn std::error::Error>> {
    let request = Request {
        settings: Settings::new(1920, 1080, (30, 1), 8, 32, Color::default())?,
        limits: Limits::measurement(4 << 30, 1 << 20, u64::MAX, 1 << 20)?,
    };
    let mut output = Vec::new();
    let mut requested_input = false;
    let result = worker::measurement_encode(request, &mut output, || {
        requested_input = true;
        Err(Error::Invalid("an oversized job must not request input"))
    });
    assert!(matches!(
        result,
        Err(Error::Limit(
            "measurement requires one continuous AV1 session"
        ))
    ));
    assert!(output.is_empty());
    assert!(!requested_input);
    Ok(())
}
