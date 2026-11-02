//! Sequence-header syntax constructed from AV1 sections 5.5 and 6.4.

use std::{collections::BTreeMap, ops::Range};

use coreav_backend_svt_av1::{
    obu::{self, Obus},
    sequence::{
        ChromaPosition, ColorRange, Error, Limit, Limits, Malformed, Sequence, Unsupported,
    },
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const LIMITS: Limits = Limits {
    max_payload_bytes: 4096,
    max_width: 65_536,
    max_height: 65_536,
};

#[derive(Default)]
struct Writer {
    bits: Vec<bool>,
    fields: BTreeMap<String, Range<usize>>,
}

impl Writer {
    fn field(&mut self, name: impl Into<String>, count: u8, value: u32) {
        let start = self.bits.len();
        for shift in (0..count).rev() {
            self.bits.push(value & (1 << shift) != 0);
        }
        self.fields.insert(name.into(), start..self.bits.len());
    }

    fn uvlc(&mut self, value: u32) {
        let n = u64::from(value) + 1;
        let width = 64 - n.leading_zeros();
        for _ in 1..width {
            self.bits.push(false);
        }
        for shift in (0..width).rev() {
            self.bits.push(n & (1 << shift) != 0);
        }
    }

    fn change(&mut self, name: &str, value: u32) -> TestResult {
        let range = self.fields.get(name).ok_or("missing test field")?.clone();
        let count = range.len();
        for (i, bit) in self
            .bits
            .get_mut(range)
            .ok_or("missing test bits")?
            .iter_mut()
            .enumerate()
        {
            *bit = value & (1 << (count - i - 1)) != 0;
        }
        Ok(())
    }

    fn finish(&mut self) {
        self.field("trailing_one", 1, 1);
        self.field("trailing_zero", ((8 - self.bits.len() % 8) % 8) as u8, 0);
    }

    fn bytes(&self) -> Vec<u8> {
        self.bits
            .chunks(8)
            .map(|chunk| {
                chunk
                    .iter()
                    .enumerate()
                    .fold(0, |byte, (i, bit)| byte | (u8::from(*bit) << (7 - i)))
            })
            .collect()
    }
}

#[derive(Clone)]
struct Settings {
    profile: u8,
    still: bool,
    reduced: bool,
    timing: Option<(u32, u32, Option<u32>)>,
    model: Option<(u8, u32)>,
    display_delays: bool,
    ops: Vec<(u16, u8, bool, bool, Option<u8>)>,
    width: u32,
    height: u32,
    width_bits: u8,
    height_bits: u8,
    frame_id: Option<(u8, u8)>,
    order_hint: bool,
    screen_choose: bool,
    screen_force: bool,
    integer_choose: bool,
    depth: u8,
    mono: bool,
    description: Option<(u8, u8, u8)>,
    full_range: bool,
    subsampling: (bool, bool),
    position: u8,
    grain: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            profile: 0,
            still: false,
            reduced: false,
            timing: None,
            model: None,
            display_delays: false,
            ops: vec![(0, 7, false, false, None)],
            width: 1920,
            height: 1080,
            width_bits: 16,
            height_bits: 16,
            frame_id: None,
            order_hint: false,
            screen_choose: false,
            screen_force: false,
            integer_choose: false,
            depth: 8,
            mono: false,
            description: None,
            full_range: false,
            subsampling: (true, true),
            position: 0,
            grain: false,
        }
    }
}

