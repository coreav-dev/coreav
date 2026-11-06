use crate::bits::Bits;
use crate::{Error, Result};

/// Optional VUI declarations, preserving absence separately from signalled values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Vui {
    /// Tolerated display declarations; invalid aspect ratios are unspecified.
    pub warnings: crate::Warnings,
    /// Aspect-ratio code, with reserved codes interpreted as unspecified (zero).
    pub aspect_ratio_idc: Option<u32>,
    /// Explicit sample-aspect-ratio width and height for Extended_SAR.
    pub sar: Option<(u32, u32)>,
    /// Signalled full-range flag, absent when video signal type is absent.
    pub full_range: Option<bool>,
    /// Signalled colour-primary code; reserved codes retain their wire value.
    pub colour_primaries: Option<u32>,
    /// Signalled transfer-characteristic code, without display processing.
    pub transfer_characteristics: Option<u32>,
    /// Signalled matrix-coefficient code, without colour conversion.
    pub matrix_coeffs: Option<u32>,
    /// Top-field and bottom-field chroma sample location codes.
    pub chroma_location: Option<(u32, u32)>,
    /// Explicit VUI clock declaration, not an inferred frame rate.
    pub timing: Option<Timing>,
    /// Additional left, right, top and bottom display offsets in chroma units.
    pub default_display_window: Option<[u32; 4]>,
    /// Whether VUI HRD syntax was present and validated.
    pub hrd_present: bool,
}

/// A signalled H.265 clock and its optional POC proportionality.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    /// Number of clock units in one clock tick.
    pub num_units_in_tick: u32,
    /// Clock units per second.
    pub time_scale: u32,
    /// Actual clock ticks per unit POC difference, when explicitly proportional.
    pub ticks_per_poc: Option<u32>,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Hrd {
    nal: bool,
    vcl: bool,
    sub_pic: bool,
}

