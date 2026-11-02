//! Checked MPEG-4 elementary-stream descriptor cookies from the Apple AAC API.

use coreav_backend_apple::aac::{Config, Error};

fn descriptor(tag: u8, data: &[u8], padded: bool) -> Vec<u8> {
    let mut bytes = vec![tag];
    if padded {
        bytes.extend([0x80, 0x80, 0x80]);
    }
    bytes.push(data.len() as u8);
    bytes.extend(data);
    bytes
}

fn cookie(asc: &[u8], padded: bool) -> Vec<u8> {
    let mut decoder = vec![0x40, 0x15];
    decoder.extend([0; 11]);
    decoder.extend(descriptor(5, asc, padded));
    let mut es = vec![0, 0, 0];
    es.extend(descriptor(4, &decoder, padded));
    es.extend(descriptor(6, &[2], padded));
    descriptor(3, &es, padded)
}

#[test]
fn valid_raw_and_descriptor_configurations_keep_the_same_format() -> Result<(), Error> {
    for (rate, channels) in [(44_100, 1), (44_100, 2), (48_000, 1), (48_000, 2)] {
        let expected = Config::for_pcm(rate, channels)?;
        let mut extended = expected.as_bytes().to_vec();
        extended.extend([0x56, 0xe5, 0]);
        for asc in [expected.as_bytes().as_slice(), extended.as_slice()] {
            assert_eq!(Config::from_cookie(asc, rate, channels)?, expected);
            for padded in [false, true] {
                assert_eq!(
                    Config::from_cookie(&cookie(asc, padded), rate, channels)?,
                    expected
                );
            }
        }
    }
    Ok(())
}

#[test]
fn descriptors_require_complete_extents_and_known_flags() {
    for padded in [false, true] {
        let valid = cookie(&[0x11, 0x90], padded);
        for end in 0..valid.len() {
            let prefix = valid.get(..end).unwrap_or_default();
            assert!(Config::from_cookie(prefix, 48_000, 2).is_err());
        }
        let mut trailing = valid.clone();
        trailing.push(0);
        assert!(Config::from_cookie(&trailing, 48_000, 2).is_err());
        let mut unclosed = valid.clone();
        if let Some(length) = unclosed.get_mut(1) {
            *length = 0xff;
        }
        assert!(Config::from_cookie(&unclosed, 48_000, 2).is_err());
    }
    for asc in [
        vec![0x11, 0x90, 0x56, 0xe5, 0x80],
        vec![0x11, 0x90, 0],
        vec![0x2b, 0x90],
        vec![],
    ] {
        assert!(Config::from_cookie(&cookie(&asc, false), 48_000, 2).is_err());
    }
    for es in [
        vec![0, 0, 0x80],
        vec![0, 0, 0x40],
        vec![0, 0, 0x20],
        vec![0, 0],
        vec![],
    ] {
        assert!(Config::from_cookie(&descriptor(3, &es, false), 48_000, 2).is_err());
    }
}

#[test]
fn a_cookie_cannot_change_the_requested_rate_or_channels() {
    let bytes = cookie(&[0x11, 0x90], true);
    assert_eq!(
        Config::from_cookie(&bytes, 44_100, 2),
        Err(Error::FormatMismatch)
    );
    assert_eq!(
        Config::from_cookie(&bytes, 48_000, 1),
        Err(Error::FormatMismatch)
    );
}

#[test]
fn an_apple_cookie_may_clear_only_the_decoder_reserved_bit() -> Result<(), Error> {
    let original = [
        0x03, 0x80, 0x80, 0x80, 0x22, 0, 0, 0, 0x04, 0x80, 0x80, 0x80, 0x14, 0x40, 0x14, 0, 0x18,
        0, 0, 0, 0, 0, 0, 0x01, 0xf4, 0, 0x05, 0x80, 0x80, 0x80, 0x02, 0x12, 0x08, 0x06, 0x80,
        0x80, 0x80, 0x01, 0x02,
    ];
    assert_eq!(
        Config::from_cookie(&original, 44_100, 1)?.as_bytes(),
        &[0x12, 0x08]
    );
    for flags in 0..=u8::MAX {
        let mut changed = original;
        if let Some(value) = changed.get_mut(14) {
            *value = flags;
        }
        assert_eq!(
            Config::from_cookie(&changed, 44_100, 1).is_ok(),
            matches!(flags, 0x14 | 0x15)
        );
    }
    assert_eq!(
        Config::from_cookie(&original, 48_000, 1),
        Err(Error::FormatMismatch)
    );
    assert_eq!(
        Config::from_cookie(&original, 44_100, 2),
        Err(Error::FormatMismatch)
    );
    Ok(())
}

#[test]
fn every_single_byte_mutation_is_checked_without_panicking() {
    let original = cookie(&[0x11, 0x90, 0x56, 0xe5, 0], true);
    for offset in 0..original.len() {
        for byte in 0..=u8::MAX {
            let mut changed = original.clone();
            if let Some(value) = changed.get_mut(offset) {
                *value = byte;
            }
            if let Ok(config) = Config::from_cookie(&changed, 48_000, 2) {
                assert_eq!(config.as_bytes(), &[0x11, 0x90]);
            }
        }
    }
}