impl Settings {
    fn write(&self) -> Writer {
        let mut w = Writer::default();
        w.field("profile", 3, self.profile.into());
        w.field("still", 1, self.still.into());
        w.field("reduced", 1, self.reduced.into());
        if self.reduced {
            w.field("level_0", 5, self.ops.first().map_or(0, |op| op.1.into()));
        } else {
            w.field("timing_present", 1, self.timing.is_some().into());
            if let Some((tick, scale, equal)) = self.timing {
                w.field("display_tick", 32, tick);
                w.field("time_scale", 32, scale);
                w.field("equal_interval", 1, equal.is_some().into());
                if let Some(interval) = equal {
                    w.uvlc(interval);
                }
                w.field("model_present", 1, self.model.is_some().into());
                if let Some((delay_bits, decoding_tick)) = self.model {
                    w.field("delay_length", 5, u32::from(delay_bits - 1));
                    w.field("decoding_tick", 32, decoding_tick);
                    w.field("removal_length", 5, 31);
                    w.field("presentation_length", 5, 0);
                }
            }
            w.field("display_delays", 1, self.display_delays.into());
            w.field("op_count", 5, (self.ops.len() - 1) as u32);
            for (i, &(id, level, tier, model, delay)) in self.ops.iter().enumerate() {
                w.field(format!("op_id_{i}"), 12, id.into());
                w.field(format!("level_{i}"), 5, level.into());
                if level > 7 {
                    w.field(format!("tier_{i}"), 1, tier.into());
                }
                if let Some((n, _)) = self.model {
                    w.field(format!("model_{i}"), 1, model.into());
                    if model {
                        w.field(format!("decoder_delay_{i}"), n, u32::MAX);
                        w.field(format!("encoder_delay_{i}"), n, 1);
                        w.field(format!("low_delay_{i}"), 1, (i % 2 == 0).into());
                    }
                }
                if self.display_delays {
                    w.field(
                        format!("display_delay_present_{i}"),
                        1,
                        delay.is_some().into(),
                    );
                    if let Some(delay) = delay {
                        w.field(format!("display_delay_{i}"), 4, (delay - 1).into());
                    }
                }
            }
        }
        w.field("width_bits", 4, (self.width_bits - 1).into());
        w.field("height_bits", 4, (self.height_bits - 1).into());
        w.field("width", self.width_bits, self.width - 1);
        w.field("height", self.height_bits, self.height - 1);
        if !self.reduced {
            w.field("frame_id_present", 1, self.frame_id.is_some().into());
            if let Some((delta, additional)) = self.frame_id {
                w.field("delta", 4, delta.into());
                w.field("additional", 3, additional.into());
            }
        }
        w.field("superblock", 1, 1);
        w.field("filter_intra", 1, 0);
        w.field("intra_edge", 1, 1);
        if !self.reduced {
            w.field("interintra", 1, 1);
            w.field("masked_compound", 1, 0);
            w.field("warped_motion", 1, 1);
            w.field("dual_filter", 1, 0);
            w.field("order_hint", 1, self.order_hint.into());
            if self.order_hint {
                w.field("joint_compound", 1, 0);
                w.field("reference_mvs", 1, 1);
            }
            w.field("screen_choose", 1, self.screen_choose.into());
            if !self.screen_choose {
                w.field("screen_force", 1, self.screen_force.into());
            }
            if self.screen_choose || self.screen_force {
                w.field("integer_choose", 1, self.integer_choose.into());
                if !self.integer_choose {
                    w.field("integer_force", 1, 1);
                }
            }
            if self.order_hint {
                w.field("order_hint_bits", 3, 7);
            }
        }
        w.field("superres", 1, 1);
        w.field("cdef", 1, 0);
        w.field("restoration", 1, 1);
        w.field("high_depth", 1, (self.depth > 8).into());
        if self.profile == 2 && self.depth > 8 {
            w.field("twelve_bit", 1, (self.depth == 12).into());
        }
        if self.profile != 1 {
            w.field("mono", 1, self.mono.into());
        }
        w.field("description", 1, self.description.is_some().into());
        if let Some((p, t, m)) = self.description {
            w.field("primaries", 8, p.into());
            w.field("transfer", 8, t.into());
            w.field("matrix", 8, m.into());
        }
        if self.mono {
            w.field("range", 1, self.full_range.into());
        } else {
            if self.description != Some((1, 13, 0)) {
                w.field("range", 1, self.full_range.into());
                let (x, y) = match (self.profile, self.depth) {
                    (0, _) => (true, true),
                    (1, _) => (false, false),
                    (_, 12) => {
                        w.field("subsampling_x", 1, self.subsampling.0.into());
                        if self.subsampling.0 {
                            w.field("subsampling_y", 1, self.subsampling.1.into());
                        }
                        (self.subsampling.0, self.subsampling.0 && self.subsampling.1)
                    }
                    _ => (true, false),
                };
                if x && y {
                    w.field("position", 2, self.position.into());
                }
            }
            w.field("separate_uv", 1, 1);
        }
        w.field("grain", 1, self.grain.into());
        w.finish();
        w
    }
}

