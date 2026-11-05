//! Synthetic HEVC headers, not compressed-picture decoding fixtures.

use coreav_backend_apple::{
    h264::{Dimensions, Limits},
    hevc::{AccessUnit, Config, Error, Stream},
};

#[derive(Default)]
struct Writer(Vec<bool>);

impl Writer {
    fn flag(&mut self, value: bool) {
        self.0.push(value);
    }

    fn u(&mut self, width: u32, value: u32) {
        for shift in (0..width).rev() {
            self.flag(value & (1 << shift) != 0);
        }
    }

    fn ue(&mut self, value: u32) {
        let coded = value + 1;
        let width = 32 - coded.leading_zeros();
        for _ in 1..width {
            self.flag(false);
        }
        self.u(width, coded);
    }

    fn padded(mut self) -> Vec<u8> {
        self.flag(true);
        while self.0.len() % 8 != 0 {
            self.flag(false);
        }
        self.0
            .chunks(8)
            .map(|bits| {
                bits.iter()
                    .enumerate()
                    .fold(0, |byte, (i, bit)| byte | (u8::from(*bit) << (7 - i)))
            })
            .collect()
    }

    fn profile(&mut self, profile: u32) {
        self.u(8, profile);
        self.u(32, 0);
        self.u(32, 0);
        self.u(16, 0);
        self.u(8, 120);
    }

    fn ordering(&mut self) {
        self.flag(false);
        self.ue(4);
        self.ue(0);
        self.ue(0);
    }
}