pub(crate) fn hrd(
    bits: &mut Bits<'_>,
    common: bool,
    sub_layers: u32,
    state: &mut Hrd,
) -> Result<()> {
    if common {
        state.nal = bits.flag()?;
        state.vcl = bits.flag()?;
        state.sub_pic = false;
        if state.nal || state.vcl {
            state.sub_pic = bits.flag()?;
            if state.sub_pic {
                bits.skip(19)?;
            }
            bits.skip(8)?;
            if state.sub_pic {
                bits.skip(4)?;
            }
            bits.skip(15)?;
        }
    }
    for _ in 0..=sub_layers {
        let general = bits.flag()?;
        let fixed = general || bits.flag()?;
        let low_delay = if fixed {
            bits.ue(2047)?;
            false
        } else {
            bits.flag()?
        };
        let count = if low_delay { 1 } else { bits.ue(31)? + 1 };
        for present in [state.nal, state.vcl] {
            if present {
                for _ in 0..count {
                    bits.ue(u32::MAX - 1)?;
                    bits.ue(u32::MAX - 1)?;
                    if state.sub_pic {
                        bits.ue(u32::MAX - 1)?;
                        bits.ue(u32::MAX - 1)?;
                    }
                    bits.flag()?;
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn parse(bits: &mut Bits<'_>, sub_layers: u32, width: u32, height: u32) -> Result<Vui> {
    let mut vui = Vui::default();
    if bits.flag()? {
        let idc = bits.u(8)?;
        vui.aspect_ratio_idc = Some(if (17..255).contains(&idc) { 0 } else { idc });
        if idc == 255 {
            let width = bits.u(16)?;
            let height = bits.u(16)?;
            if width != 0 && height != 0 {
                let (mut a, mut b) = (width, height);
                while b != 0 {
                    (a, b) = (b, a % b);
                }
                if a != 1 {
                    vui.warnings.invalid_sample_aspect_ratio = true;
                    vui.aspect_ratio_idc = Some(0);
                }
            }
            if !vui.warnings.invalid_sample_aspect_ratio {
                vui.sar = Some((width, height));
            }
        }
    }
    if bits.flag()? {
        bits.flag()?;
    }
    if bits.flag()? {
        bits.u(3)?;
        vui.full_range = Some(bits.flag()?);
        if bits.flag()? {
            vui.colour_primaries = Some(bits.u(8)?);
            vui.transfer_characteristics = Some(bits.u(8)?);
            vui.matrix_coeffs = Some(bits.u(8)?);
        }
    }
    if bits.flag()? {
        vui.chroma_location = Some((bits.ue(5)?, bits.ue(5)?));
    }
    bits.flag()?;
    if bits.flag()? {
        return Err(Error::Unsupported("HEVC field sequence"));
    }
    bits.flag()?;
    if bits.flag()? {
        let left = bits.ue(u32::MAX)?;
        let right = bits.ue(u32::MAX)?;
        let top = bits.ue(u32::MAX)?;
        let bottom = bits.ue(u32::MAX)?;
        if u64::from(left) + u64::from(right) >= u64::from(width).div_ceil(2)
            || u64::from(top) + u64::from(bottom) >= u64::from(height).div_ceil(2)
        {
            return Err(Error::InvalidData("VUI display window"));
        }
        vui.default_display_window = Some([left, right, top, bottom]);
    }
    if bits.flag()? {
        let num_units_in_tick = bits.u(32)?;
        let time_scale = bits.u(32)?;
        if num_units_in_tick == 0 || time_scale == 0 {
            return Err(Error::InvalidData("VUI timing"));
        }
        let ticks_per_poc = if bits.flag()? {
            Some(bits.ue(u32::MAX - 1)? + 1)
        } else {
            None
        };
        vui.timing = Some(Timing {
            num_units_in_tick,
            time_scale,
            ticks_per_poc,
        });
        vui.hrd_present = bits.flag()?;
        if vui.hrd_present {
            hrd(bits, true, sub_layers, &mut Hrd::default())?;
        }
    }
    if bits.flag()? {
        bits.skip(3)?;
        bits.ue(4095)?;
        bits.ue(16)?;
        bits.ue(16)?;
        bits.ue(15)?;
        bits.ue(15)?;
    }
    Ok(vui)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::params::tests::Writer;

    #[test]
    fn optional_colour_aspect_display_and_clock_declarations_are_preserved() {
        let mut w = Writer::default();
        w.flag(true);
        w.u(8, 255);
        w.u(16, 4);
        w.u(16, 3);
        w.flag(true);
        w.flag(false);
        w.flag(true);
        w.u(3, 5);
        w.flag(true);
        w.flag(true);
        w.u(8, 9);
        w.u(8, 16);
        w.u(8, 9);
        w.flag(true);
        w.ue(2);
        w.ue(2);
        w.flag(false);
        w.flag(false);
        w.flag(true);
        w.flag(true);
        for offset in [1, 2, 3, 4] {
            w.ue(offset);
        }
        w.flag(true);
        w.u(32, 1001);
        w.u(32, 60000);
        w.flag(true);
        w.ue(1);
        w.flag(false);
        w.flag(true);
        w.u(3, 0);
        for bound in [4095, 16, 16, 15, 15] {
            w.ue(bound);
        }
        let bytes = w.bytes();
        let mut bits = Bits::new(&bytes);
        let vui = parse(&mut bits, 0, 64, 64).unwrap();
        assert_eq!(bits.position(), w.len());
        assert_eq!(vui.aspect_ratio_idc, Some(255));
        assert_eq!(vui.sar, Some((4, 3)));
        assert_eq!(vui.full_range, Some(true));
        assert_eq!(vui.colour_primaries, Some(9));
        assert_eq!(vui.transfer_characteristics, Some(16));
        assert_eq!(vui.matrix_coeffs, Some(9));
        assert_eq!(vui.chroma_location, Some((2, 2)));
        assert_eq!(vui.default_display_window, Some([1, 2, 3, 4]));
        assert_eq!(
            vui.timing,
            Some(Timing {
                num_units_in_tick: 1001,
                time_scale: 60000,
                ticks_per_poc: Some(2)
            })
        );
        assert!(!vui.hrd_present);
        assert_eq!(
            parse(&mut Bits::new(&[0, 0]), 0, 64, 64).unwrap(),
            Vui::default()
        );
    }

    #[test]
    fn hrd_common_parameters_are_inherited_by_the_next_structure() {
        let mut w = Writer::default();
        w.flag(true);
        w.flag(false);
        w.flag(false);
        w.u(8, 0);
        w.u(15, 0);
        for _ in 0..2 {
            w.flag(false);
            w.flag(false);
            w.flag(true);
            w.ue(7);
            w.ue(11);
            w.flag(true);
        }
        let bytes = w.bytes();
        let mut bits = Bits::new(&bytes);
        let mut state = Hrd::default();
        hrd(&mut bits, true, 0, &mut state).unwrap();
        assert!(state.nal && !state.vcl && !state.sub_pic);
        hrd(&mut bits, false, 0, &mut state).unwrap();
        assert_eq!(bits.position(), w.len());
    }

    #[test]
    fn invalid_display_window_and_truncated_hrd_fail() {
        let mut w = Writer::default();
        w.u(8, 1);
        for offset in [16, 16, 0, 0] {
            w.ue(offset);
        }
        let bytes = w.bytes();
        assert_eq!(
            parse(&mut Bits::new(&bytes), 0, 64, 64).unwrap_err(),
            Error::InvalidData("VUI display window")
        );
        assert!(matches!(
            hrd(&mut Bits::new(&[0x80]), true, 0, &mut Hrd::default()),
            Err(Error::InvalidData(_))
        ));
    }

    #[test]
    fn explicit_sar_is_coprime_or_unspecified() {
        let mut w = Writer::default();
        w.flag(true);
        w.u(8, 255);
        w.u(16, 8);
        w.u(16, 6);
        w.u(9, 0);
        let bytes = w.bytes();
        let vui = parse(&mut Bits::new(&bytes), 0, 64, 64).unwrap();
        assert_eq!(vui.sar, None);
        assert_eq!(vui.aspect_ratio_idc, Some(0));
        assert!(vui.warnings.invalid_sample_aspect_ratio);
        assert!(parse(&mut Bits::new(&bytes[..5]), 0, 64, 64).is_err());
        let mut w = Writer::default();
        w.flag(true);
        w.u(8, 255);
        w.u(16, 0);
        w.u(16, 6);
        w.u(9, 0);
        let bytes = w.bytes();
        assert_eq!(
            parse(&mut Bits::new(&bytes), 0, 64, 64).unwrap().sar,
            Some((0, 6))
        );
    }
}