fn parse_kind(
    payload: &[u8],
    kind: u8,
    limits: Limits,
) -> Result<Sequence, Box<dyn std::error::Error>> {
    let mut packet = vec![(kind << 3) | 2];
    let mut n = payload.len();
    loop {
        let low = (n & 127) as u8;
        n >>= 7;
        packet.push(low | if n == 0 { 0 } else { 128 });
        if n == 0 {
            break;
        }
    }
    packet.extend_from_slice(payload);
    let mut obus = Obus::new(
        &packet,
        obu::Limits {
            max_packet_bytes: packet.len(),
            max_obus: 1,
        },
    )?;
    let obu = obus.next().transpose()?.ok_or("missing framed OBU")?;
    Ok(Sequence::parse(obu, limits)?)
}

fn parse(payload: &[u8]) -> Result<Sequence, Box<dyn std::error::Error>> {
    parse_kind(payload, 1, LIMITS)
}

fn fails(payload: &[u8], expected: Error) -> TestResult {
    let error = parse(payload).err().ok_or("unexpected valid sequence")?;
    assert_eq!(error.downcast_ref::<Error>(), Some(&expected));
    Ok(())
}

#[test]
fn inferred_normal_configuration_and_av1c() -> TestResult {
    let seq = parse(&Settings::default().write().bytes())?;
    assert_eq!((seq.max_width(), seq.max_height()), (1920, 1080));
    assert!(!seq.still_picture());
    assert_eq!(seq.first_level(), 7);
    assert!(!seq.first_high_tier());
    assert_eq!(seq.first_display_delay(), None);
    assert_eq!(seq.timing(), None);
    let color = seq.color();
    assert_eq!(
        (color.primaries(), color.transfer(), color.matrix()),
        (2, 2, 2)
    );
    assert_eq!(color.range(), ColorRange::Studio);
    assert_eq!(color.chroma_position(), ChromaPosition::Unknown);
    assert_eq!(seq.av1c_header().bytes(), [0x81, 7, 0x0c, 0]);
    Ok(())
}

#[test]
fn reduced_header_infers_main_tier_even_for_high_level() -> TestResult {
    for level in [0, 7, 8, 23, 31] {
        let settings = Settings {
            still: true,
            reduced: true,
            ops: vec![(0, level, true, true, Some(16))],
            description: Some((1, 1, 1)),
            position: 2,
            full_range: true,
            ..Settings::default()
        };
        let seq = parse(&settings.write().bytes())?;
        assert!(seq.still_picture());
        assert_eq!(seq.first_level(), level);
        assert!(!seq.first_high_tier());
        assert_eq!(seq.first_display_delay(), None);
        assert_eq!(seq.timing(), None);
        assert_eq!(seq.color().chroma_position(), ChromaPosition::Colocated);
        assert_eq!(seq.color().range(), ColorRange::Full);
        assert_eq!(seq.av1c_header().bytes(), [0x81, level, 0x0e, 0]);
    }
    Ok(())
}

