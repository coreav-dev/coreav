//! Settings and retained-session resource boundary regressions.

use coreav_backend_svt_av1::model::{Color, RETAINED_BYTES, Retention, Settings};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn settings(
    width: u32,
    height: u32,
    rate: (u32, u32),
) -> Result<Settings, coreav_backend_svt_av1::Error> {
    Settings::new(width, height, rate, 8, 32, Color::default())
}

#[test]
fn rational_rate_is_not_rounded_and_keyint_has_a_one_picture_floor() -> TestResult {
    let fractional = settings(1920, 1080, (30_000, 1001))?;
    assert_eq!(fractional.rate(), (30_000, 1001));
    assert_eq!(fractional.keyint(), 149);
    assert_eq!(settings(64, 64, (1, 100))?.keyint(), 1);
    assert!(Settings::new(64, 64, (u32::MAX, 1), 8, 32, Color::default()).is_err());
    assert!(Settings::new(64, 64, (0, 1), 8, 32, Color::default()).is_err());
    Ok(())
}

#[test]
fn exact_retention_boundaries_include_headers_eof_and_slots() -> TestResult {
    let s = settings(64, 64, (30, 1))?;
    let native = 176;
    let slot = 40;
    let owner = 128;
    let fixed = native + owner;
    let picture = s.raw_bytes() + native + slot;
    assert!(Retention::new(s, (fixed + picture - 1) as u64, native, slot, owner).is_err());
    for count in [1, 2, 3, 149] {
        for residue in [0, picture - 1] {
            let budget = (fixed + count * picture + residue) as u64;
            let plan = Retention::new(s, budget, native, slot, owner)?;
            assert_eq!(plan.pictures(), count as u32);
            assert_eq!(plan.bytes(), (fixed + count * picture) as u64);
            assert!(plan.bytes() <= plan.budget());
        }
    }
    let capped = Retention::new(s, u64::MAX, native, slot, owner)?;
    assert_eq!(capped.pictures(), 150);
    assert_eq!(capped.budget(), RETAINED_BYTES);
    assert!(Retention::new(s, 0, native, slot, owner).is_err());
    assert!(Retention::new(s, u64::MAX, usize::MAX, slot, owner).is_err());
    assert!(Retention::new(s, u64::MAX, native, usize::MAX, owner).is_err());
    assert!(Retention::new(s, u64::MAX, 0, slot, owner).is_err());
    Ok(())
}

#[test]
fn exact_peak_includes_every_reserved_slot_for_empty_short_and_full_sessions() -> TestResult {
    let s = settings(64, 64, (30, 1))?;
    let plan = Retention::new(s, 12384, 16, 16, 16)?;
    assert_eq!(plan.pictures(), 2);
    assert_eq!(plan.fixed_bytes(), 32);
    assert_eq!(plan.picture_bytes(), 6176);
    assert_eq!(plan.peak_bytes(0)?, 64);
    assert_eq!(plan.peak_bytes(1)?, 6224);
    assert_eq!(plan.peak_bytes(2)?, 12384);
    assert_eq!(plan.peak_bytes(2)?, plan.bytes());
    assert!(plan.peak_bytes(3).is_err());
    assert!(plan.peak_bytes(u32::MAX).is_err());
    let single = Retention::new(s, 6208, 16, 16, 16)?;
    assert_eq!(single.pictures(), 1);
    assert_eq!(single.peak_bytes(0)?, 48);
    assert_eq!(single.peak_bytes(1)?, 6208);
    Ok(())
}

#[test]
fn memory_budget_shortens_large_frames_not_whole_jobs() -> TestResult {
    let hd = Retention::new(settings(1920, 1080, (30, 1))?, RETAINED_BYTES, 176, 40, 128)?;
    let uhd = Retention::new(settings(3840, 2160, (30, 1))?, RETAINED_BYTES, 176, 40, 128)?;
    assert_eq!(hd.pictures(), 150);
    assert_eq!(uhd.pictures(), 86);
    assert_eq!(
        settings(1920, 1080, (30, 1))?.raw_bytes() * 150,
        466_560_000
    );
    assert!(uhd.bytes() <= RETAINED_BYTES);
    Ok(())
}

#[test]
fn unsupported_geometry_colour_and_rate_controls_are_refused() {
    for (width, height) in [(63, 64), (64, 63), (65, 64), (16_386, 64), (64, 8706)] {
        assert!(Settings::new(width, height, (30, 1), 8, 32, Color::default()).is_err());
    }
    assert!(Settings::new(64, 64, (30, 1), 14, 32, Color::default()).is_err());
    assert!(Settings::new(64, 64, (30, 1), 8, 64, Color::default()).is_err());
    for (p, t, m, c) in [
        (3, 1, 1, 0),
        (1, 16, 1, 0),
        (1, 18, 1, 0),
        (1, 1, 0, 0),
        (2, 1, 12, 0),
        (1, 1, 1, 3),
    ] {
        assert!(Color::new(p, t, m, false, c).is_err());
    }
    assert!(Color::new(1, 1, 1, true, 2).is_ok());
}
