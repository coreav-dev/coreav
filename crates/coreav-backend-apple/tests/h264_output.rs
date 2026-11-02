//! Synthetic header fixtures, not decodable H.264 pictures or an encoder.

#[path = "common/h264.rs"]
mod fixtures;

use coreav_backend_apple::h264::{AccessUnit, Config, Dimensions, Error, Limits, Sync};
use fixtures::{Picture, Sequence, Slice, Writer, unit};

fn limits() -> Limits {
    Limits {
        max_configuration_bytes: 4096,
        max_width: 4096,
        max_height: 4096,
        max_access_unit_bytes: 16384,
        max_nals: 32,
    }
}
fn dimensions() -> Result<Dimensions, Error> {
    Dimensions::new(32, 32)
}
fn config() -> Result<Config, Error> {
    Config::from_parameter_sets(
        &Sequence::default().nal(),
        &Picture::default().nal(),
        dimensions()?,
        limits(),
    )
}

#[test]
fn configuration_is_owned_canonical_high_avcc() -> Result<(), Error> {
    let mut sps = Sequence::default().nal();
    let mut pps = Picture::default().nal();
    let c = Config::from_parameter_sets(&sps, &pps, dimensions()?, limits())?;
    assert_eq!(c.dimensions(), dimensions()?);
    let mut expected = vec![1, 100, 0, 31, 0xff, 0xe1];
    expected.extend((sps.len() as u16).to_be_bytes());
    expected.extend(&sps);
    expected.push(1);
    expected.extend((pps.len() as u16).to_be_bytes());
    expected.extend(&pps);
    expected.extend([0xfd, 0xf8, 0xf8, 0]);
    assert_eq!(c.as_avcc(), expected);
    let original_sps = sps.clone();
    let original_pps = pps.clone();
    sps.fill(0);
    pps.fill(0);
    assert_eq!(c.sps(), original_sps);
    assert_eq!(c.pps(), original_pps);
    Ok(())
}

#[test]
fn cropping_vui_hrd_and_pps_extension_are_checked() -> Result<(), Error> {
    let s = Sequence {
        crop: Some([1, 1, 1, 1]),
        vui: true,
        ..Sequence::default()
    };
    let p = Picture {
        extension: true,
        bottom: true,
        ..Picture::default()
    };
    let c = Config::from_parameter_sets(&s.nal(), &p.nal(), Dimensions::new(28, 28)?, limits())?;
    let slice = Slice {
        bottom: Some(-1),
        ..Slice::default()
    }
    .nal();
    assert_eq!(
        AccessUnit::copy_from(&unit(&[&slice]), &c, Sync::Idr, limits())?.slices(),
        1
    );
    let mut short = limits();
    short.max_width = 31;
    assert_eq!(
        Config::from_parameter_sets(&s.nal(), &p.nal(), Dimensions::new(28, 28)?, short).err(),
        Some(Error::Limit)
    );
    Ok(())
}

#[test]
fn zero_extended_sar_is_unspecified_not_malformed() -> Result<(), Error> {
    for sar in [(0, 0), (0, 1), (1, 0), (1, 1)] {
        let sps = Sequence {
            vui: true,
            sar,
            ..Sequence::default()
        }
        .nal();
        let c =
            Config::from_parameter_sets(&sps, &Picture::default().nal(), dimensions()?, limits())?;
        assert_eq!(c.sps(), sps);
        assert_eq!(c.dimensions(), dimensions()?);
    }
    Ok(())
}