#[test]
fn full_timing_models_and_all_32_operating_points() -> TestResult {
    for delay_bits in [1, 32] {
        let settings = Settings {
            timing: Some((1001, 60_000, Some(1))),
            model: Some((delay_bits, 1000)),
            display_delays: true,
            ops: (0..32)
                .map(|i| {
                    (
                        i * 129,
                        if i == 0 { 13 } else { 8 },
                        i == 0,
                        i % 2 == 0,
                        if i % 3 == 0 { Some(16) } else { None },
                    )
                })
                .collect(),
            frame_id: Some((6, 7)),
            order_hint: true,
            screen_choose: true,
            integer_choose: true,
            description: Some((1, 1, 1)),
            position: 1,
            ..Settings::default()
        };
        let seq = parse(&settings.write().bytes())?;
        let timing = seq.timing().ok_or("missing timing")?;
        assert_eq!(timing.display_tick(), 1001);
        assert_eq!(timing.time_scale(), 60_000);
        assert_eq!(timing.ticks_per_picture(), Some(2));
        assert_eq!(seq.first_display_delay(), Some(16));
        assert_eq!(seq.first_level(), 13);
        assert!(seq.first_high_tier());
        assert_eq!(seq.color().chroma_position(), ChromaPosition::Vertical);
        assert_eq!(seq.av1c_header().bytes(), [0x81, 13, 0x8d, 0]);
    }
    Ok(())
}

#[test]
fn every_payload_prefix_is_rejected_for_each_header_shape() -> TestResult {
    let settings = [
        Settings::default(),
        Settings {
            still: true,
            reduced: true,
            ..Settings::default()
        },
        Settings {
            timing: Some((1, 1, None)),
            ..Settings::default()
        },
        Settings {
            timing: Some((1, u32::MAX, Some(u32::MAX - 1))),
            model: Some((32, u32::MAX)),
            display_delays: true,
            ops: (0..32).map(|i| (i, 31, true, true, Some(16))).collect(),
            frame_id: Some((13, 0)),
            order_hint: true,
            screen_force: true,
            integer_choose: false,
            description: Some((22, 15, 14)),
            position: 2,
            ..Settings::default()
        },
    ];
    for config in settings {
        let payload = config.write().bytes();
        parse(&payload)?;
        for n in 0..payload.len() {
            fails(payload.get(..n).ok_or("missing prefix")?, Error::Truncated)?;
        }
    }
    Ok(())
}

#[test]
fn conditional_inference_does_not_consume_absent_bits() -> TestResult {
    for order_hint in [false, true] {
        for (choose, force, integer_choose) in [
            (false, false, false),
            (false, true, false),
            (false, true, true),
            (true, false, false),
            (true, false, true),
        ] {
            for frame_id in [None, Some((0, 0)), Some((13, 0)), Some((6, 7))] {
                let config = Settings {
                    still: true,
                    order_hint,
                    screen_choose: choose,
                    screen_force: force,
                    integer_choose,
                    frame_id,
                    ..Settings::default()
                };
                let seq = parse(&config.write().bytes())?;
                assert!(seq.still_picture());
                assert_eq!(seq.first_level(), 7);
            }
        }
    }
    Ok(())
}

#[test]
fn timing_interval_extremes_and_optional_models() -> TestResult {
    for interval in [None, Some(0), Some(1), Some(254), Some(u32::MAX - 1)] {
        for model in [None, Some((1, 1)), Some((32, u32::MAX))] {
            let config = Settings {
                timing: Some((u32::MAX, u32::MAX, interval)),
                model,
                display_delays: true,
                ops: vec![(0, 8, false, false, Some(1)), (0xfff, 31, true, true, None)],
                ..Settings::default()
            };
            let seq = parse(&config.write().bytes())?;
            assert_eq!(
                seq.timing().ok_or("missing timing")?.ticks_per_picture(),
                interval.map(|v| v + 1)
            );
            assert_eq!(seq.first_display_delay(), Some(1));
            assert!(!seq.first_high_tier());
        }
    }
    Ok(())
}