fn nal(kind: u8, rbsp: &[u8]) -> Vec<u8> {
    let mut bytes = vec![kind << 1, 1];
    let mut zeros = 0;
    for &byte in rbsp {
        if zeros == 2 && byte <= 3 {
            bytes.push(3);
            zeros = 0;
        }
        bytes.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
    bytes
}

fn sets(profile: u32, depth: u32, colour: bool) -> [Vec<u8>; 3] {
    let mut w = Writer::default();
    w.u(4, 0);
    w.flag(true);
    w.flag(true);
    w.u(6, 0);
    w.u(3, 0);
    w.flag(true);
    w.u(16, 0xffff);
    w.profile(1);
    w.ordering();
    w.u(6, 0);
    w.ue(0);
    w.flag(false);
    w.flag(false);
    let vps = nal(32, &w.padded());

    let mut w = Writer::default();
    w.u(4, 0);
    w.u(3, 0);
    w.flag(true);
    w.profile(profile);
    w.ue(0);
    w.ue(1);
    w.ue(128);
    w.ue(64);
    w.flag(false);
    w.ue(depth - 8);
    w.ue(depth - 8);
    w.ue(0);
    w.ordering();
    for value in [0, 3, 0, 3, 0, 0] {
        w.ue(value);
    }
    for _ in 0..4 {
        w.flag(false);
    }
    w.ue(0);
    w.flag(false);
    w.flag(false);
    w.flag(true);
    w.flag(colour);
    if colour {
        w.flag(false);
        w.flag(false);
        w.flag(true);
        w.u(3, 5);
        w.flag(false);
        w.flag(true);
        for _ in 0..3 {
            w.u(8, 1);
        }
        for _ in 0..7 {
            w.flag(false);
        }
    }
    w.flag(false);
    let sps = nal(33, &w.padded());

    let mut w = Writer::default();
    w.ue(0);
    w.ue(0);
    w.flag(true);
    w.flag(true);
    w.u(3, 0);
    w.flag(false);
    w.flag(false);
    w.ue(0);
    w.ue(0);
    w.ue(0);
    for _ in 0..3 {
        w.flag(false);
    }
    w.ue(0);
    w.ue(0);
    for _ in 0..10 {
        w.flag(false);
    }
    w.ue(0);
    w.flag(false);
    w.flag(false);
    let pps = nal(34, &w.padded());
    [vps, sps, pps]
}

fn hvcc(sets: &[Vec<u8>; 3]) -> Vec<u8> {
    let mut bytes = vec![
        1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 120, 0xf0, 0, 0xfc, 0xfd, 0xf8, 0xf8, 0, 0, 15, 3,
    ];
    for (kind, set) in (32..=34).zip(sets) {
        bytes.push(kind | 0x80);
        bytes.extend(1u16.to_be_bytes());
        bytes.extend((set.len() as u16).to_be_bytes());
        bytes.extend(set);
    }
    bytes
}

fn limits() -> Limits {
    Limits {
        max_configuration_bytes: 4096,
        max_width: 4096,
        max_height: 4096,
        max_access_unit_bytes: 16384,
        max_nals: 32,
    }
}

fn config_from(sets: &[Vec<u8>; 3], hvcc: &[u8], bounds: Limits) -> Result<Config, Error> {
    let [vps, sps, pps] = sets;
    Config::from_parameter_sets(
        vps,
        sps,
        pps,
        hvcc,
        Dimensions::new(128, 64).map_err(|_| Error::Unsupported)?,
        bounds,
    )
}

fn config() -> Result<Config, Error> {
    let sets = sets(1, 8, true);
    config_from(&sets, &hvcc(&sets), limits())
}

fn slice(kind: u8, first: bool, dependent: bool, slice_type: u32, output: bool) -> Vec<u8> {
    let mut w = Writer::default();
    w.flag(first);
    if (16..=21).contains(&kind) {
        w.flag(false);
    }
    w.ue(0);
    if !first {
        w.flag(dependent);
        w.u(1, 1);
    }
    if !dependent {
        w.ue(slice_type);
        w.flag(output);
        if kind != 19 && kind != 20 {
            w.u(4, 1);
            w.flag(false);
            w.ue(u32::from(slice_type < 2));
            w.ue(0);
            if slice_type < 2 {
                w.ue(0);
                w.flag(true);
            }
        }
        if slice_type < 2 {
            w.flag(false);
            if slice_type == 0 {
                w.flag(false);
            }
            w.ue(0);
        }
        w.ue(0);
    }
    let mut rbsp = w.padded();
    rbsp.push(0x80);
    nal(kind, &rbsp)
}

fn unit(nals: &[&[u8]], sync: bool) -> Result<AccessUnit, Error> {
    let mut bytes = Vec::new();
    for nal in nals {
        bytes.extend((nal.len() as u32).to_be_bytes());
        bytes.extend_from_slice(nal);
    }
    AccessUnit::copy_from(&bytes, sync, limits())
}

#[test]
fn configuration_owns_all_sets_and_exact_hvcc() -> Result<(), Error> {
    let mut sets = sets(1, 8, true);
    let mut record = hvcc(&sets);
    let config = config_from(&sets, &record, limits())?;
    let [vps, sps, pps] = &sets;
    assert_eq!(config.vps(), vps);
    assert_eq!(config.sps(), sps);
    assert_eq!(config.pps(), pps);
    assert_eq!(config.as_hvcc(), record);
    assert_eq!(config.dimensions().width(), 128);
    for set in &mut sets {
        set.fill(0);
    }
    record.fill(0);
    assert_ne!(config.as_hvcc(), record);
    assert!(config.vps().iter().any(|byte| *byte != 0));
    Ok(())
}

#[test]
fn hvcc_requires_exact_unique_arrays_and_no_trailing_bytes() {
    let sets = sets(1, 8, true);
    let record = hvcc(&sets);
    for length in 0..record.len() {
        assert!(config_from(&sets, record.get(..length).unwrap_or_default(), limits()).is_err());
    }
    let mut trailing = record.clone();
    trailing.push(0);
    assert_eq!(
        config_from(&sets, &trailing, limits()).err(),
        Some(Error::Malformed)
    );
    for (offset, value) in [(21, 12), (22, 4), (23, 0xa7), (23, 0xe0), (25, 2)] {
        let mut changed = record.clone();
        if let Some(byte) = changed.get_mut(offset) {
            *byte = value;
        }
        assert!(config_from(&sets, &changed, limits()).is_err());
    }
    let mut changed_sets = sets.clone();
    if let Some(byte) = changed_sets.first_mut().and_then(|set| set.last_mut()) {
        *byte ^= 1;
    }
    assert_eq!(
        config_from(&changed_sets, &record, limits()).err(),
        Some(Error::FormatMismatch)
    );
    let mut duplicate = record.clone();
    if let Some(byte) = duplicate.get_mut(23 + 5 + sets.first().map_or(0, Vec::len)) {
        *byte = 0xa0;
    }
    assert_eq!(
        config_from(&sets, &duplicate, limits()).err(),
        Some(Error::Malformed)
    );
}

#[test]
fn configuration_budget_counts_sets_and_record_together() -> Result<(), Error> {
    let sets = sets(1, 8, true);
    let record = hvcc(&sets);
    let size = record.len() + sets.iter().map(Vec::len).sum::<usize>();
    let mut bounds = limits();
    bounds.max_configuration_bytes = size;
    config_from(&sets, &record, bounds)?;
    bounds.max_configuration_bytes -= 1;
    assert_eq!(
        config_from(&sets, &record, bounds).err(),
        Some(Error::Limit)
    );
    Ok(())
}

#[test]
fn configuration_requires_main_record_and_nonzero_sufficient_bounds() -> Result<(), Error> {
    let sets = sets(1, 8, true);
    let record = hvcc(&sets);
    for profile in [0, 2, 3, 0x41, 0x81, 0xc1] {
        let mut changed = record.clone();
        if let Some(byte) = changed.get_mut(1) {
            *byte = profile;
        }
        assert_eq!(
            config_from(&sets, &changed, limits()).err(),
            Some(Error::Unsupported)
        );
    }
    for bounds in [
        Limits {
            max_configuration_bytes: 0,
            ..limits()
        },
        Limits {
            max_width: 0,
            ..limits()
        },
        Limits {
            max_height: 0,
            ..limits()
        },
        Limits {
            max_width: 127,
            ..limits()
        },
        Limits {
            max_height: 63,
            ..limits()
        },
    ] {
        assert_eq!(
            config_from(&sets, &record, bounds).err(),
            Some(Error::Limit)
        );
    }
    let [vps, sps, pps] = &sets;
    assert_eq!(
        Config::from_parameter_sets(
            vps,
            sps,
            pps,
            &record,
            Dimensions::new(4098, 64).map_err(|_| Error::Unsupported)?,
            limits()
        )
        .err(),
        Some(Error::Limit)
    );
    Ok(())
}

#[test]
fn samples_are_bounded_and_framed_before_ownership() {
    for bytes in [
        &[][..],
        &[0, 0, 0, 0],
        &[0, 0, 0, 2, 38],
        &[0, 0, 0, 1, 38],
        &[0, 0, 0],
    ] {
        assert!(AccessUnit::copy_from(bytes, true, limits()).is_err());
    }
    let bytes = [0, 0, 0, 2, 38, 1];
    let mut bounds = limits();
    bounds.max_access_unit_bytes = bytes.len() - 1;
    assert_eq!(
        AccessUnit::copy_from(&bytes, true, bounds).err(),
        Some(Error::Limit)
    );
    bounds = limits();
    bounds.max_nals = 0;
    assert_eq!(
        AccessUnit::copy_from(&bytes, true, bounds).err(),
        Some(Error::Limit)
    );
}

#[test]
fn idr_then_p_and_exact_repeated_sets_drain_one_picture_per_sample() -> Result<(), Error> {
    assert!(std::mem::size_of::<Stream>() <= 256);
    let config = config()?;
    let mut stream = Stream::new(&config, coreav_hevc::Limits::default())?;
    let idr = slice(19, true, false, 2, true);
    let continuation = slice(19, false, true, 2, true);
    let sample = unit(&[&idr, config.sps(), &continuation], true)?;
    assert!(sample.is_sync());
    assert!(!sample.as_bytes().is_empty());
    stream.validate(&sample, &config)?;
    stream.validate(&unit(&[&slice(1, true, false, 1, true)], false)?, &config)?;
    stream.finish(2)?;
    assert_eq!(stream.validate(&sample, &config), Err(Error::State));
    assert_eq!(stream.finish(2), Err(Error::State));
    Ok(())
}

#[test]
fn sample_boundaries_sync_replacements_and_end_markers_are_terminal() -> Result<(), Error> {
    let config = config()?;
    let idr = slice(19, true, false, 2, true);
    let continuation = slice(19, false, false, 2, true);
    for (sample, expected) in [
        (unit(&[&idr, &idr], true)?, Error::Picture),
        (unit(&[&continuation], true)?, Error::Malformed),
        (unit(&[&idr], false)?, Error::Sync),
        (unit(&[config.sps()], false)?, Error::Picture),
        (unit(&[&[72, 1]], false)?, Error::Unsupported),
        (unit(&[&[74, 1]], false)?, Error::Unsupported),
        (unit(&[&[66, 1, 0x80], &idr], true)?, Error::FormatMismatch),
    ] {
        let mut stream = Stream::new(&config, coreav_hevc::Limits::default())?;
        assert_eq!(stream.validate(&sample, &config), Err(expected));
        assert_eq!(
            stream.validate(&unit(&[&idr], true)?, &config),
            Err(Error::State)
        );
        assert_eq!(stream.finish(1), Err(Error::State));
    }
    let mut stream = Stream::new(&config, coreav_hevc::Limits::default())?;
    stream.validate(&unit(&[&idr], true)?, &config)?;
    assert_eq!(
        stream.validate(&unit(&[&continuation], true)?, &config),
        Err(Error::Picture)
    );
    Ok(())
}

#[test]
fn activation_requires_main8_explicit_bt709_and_expected_geometry() -> Result<(), Error> {
    let idr = unit(&[&slice(19, true, false, 2, true)], true)?;
    for (sets, expected) in [
        (sets(2, 8, true), Error::Unsupported),
        (sets(2, 10, true), Error::Unsupported),
        (sets(1, 8, false), Error::FormatMismatch),
    ] {
        let config = config_from(&sets, &hvcc(&sets), limits())?;
        let mut stream = Stream::new(&config, coreav_hevc::Limits::default())?;
        assert_eq!(stream.validate(&idr, &config), Err(expected));
    }
    let sets = sets(1, 8, true);
    let [vps, sps, pps] = &sets;
    let wrong = Config::from_parameter_sets(
        vps,
        sps,
        pps,
        &hvcc(&sets),
        Dimensions::new(64, 64).map_err(|_| Error::Unsupported)?,
        limits(),
    )?;
    let mut stream = Stream::new(&wrong, coreav_hevc::Limits::default())?;
    assert_eq!(stream.validate(&idr, &wrong), Err(Error::FormatMismatch));
    Ok(())
}

#[test]
fn b_slices_suppressed_output_and_incomplete_drain_counts_fail() -> Result<(), Error> {
    let config = config()?;
    let idr = unit(&[&slice(19, true, false, 2, true)], true)?;
    for sample in [
        unit(&[&slice(1, true, false, 0, true)], false)?,
        unit(&[&slice(1, true, false, 1, false)], false)?,
    ] {
        let mut stream = Stream::new(&config, coreav_hevc::Limits::default())?;
        stream.validate(&idr, &config)?;
        assert_eq!(stream.validate(&sample, &config), Err(Error::Unsupported));
    }
    for expected in [0, 2] {
        let mut stream = Stream::new(&config, coreav_hevc::Limits::default())?;
        stream.validate(&idr, &config)?;
        assert_eq!(stream.finish(expected), Err(Error::Count));
    }
    let mut stream = Stream::new(&config, coreav_hevc::Limits::default())?;
    assert!(stream.finish(0).is_err());
    Ok(())
}