#[test]
fn configuration_refuses_unsupported_tools_and_invalid_ranges() -> Result<(), Error> {
    let pps = Picture::default().nal();
    for sequence in [
        Sequence {
            profile: 66,
            ..Sequence::default()
        },
        Sequence {
            chroma: 2,
            ..Sequence::default()
        },
        Sequence {
            depth: 2,
            ..Sequence::default()
        },
        Sequence {
            progressive: false,
            ..Sequence::default()
        },
        Sequence {
            scaling: true,
            ..Sequence::default()
        },
        Sequence {
            poc: 1,
            ..Sequence::default()
        },
        Sequence {
            gaps: true,
            ..Sequence::default()
        },
        Sequence {
            vui: true,
            reorder: 1,
            ..Sequence::default()
        },
    ] {
        assert_eq!(
            Config::from_parameter_sets(&sequence.nal(), &pps, dimensions()?, limits()).err(),
            Some(Error::Unsupported)
        );
    }
    for sequence in [
        Sequence {
            constraints: 1,
            ..Sequence::default()
        },
        Sequence {
            frame_bits: 13,
            ..Sequence::default()
        },
        Sequence {
            id: 32,
            ..Sequence::default()
        },
        Sequence {
            references: 17,
            ..Sequence::default()
        },
        Sequence {
            crop: Some([0, 16, 0, 0]),
            ..Sequence::default()
        },
        Sequence {
            width_mbs: u32::MAX,
            ..Sequence::default()
        },
        Sequence {
            vui: true,
            cpb: 32,
            ..Sequence::default()
        },
    ] {
        assert_eq!(
            Config::from_parameter_sets(&sequence.nal(), &pps, dimensions()?, limits()).err(),
            Some(Error::Malformed)
        );
    }
    for picture in [
        Picture {
            groups: 1,
            ..Picture::default()
        },
        Picture {
            redundant: true,
            ..Picture::default()
        },
        Picture {
            extension: true,
            scaling: true,
            ..Picture::default()
        },
    ] {
        assert_eq!(
            Config::from_parameter_sets(
                &Sequence::default().nal(),
                &picture.nal(),
                dimensions()?,
                limits()
            )
            .err(),
            Some(Error::Unsupported)
        );
    }
    assert_eq!(
        Config::from_parameter_sets(
            &Sequence::default().nal(),
            &Picture {
                qp: 26,
                ..Picture::default()
            }
            .nal(),
            dimensions()?,
            limits()
        )
        .err(),
        Some(Error::Malformed)
    );
    Ok(())
}

#[test]
fn unknown_references_and_geometry_changes_are_refused() -> Result<(), Error> {
    let sps = Sequence::default().nal();
    let pps = Picture::default().nal();
    assert_eq!(
        Config::from_parameter_sets(&sps, &pps, Dimensions::new(30, 32)?, limits()).err(),
        Some(Error::FormatMismatch)
    );
    assert_eq!(
        Config::from_parameter_sets(
            &sps,
            &Picture {
                sequence: 1,
                ..Picture::default()
            }
            .nal(),
            dimensions()?,
            limits()
        )
        .err(),
        Some(Error::ParameterSet)
    );
    assert_eq!(
        Config::from_parameter_sets(
            &sps,
            &Picture {
                id: 256,
                ..Picture::default()
            }
            .nal(),
            dimensions()?,
            limits()
        )
        .err(),
        Some(Error::Malformed)
    );
    let c = config()?;
    assert_eq!(
        AccessUnit::copy_from(
            &unit(&[&Slice {
                pps: 1,
                ..Slice::default()
            }
            .nal()]),
            &c,
            Sync::Idr,
            limits()
        )
        .err(),
        Some(Error::ParameterSet)
    );
    Ok(())
}

#[test]
fn truncation_trailing_bytes_and_invalid_escapes_are_refused() -> Result<(), Error> {
    let sps = Sequence::default().nal();
    let pps = Picture::default().nal();
    for end in 0..sps.len() {
        assert!(
            Config::from_parameter_sets(
                sps.get(..end).unwrap_or_default(),
                &pps,
                dimensions()?,
                limits()
            )
            .is_err()
        );
    }
    for end in 0..pps.len() {
        assert!(
            Config::from_parameter_sets(
                &sps,
                pps.get(..end).unwrap_or_default(),
                dimensions()?,
                limits()
            )
            .is_err()
        );
    }
    let mut trailing = sps.clone();
    trailing.push(0);
    assert_eq!(
        Config::from_parameter_sets(&trailing, &pps, dimensions()?, limits()).err(),
        Some(Error::Malformed)
    );
    let c = config()?;
    for suffix in [&[0, 0, 0][..], &[0, 0, 1], &[0, 0, 2], &[0], &[0, 0, 3, 4]] {
        let mut nal = Slice::default().nal();
        nal.extend(suffix);
        assert_eq!(
            AccessUnit::copy_from(&unit(&[&nal]), &c, Sync::Idr, limits()).err(),
            Some(Error::Malformed)
        );
    }
    let bytes = unit(&[&Slice::default().nal()]);
    for end in 0..bytes.len() {
        assert!(
            AccessUnit::copy_from(
                bytes.get(..end).unwrap_or_default(),
                &c,
                Sync::Idr,
                limits()
            )
            .is_err()
        );
    }
    Ok(())
}

#[test]
fn terminal_cabac_escape_is_removed_without_changing_owned_bytes() -> Result<(), Error> {
    let c = config()?;
    for suffix in [&[0, 0, 3][..], &[0, 0, 3, 0, 0, 3]] {
        let mut nal = Slice::default().nal();
        nal.extend(suffix);
        let bytes = unit(&[&nal]);
        let checked = AccessUnit::copy_from(&bytes, &c, Sync::Idr, limits())?;
        assert_eq!(checked.as_bytes(), bytes);
        assert_eq!(checked.slices(), 1);
    }
    Ok(())
}