#[test]
fn first_operating_point_is_preserved_instead_of_later_values() -> TestResult {
    for level in (0..=23).chain([31]) {
        for tier in [false, true] {
            let config = Settings {
                display_delays: true,
                ops: vec![
                    (0, level, tier, false, None),
                    (0xfff, 31, !tier, false, Some(16)),
                ],
                ..Settings::default()
            };
            let seq = parse(&config.write().bytes())?;
            assert_eq!(seq.first_level(), level);
            assert_eq!(seq.first_high_tier(), level > 7 && tier);
            assert_eq!(seq.first_display_delay(), None);
            assert_eq!(
                seq.av1c_header().bytes(),
                [0x81, level, if level > 7 && tier { 0x8c } else { 0x0c }, 0]
            );
        }
    }
    for level in 24..=30 {
        let config = Settings {
            ops: vec![
                (0, 0, false, false, None),
                (0xfff, level, true, false, None),
            ],
            ..Settings::default()
        };
        fails(
            &config.write().bytes(),
            Error::Malformed(Malformed::ReservedLevel),
        )?;
    }
    Ok(())
}

#[test]
fn rejects_reserved_profiles_levels_and_invalid_relationships() -> TestResult {
    for profile in 3..=7 {
        let mut w = Settings::default().write();
        w.change("profile", profile)?;
        fails(&w.bytes(), Error::Malformed(Malformed::ReservedProfile))?;
    }
    for reduced in [false, true] {
        for level in 24..=30 {
            let config = Settings {
                still: true,
                reduced,
                ..Settings::default()
            };
            let mut w = config.write();
            w.change("level_0", level)?;
            fails(&w.bytes(), Error::Malformed(Malformed::ReservedLevel))?;
        }
    }
    let config = Settings {
        reduced: true,
        ..Settings::default()
    };
    fails(
        &config.write().bytes(),
        Error::Malformed(Malformed::ReducedWithoutStill),
    )?;
    for id in [(14, 0), (7, 7), (15, 7)] {
        let config = Settings {
            frame_id: Some(id),
            ..Settings::default()
        };
        fails(
            &config.write().bytes(),
            Error::Malformed(Malformed::FrameIdLength),
        )?;
    }
    let config = Settings {
        ops: vec![
            (123, 7, false, false, None),
            (124, 8, true, false, None),
            (123, 31, true, false, None),
        ],
        ..Settings::default()
    };
    fails(
        &config.write().bytes(),
        Error::Malformed(Malformed::DuplicateOperatingPoint),
    )?;
    let config = Settings {
        ops: vec![(0, 7, false, false, None), (0, 7, false, false, None)],
        ..Settings::default()
    };
    fails(
        &config.write().bytes(),
        Error::Malformed(Malformed::DuplicateOperatingPoint),
    )?;
    Ok(())
}

#[test]
fn rejects_zero_clocks_and_uvlc_overflow() -> TestResult {
    for (tick, scale, model) in [(0, 1, None), (1, 0, None), (1, 1, Some((32, 0)))] {
        let config = Settings {
            timing: Some((tick, scale, None)),
            model,
            ..Settings::default()
        };
        fails(
            &config.write().bytes(),
            Error::Malformed(Malformed::ZeroClock),
        )?;
    }
    let config = Settings {
        timing: Some((1, 1, Some(u32::MAX))),
        ..Settings::default()
    };
    fails(
        &config.write().bytes(),
        Error::Malformed(Malformed::PictureInterval),
    )?;
    let mut w = Writer::default();
    w.field("profile_still_reduced", 5, 0);
    w.field("timing", 1, 1);
    w.field("tick", 32, 1);
    w.field("scale", 32, 1);
    w.field("equal", 1, 1);
    w.field("long_zeros", 32, 0);
    for _ in 0..256 {
        w.field("zeros", 32, 0);
    }
    fails(&w.bytes(), Error::Malformed(Malformed::PictureInterval))?;
    Ok(())
}

#[test]
fn rejects_all_reserved_color_codes_and_chroma_position() -> TestResult {
    for value in 0..=255 {
        for (field, valid) in [
            ("primaries", matches!(value, 1 | 2 | 4..=12 | 22)),
            ("transfer", matches!(value, 1 | 2 | 4..=18)),
            ("matrix", matches!(value, 0..=2 | 4..=14)),
        ] {
            if !valid {
                let config = Settings {
                    description: Some((1, 1, 1)),
                    ..Settings::default()
                };
                let mut w = config.write();
                w.change(field, value)?;
                fails(&w.bytes(), Error::Malformed(Malformed::ReservedColor))?;
            }
        }
    }
    let config = Settings {
        position: 3,
        ..Settings::default()
    };
    fails(
        &config.write().bytes(),
        Error::Malformed(Malformed::ReservedChromaPosition),
    )?;
    let config = Settings {
        description: Some((1, 1, 0)),
        ..Settings::default()
    };
    fails(
        &config.write().bytes(),
        Error::Malformed(Malformed::IdentitySubsampling),
    )?;
    Ok(())
}

#[test]
fn accepts_assigned_sdr_color_codes_and_all_chroma_positions() -> TestResult {
    for (p, t, m) in [
        (1, 1, 1),
        (2, 2, 2),
        (4, 4, 4),
        (5, 5, 5),
        (6, 6, 6),
        (7, 7, 7),
        (8, 8, 8),
        (9, 9, 9),
        (10, 10, 10),
        (11, 11, 11),
        (12, 12, 12),
        (22, 13, 13),
        (9, 14, 14),
        (9, 15, 9),
        (10, 17, 10),
    ] {
        for (position, expected) in [
            (0, ChromaPosition::Unknown),
            (1, ChromaPosition::Vertical),
            (2, ChromaPosition::Colocated),
        ] {
            for full_range in [false, true] {
                let config = Settings {
                    description: Some((p, t, m)),
                    position,
                    full_range,
                    ..Settings::default()
                };
                let color = parse(&config.write().bytes())?.color();
                assert_eq!(
                    (color.primaries(), color.transfer(), color.matrix()),
                    (p, t, m)
                );
                assert_eq!(color.chroma_position(), expected);
                assert_eq!(
                    color.range(),
                    if full_range {
                        ColorRange::Full
                    } else {
                        ColorRange::Studio
                    }
                );
            }
        }
    }
    Ok(())
}

#[test]
fn unsupported_formats_are_distinct_from_malformed_headers() -> TestResult {
    let cases = [
        (
            Settings {
                profile: 1,
                ..Settings::default()
            },
            Unsupported::Profile,
        ),
        (
            Settings {
                profile: 2,
                ..Settings::default()
            },
            Unsupported::Profile,
        ),
        (
            Settings {
                profile: 2,
                depth: 12,
                ..Settings::default()
            },
            Unsupported::Profile,
        ),
        (
            Settings {
                profile: 2,
                depth: 12,
                subsampling: (true, false),
                ..Settings::default()
            },
            Unsupported::Profile,
        ),
        (
            Settings {
                profile: 2,
                depth: 12,
                subsampling: (false, false),
                ..Settings::default()
            },
            Unsupported::Profile,
        ),
        (
            Settings {
                depth: 10,
                ..Settings::default()
            },
            Unsupported::BitDepth,
        ),
        (
            Settings {
                mono: true,
                ..Settings::default()
            },
            Unsupported::Monochrome,
        ),
        (
            Settings {
                description: Some((1, 13, 0)),
                ..Settings::default()
            },
            Unsupported::ChromaFormat,
        ),
        (
            Settings {
                description: Some((9, 16, 9)),
                ..Settings::default()
            },
            Unsupported::HdrTransfer,
        ),
        (
            Settings {
                description: Some((9, 18, 9)),
                ..Settings::default()
            },
            Unsupported::HdrTransfer,
        ),
        (
            Settings {
                grain: true,
                ..Settings::default()
            },
            Unsupported::FilmGrain,
        ),
    ];
    for (config, reason) in cases {
        let mut w = config.write();
        fails(&w.bytes(), Error::Unsupported(reason))?;
        w.change("trailing_one", 0)?;
        fails(&w.bytes(), Error::Malformed(Malformed::TrailingBits))?;
        let payload = config.write().bytes();
        for n in 0..payload.len() {
            fails(payload.get(..n).ok_or("missing prefix")?, Error::Truncated)?;
        }
    }
    Ok(())
}