#[test]
fn byte_and_nal_limits_are_exact_and_precede_copying() -> Result<(), Error> {
    let sps = Sequence::default().nal();
    let pps = Picture::default().nal();
    let mut bounded = limits();
    bounded.max_configuration_bytes = sps.len() + pps.len() + 15;
    let c = Config::from_parameter_sets(&sps, &pps, dimensions()?, bounded)?;
    bounded.max_configuration_bytes -= 1;
    assert_eq!(
        Config::from_parameter_sets(&sps, &pps, dimensions()?, bounded).err(),
        Some(Error::Limit)
    );
    let bytes = unit(&[&Slice::default().nal()]);
    bounded.max_access_unit_bytes = bytes.len();
    bounded.max_nals = 1;
    assert!(AccessUnit::copy_from(&bytes, &c, Sync::Idr, bounded).is_ok());
    bounded.max_access_unit_bytes -= 1;
    assert_eq!(
        AccessUnit::copy_from(&bytes, &c, Sync::Idr, bounded).err(),
        Some(Error::Limit)
    );
    bounded = limits();
    bounded.max_nals = 0;
    assert_eq!(
        AccessUnit::copy_from(&bytes, &c, Sync::Idr, bounded).err(),
        Some(Error::Limit)
    );
    assert!(
        Config::from_parameter_sets(
            &vec![0; 65536],
            &pps,
            dimensions()?,
            Limits {
                max_configuration_bytes: usize::MAX,
                ..limits()
            }
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn one_picture_can_have_multiple_consistent_ordered_slices() -> Result<(), Error> {
    let c = config()?;
    let first = Slice::default().nal();
    let second = Slice {
        first_mb: 2,
        ..Slice::default()
    }
    .nal();
    let mut bytes = unit(&[c.sps(), c.pps(), &first, &second]);
    let original = bytes.clone();
    let output = AccessUnit::copy_from(&bytes, &c, Sync::Idr, limits())?;
    bytes.fill(0);
    assert_eq!(output.as_bytes(), original);
    assert_eq!(output.sync(), Sync::Idr);
    assert_eq!(output.slices(), 2);
    assert_eq!(output.nals(), 4);
    let p = Slice {
        idr: false,
        kind: 0,
        frame: 1,
        poc: 2,
        ..Slice::default()
    }
    .nal();
    assert_eq!(
        AccessUnit::copy_from(&unit(&[&p]), &c, Sync::NonIdr, limits())?.sync(),
        Sync::NonIdr
    );
    assert_eq!(
        AccessUnit::copy_from(&unit(&[&p]), &c, Sync::Idr, limits()).err(),
        Some(Error::Sync)
    );
    assert_eq!(
        AccessUnit::copy_from(&unit(&[&first]), &c, Sync::NonIdr, limits()).err(),
        Some(Error::Sync)
    );
    Ok(())
}

#[test]
fn multiple_pictures_reordered_slices_and_b_slices_are_refused() -> Result<(), Error> {
    let c = config()?;
    let first = Slice::default().nal();
    for slice in [
        Slice::default(),
        Slice {
            idr_id: 1,
            first_mb: 1,
            ..Slice::default()
        },
        Slice {
            poc: 1,
            first_mb: 1,
            ..Slice::default()
        },
        Slice {
            idr: false,
            frame: 1,
            kind: 0,
            first_mb: 1,
            ..Slice::default()
        },
    ] {
        assert_eq!(
            AccessUnit::copy_from(&unit(&[&first, &slice.nal()]), &c, Sync::Idr, limits()).err(),
            Some(Error::Picture)
        );
    }
    assert_eq!(
        AccessUnit::copy_from(
            &unit(&[&Slice {
                first_mb: 1,
                ..Slice::default()
            }
            .nal()]),
            &c,
            Sync::Idr,
            limits()
        )
        .err(),
        Some(Error::Picture)
    );
    assert_eq!(
        AccessUnit::copy_from(
            &unit(&[&Slice {
                first_mb: 4,
                ..Slice::default()
            }
            .nal()]),
            &c,
            Sync::Idr,
            limits()
        )
        .err(),
        Some(Error::Malformed)
    );
    assert_eq!(
        AccessUnit::copy_from(
            &unit(&[&Slice {
                idr: false,
                kind: 1,
                ..Slice::default()
            }
            .nal()]),
            &c,
            Sync::NonIdr,
            limits()
        )
        .err(),
        Some(Error::Unsupported)
    );
    assert_eq!(
        AccessUnit::copy_from(
            &unit(&[&Slice {
                frame: 1,
                ..Slice::default()
            }
            .nal()]),
            &c,
            Sync::Idr,
            limits()
        )
        .err(),
        Some(Error::Malformed)
    );
    Ok(())
}

#[test]
fn auxiliary_nals_have_bounded_known_forms() -> Result<(), Error> {
    let c = config()?;
    let slice = Slice::default().nal();
    let mut aud = Writer::default();
    aud.bits(0, 3);
    let aud = aud.nal(9);
    let sei = [6, 5, 1, 0x42, 0x80];
    let filler = [12, 0xff, 0x80];
    assert_eq!(
        AccessUnit::copy_from(
            &unit(&[&aud, &sei, &slice, &filler]),
            &c,
            Sync::Idr,
            limits()
        )?
        .nals(),
        4
    );
    for nal in [
        &[6, 5, 2, 0x42, 0x80][..],
        &[6, 0xff],
        &[12, 0, 0x80],
        &[0x86, 0x80],
        &[0x26, 0x80],
        &[0x45, 0x80],
        &[0x69, 0x80],
        &[10, 0x80],
    ] {
        assert!(AccessUnit::copy_from(&unit(&[nal, &slice]), &c, Sync::Idr, limits()).is_err());
    }
    assert!(AccessUnit::copy_from(&unit(&[&slice, &aud]), &c, Sync::Idr, limits()).is_err());
    assert_eq!(
        AccessUnit::copy_from(&unit(&[&sei]), &c, Sync::NonIdr, limits()).err(),
        Some(Error::Picture)
    );
    assert!(AccessUnit::copy_from(&[0, 0, 0, 0], &c, Sync::Idr, limits()).is_err());
    assert!(AccessUnit::copy_from(&[0xff; 4], &c, Sync::Idr, limits()).is_err());
    Ok(())
}

#[test]
fn auxiliary_nals_stay_in_their_access_unit_positions() -> Result<(), Error> {
    let c = config()?;
    let slice = Slice::default().nal();
    let sei = [6, 5, 1, 0x42, 0x80];
    let filler = [12, 0xff, 0x80];
    let bytes = unit(&[&sei, &slice, &filler]);
    assert_eq!(
        AccessUnit::copy_from(&bytes, &c, Sync::Idr, limits())?.as_bytes(),
        bytes
    );
    for bytes in [unit(&[&slice, &sei]), unit(&[&filler, &slice])] {
        assert_eq!(
            AccessUnit::copy_from(&bytes, &c, Sync::Idr, limits()).err(),
            Some(Error::Unsupported)
        );
    }
    Ok(())
}

#[test]
fn poc_type_two_and_nonzero_parameter_ids_are_supported() -> Result<(), Error> {
    let s = Sequence {
        id: 31,
        poc: 2,
        ..Sequence::default()
    };
    let p = Picture {
        id: 255,
        sequence: 31,
        ..Picture::default()
    };
    let c = Config::from_parameter_sets(&s.nal(), &p.nal(), dimensions()?, limits())?;
    let nal = Slice {
        pps: 255,
        poc_type: 2,
        ..Slice::default()
    }
    .nal();
    assert!(AccessUnit::copy_from(&unit(&[&nal]), &c, Sync::Idr, limits()).is_ok());
    for dimensions in [(0, 32), (32, 0), (31, 32), (32, 31)] {
        assert!(Dimensions::new(dimensions.0, dimensions.1).is_err());
    }
    Ok(())
}

#[test]
fn mutations_never_change_checked_geometry_or_owned_bytes() -> Result<(), Error> {
    let original = Sequence {
        vui: true,
        ..Sequence::default()
    }
    .nal();
    let pps = Picture::default().nal();
    for offset in 0..original.len() {
        for byte in 0..=u8::MAX {
            let mut changed = original.clone();
            if let Some(value) = changed.get_mut(offset) {
                *value = byte;
            }
            if let Ok(c) = Config::from_parameter_sets(&changed, &pps, dimensions()?, limits()) {
                assert_eq!(c.dimensions(), dimensions()?);
                assert_eq!(c.sps(), changed);
            }
        }
    }
    let c = config()?;
    let original = unit(&[&Slice::default().nal()]);
    for offset in 0..original.len() {
        for byte in 0..=u8::MAX {
            let mut changed = original.clone();
            if let Some(value) = changed.get_mut(offset) {
                *value = byte;
            }
            if let Ok(unit) = AccessUnit::copy_from(&changed, &c, Sync::Idr, limits()) {
                assert_eq!(unit.sync(), Sync::Idr);
                assert_eq!(unit.as_bytes(), changed);
            }
        }
    }
    Ok(())
}

#[test]
fn profile_flags_and_in_band_parameter_changes_are_not_assumed() -> Result<(), Error> {
    let pps = Picture::default().nal();
    let constrained = Sequence {
        constraints: 0x0c,
        ..Sequence::default()
    }
    .nal();
    assert!(Config::from_parameter_sets(&constrained, &pps, dimensions()?, limits()).is_ok());
    for constraints in [0x80, 0x40, 0x20, 1, 2, 3] {
        let sps = Sequence {
            constraints,
            ..Sequence::default()
        }
        .nal();
        assert_eq!(
            Config::from_parameter_sets(&sps, &pps, dimensions()?, limits()).err(),
            Some(Error::Malformed)
        );
    }
    let intra = Sequence {
        constraints: 0x10,
        ..Sequence::default()
    }
    .nal();
    assert_eq!(
        Config::from_parameter_sets(&intra, &pps, dimensions()?, limits()).err(),
        Some(Error::Unsupported)
    );
    let c = config()?;
    let slice = Slice::default().nal();
    assert_eq!(
        AccessUnit::copy_from(&unit(&[&constrained, &slice]), &c, Sync::Idr, limits()).err(),
        Some(Error::FormatMismatch)
    );
    assert!(AccessUnit::copy_from(&unit(&[&slice, c.pps()]), &c, Sync::Idr, limits()).is_err());
    let mut forbidden = Sequence::default().nal();
    if let Some(header) = forbidden.first_mut() {
        *header |= 0x80;
    }
    assert_eq!(
        Config::from_parameter_sets(&forbidden, &pps, dimensions()?, limits()).err(),
        Some(Error::Malformed)
    );
    let excessive_id = Sequence {
        id: u32::MAX,
        ..Sequence::default()
    }
    .nal();
    assert_eq!(
        Config::from_parameter_sets(&excessive_id, &pps, dimensions()?, limits()).err(),
        Some(Error::Malformed)
    );
    Ok(())
}

#[test]
fn delimiter_picture_types_must_agree_with_coded_slices() -> Result<(), Error> {
    let c = config()?;
    let i = Slice::default().nal();
    let p = Slice {
        idr: false,
        kind: 0,
        frame: 1,
        ..Slice::default()
    }
    .nal();
    for kind in 0..8 {
        let mut w = Writer::default();
        w.bits(kind, 3);
        let aud = w.nal(9);
        assert_eq!(
            AccessUnit::copy_from(&unit(&[&aud, &i]), &c, Sync::Idr, limits()).is_ok(),
            matches!(kind, 0..=2 | 5..=7)
        );
        assert_eq!(
            AccessUnit::copy_from(&unit(&[&aud, &p]), &c, Sync::NonIdr, limits()).is_ok(),
            matches!(kind, 1 | 2 | 6 | 7)
        );
    }
    let i = Slice {
        idr: false,
        ..Slice::default()
    }
    .nal();
    assert!(AccessUnit::copy_from(&unit(&[&i]), &c, Sync::NonIdr, limits()).is_ok());
    assert_eq!(
        AccessUnit::copy_from(&unit(&[&i]), &c, Sync::Idr, limits()).err(),
        Some(Error::Sync)
    );
    Ok(())
}

#[test]
fn extended_sei_extents_remain_bounded() -> Result<(), Error> {
    let c = config()?;
    let slice = Slice::default().nal();
    let mut sei = vec![6, 5, 255, 1];
    sei.extend([0x42; 256]);
    sei.push(0x80);
    assert!(AccessUnit::copy_from(&unit(&[&sei, &slice]), &c, Sync::Idr, limits()).is_ok());
    for bytes in [&[6, 5, 255, 0x80][..], &[6, 5, 0], &[6, 255, 255]] {
        assert!(AccessUnit::copy_from(&unit(&[bytes, &slice]), &c, Sync::Idr, limits()).is_err());
    }
    Ok(())
}

#[test]
fn the_fuzz_seed_uses_the_same_normative_metadata_fixture() {
    assert_eq!(
        Sequence::default().nal(),
        [0x67, 0x64, 0, 0x1f, 0xac, 0xec, 0x96, 0x40]
    );
    assert_eq!(Picture::default().nal(), [0x68, 0xee, 0x3c, 0x80]);
}