#[test]
fn enforces_trailing_one_and_all_zero_padding_to_payload_end() -> TestResult {
    for reduced in [false, true] {
        let config = Settings {
            reduced,
            still: true,
            ..Settings::default()
        };
        let mut w = config.write();
        w.change("trailing_one", 0)?;
        fails(&w.bytes(), Error::Malformed(Malformed::TrailingBits))?;
        let w = config.write();
        let padding_bits = w
            .fields
            .get("trailing_zero")
            .ok_or("missing padding")?
            .len();
        assert!(padding_bits > 0);
        for bit in 0..padding_bits {
            let mut w = config.write();
            w.change("trailing_zero", 1 << bit)?;
            fails(&w.bytes(), Error::Malformed(Malformed::TrailingBits))?;
        }
        let mut payload = config.write().bytes();
        let before = parse(&payload)?;
        payload.extend_from_slice(&[0; 128]);
        assert_eq!(parse(&payload)?, before);
        for byte in 1..=255 {
            let mut invalid = payload.clone();
            invalid.push(byte);
            fails(&invalid, Error::Malformed(Malformed::TrailingBits))?;
        }
    }
    Ok(())
}

#[test]
fn caller_limits_are_exact_and_maximum_dimensions_can_be_odd() -> TestResult {
    for (width, height, width_bits, height_bits) in
        [(1, 1, 1, 1), (5, 7, 3, 3), (65_536, 65_536, 16, 16)]
    {
        let config = Settings {
            width,
            height,
            width_bits,
            height_bits,
            ..Settings::default()
        };
        let bytes = config.write().bytes();
        let exact = Limits {
            max_payload_bytes: bytes.len(),
            max_width: width,
            max_height: height,
        };
        let seq = parse_kind(&bytes, 1, exact)?;
        assert_eq!((seq.max_width(), seq.max_height()), (width, height));
        for (limits, reason) in [
            (
                Limits {
                    max_payload_bytes: bytes.len() - 1,
                    ..exact
                },
                Limit::PayloadBytes,
            ),
            (
                Limits {
                    max_width: width - 1,
                    ..exact
                },
                Limit::Width,
            ),
            (
                Limits {
                    max_height: height - 1,
                    ..exact
                },
                Limit::Height,
            ),
        ] {
            let error = parse_kind(&bytes, 1, limits)
                .err()
                .ok_or("limit was not enforced")?;
            assert_eq!(
                error.downcast_ref::<Error>(),
                Some(&Error::LimitExceeded(reason))
            );
        }
    }
    Ok(())
}

#[test]
fn wrong_obu_kinds_and_error_messages_have_no_payload() -> TestResult {
    let bytes = Settings::default().write().bytes();
    for kind in [3, 4, 5, 6, 7, 8, 15] {
        let error = parse_kind(&bytes, kind, LIMITS)
            .err()
            .ok_or("wrong OBU kind accepted")?;
        assert_eq!(
            error.downcast_ref::<Error>(),
            Some(&Error::NotSequenceHeader)
        );
    }
    let error = parse_kind(&[], 2, LIMITS)
        .err()
        .ok_or("temporal delimiter accepted")?;
    assert_eq!(
        error.downcast_ref::<Error>(),
        Some(&Error::NotSequenceHeader)
    );
    let error = Error::Malformed(Malformed::ReservedColor);
    let standard_error: &dyn std::error::Error = &error;
    assert_eq!(
        standard_error.to_string(),
        "AV1 color description is reserved"
    );
    assert!(standard_error.source().is_none());
    Ok(())
}
