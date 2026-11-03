//! Handwritten syntax fixtures from AV1 sections 5.5, 5.9, 5.10 and 5.11.

use std::{collections::BTreeMap, ops::Range};

pub(crate) use coreav_backend_svt_av1::{frame, obu, sequence};

pub(crate) type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Default)]
pub(crate) struct Writer {
    pub(crate) bits: Vec<bool>,
    fields: BTreeMap<&'static str, Range<usize>>,
}

impl Writer {
    pub(crate) fn field(&mut self, name: &'static str, count: u8, value: u32) {
        let start = self.bits.len();
        for shift in (0..count).rev() {
            self.bits.push(value & (1 << shift) != 0);
        }
        self.fields.insert(name, start..self.bits.len());
    }
    pub(crate) fn change(&mut self, name: &str, value: u32) -> TestResult {
        let range = self.fields.get(name).ok_or("missing field")?.clone();
        let count = range.len();
        for (i, bit) in self
            .bits
            .get_mut(range)
            .ok_or("missing bits")?
            .iter_mut()
            .enumerate()
        {
            *bit = value & (1 << (count - i - 1)) != 0;
        }
        Ok(())
    }
    pub(crate) fn replace(&mut self, name: &str, fields: &[(u8, u32)]) -> TestResult {
        let range = self.fields.get(name).ok_or("missing field")?.clone();
        let mut replacement = Vec::new();
        for (count, value) in fields {
            for shift in (0..*count).rev() {
                replacement.push(value & (1 << shift) != 0);
            }
        }
        self.bits.splice(range, replacement);
        Ok(())
    }
    pub(crate) fn align(&mut self) {
        while self.bits.len() % 8 != 0 {
            self.bits.push(false);
        }
    }
    pub(crate) fn trailing(&mut self) {
        self.bits.push(true);
        self.align();
    }
    pub(crate) fn bytes(&self) -> Vec<u8> {
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

#[derive(Clone, Copy)]
pub(crate) struct Seq {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) hints: u8,
    pub(crate) screen: bool,
    pub(crate) filters: bool,
    pub(crate) reduced: bool,
}

impl Default for Seq {
    fn default() -> Self {
        Self {
            width: 64,
            height: 64,
            hints: 3,
            screen: false,
            filters: false,
            reduced: false,
        }
    }
}

impl Seq {
    pub(crate) fn syntax(self) -> Writer {
        let mut w = Writer::default();
        w.field("profile", 3, 0);
        w.field("still", 1, self.reduced.into());
        w.field("reduced", 1, self.reduced.into());
        if !self.reduced {
            w.field("timing", 1, 0);
            w.field("display_delay", 1, 0);
            w.field("ops", 5, 0);
            w.field("op_idc", 12, 0);
        }
        w.field("level", 5, 7);
        w.field("width_bits", 4, 15);
        w.field("height_bits", 4, 15);
        w.field("width", 16, self.width - 1);
        w.field("height", 16, self.height - 1);
        if !self.reduced {
            w.field("frame_ids", 1, 0);
        }
        w.field("superblock", 1, 0);
        w.field("intra_tools", 2, 0);
        if !self.reduced {
            w.field("inter_tools", 2, 0);
            w.field("warped_motion", 1, self.filters.into());
            w.field("dual_filter", 1, 0);
            w.field("hints", 1, (self.hints != 0).into());
            if self.hints != 0 {
                w.field("hint_tools", 2, 0);
            }
            w.field("screen_choose", 1, self.screen.into());
            if !self.screen {
                w.field("screen_force", 1, 0);
            }
            if self.screen {
                w.field("integer_choose", 1, 1);
            }
            if self.hints != 0 {
                w.field("hint_bits", 3, u32::from(self.hints - 1));
            }
        }
        w.field("superres", 1, self.filters.into());
        w.field("cdef", 1, self.filters.into());
        w.field("restoration", 1, self.filters.into());
        w.field("depth", 1, 0);
        w.field("mono", 1, 0);
        w.field("color_description", 1, 0);
        w.field("range", 1, 0);
        w.field("chroma_position", 2, 0);
        w.field("separate_uv", 1, 0);
        w.field("film_grain", 1, 0);
        w.trailing();
        w
    }
    pub(crate) fn packet(self) -> Vec<u8> {
        packet(1, &self.syntax().bytes())
    }
    pub(crate) fn parse(self) -> Result<sequence::Sequence, Box<dyn std::error::Error>> {
        parse_sequence(&self.packet())
    }
    pub(crate) fn expected(self) -> frame::Expected {
        frame::Expected {
            width: self.width,
            height: self.height,
        }
    }
    pub(crate) fn reader(self) -> Result<frame::Frames, Box<dyn std::error::Error>> {
        Ok(frame::Frames::new(self.parse()?, self.expected(), 4096)?)
    }
}

pub(crate) fn parse_sequence(
    bytes: &[u8],
) -> Result<sequence::Sequence, Box<dyn std::error::Error>> {
    Ok(sequence::Sequence::parse(
        obu(bytes)?,
        sequence::Limits {
            max_payload_bytes: 4096,
            max_width: 65_536,
            max_height: 65_536,
        },
    )?)
}

#[derive(Clone, Copy)]
pub(crate) struct Coded {
    pub(crate) kind: u32,
    pub(crate) show: bool,
    pub(crate) resilient: bool,
    pub(crate) refresh: u32,
    pub(crate) hint: u32,
    pub(crate) refs: [u32; 7],
    pub(crate) override_size: Option<frame::Expected>,
    pub(crate) inherited: bool,
    pub(crate) render: Option<frame::Expected>,
    pub(crate) lossy: bool,
    pub(crate) tile_cols: u8,
    pub(crate) disable_cdf: bool,
}

impl Default for Coded {
    fn default() -> Self {
        Self {
            kind: 0,
            show: true,
            resilient: false,
            refresh: 1,
            hint: 0,
            refs: [0; 7],
            override_size: None,
            inherited: false,
            render: None,
            lossy: false,
            tile_cols: 0,
            disable_cdf: true,
        }
    }
}

impl Coded {
    pub(crate) fn syntax(self, seq: Seq) -> Writer {
        let mut w = Writer::default();
        let intra = self.kind == 0 || self.kind == 2;
        let resilient =
            seq.reduced || self.kind == 3 || (self.kind == 0 && self.show) || self.resilient;
        if !seq.reduced {
            w.field("existing", 1, 0);
            w.field("kind", 2, self.kind);
            w.field("show", 1, self.show.into());
            if !self.show {
                w.field("showable", 1, 1);
            }
            if self.kind != 3 && !(self.kind == 0 && self.show) {
                w.field("resilient", 1, self.resilient.into());
            }
        }
        w.field("disable_cdf", 1, self.disable_cdf.into());
        if seq.screen || seq.reduced {
            w.field("screen", 1, 0);
        }
        if !seq.reduced && self.kind != 3 {
            w.field("override", 1, self.override_size.is_some().into());
        }
        if !seq.reduced {
            w.field("hint", seq.hints, self.hint);
        }
        if !intra && !resilient {
            w.field("primary", 3, 7);
        }
        if self.kind != 3 && !(self.kind == 0 && self.show) {
            w.field("refresh", 8, self.refresh);
        }
        if (!intra || self.refresh != 255)
            && !(self.kind == 0 && self.show)
            && resilient
            && seq.hints != 0
        {
            for _ in 0..8 {
                w.field("ref_hint", seq.hints, 0);
            }
        }
        if !intra {
            if seq.hints != 0 {
                w.field("short_refs", 1, 0);
            }
            for index in self.refs {
                w.field("ref_index", 3, index);
            }
        }
        if !intra && self.override_size.is_some() && !resilient {
            if self.inherited {
                w.field("found_ref", 1, 1);
            } else {
                for _ in 0..7 {
                    w.field("found_ref", 1, 0);
                }
            }
        }
        if !self.inherited {
            if let Some(dim) = self.override_size {
                w.field("frame_width", 16, dim.width - 1);
                w.field("frame_height", 16, dim.height - 1);
            }
        }
        if seq.filters {
            w.field("superres", 1, 0);
        }
        if !self.inherited {
            w.field("render_different", 1, self.render.is_some().into());
            if let Some(dim) = self.render {
                w.field("render_width", 16, dim.width - 1);
                w.field("render_height", 16, dim.height - 1);
            }
        }
        if !intra {
            w.field("high_precision", 1, 0);
            w.field("switch_filter", 1, 1);
            w.field("motion_switch", 1, 0);
        }
        if !seq.reduced && !self.disable_cdf {
            w.field("end_cdf", 1, 0);
        }
        w.field("uniform", 1, 1);
        let max_col = if seq.width > 64 {
            (seq.width.div_ceil(64).min(64)).next_power_of_two().ilog2() as u8
        } else {
            0
        };
        for log in 0..max_col {
            w.field("col_increment", 1, (log < self.tile_cols).into());
            if log >= self.tile_cols {
                break;
            }
        }
        if seq.height > 64 {
            w.field("row_increment", 1, 0);
        }
        if self.tile_cols > 0 {
            w.field("context_tile", self.tile_cols, 0);
            w.field("tile_size_bytes", 2, 0);
        }
        w.field("base_q", 8, if self.lossy { 10 } else { 0 });
        w.field("delta_y", 1, 0);
        w.field("delta_u_dc", 1, 0);
        w.field("delta_u_ac", 1, 0);
        w.field("qmatrix", 1, 0);
        w.field("segmentation", 1, 0);
        if self.lossy {
            w.field("delta_q", 1, 0);
            w.field("filter_y0", 6, 0);
            w.field("filter_y1", 6, 0);
            w.field("filter_sharp", 3, 0);
            w.field("filter_delta", 1, 0);
            if seq.filters {
                w.field("cdef_damping", 2, 0);
                w.field("cdef_bits", 2, 0);
                w.field("cdef_strengths", 12, 0);
                w.field("restoration_types", 6, 0);
            }
            w.field("tx_mode", 1, 0);
        }
        if !intra {
            w.field("compound", 1, 0);
        }
        if !intra && !resilient && seq.filters {
            w.field("warped", 1, 0);
        }
        w.field("reduced_tx", 1, 0);
        if !intra {
            for _ in 0..7 {
                w.field("global", 1, 0);
            }
        }
        w
    }
    pub(crate) fn packet(self, seq: Seq) -> Vec<u8> {
        let mut w = self.syntax(seq);
        w.align();
        let mut payload = w.bytes();
        if self.tile_cols > 0 {
            payload.push(0);
            for _ in 1..(1 << self.tile_cols) {
                payload.extend_from_slice(&[0, 0x80]);
            }
        }
        payload.push(0x80);
        packet(6, &payload)
    }
    pub(crate) fn header(self, seq: Seq) -> Vec<u8> {
        let mut w = self.syntax(seq);
        w.trailing();
        packet(3, &w.bytes())
    }
}

pub(crate) fn packet(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut bytes = vec![(kind << 3) | 2];
    let mut n = payload.len();
    loop {
        let low = (n & 127) as u8;
        n >>= 7;
        bytes.push(low | if n == 0 { 0 } else { 128 });
        if n == 0 {
            break;
        }
    }
    bytes.extend_from_slice(payload);
    bytes
}

pub(crate) fn obu(bytes: &[u8]) -> Result<obu::Obu<'_>, Box<dyn std::error::Error>> {
    Ok(obu::Obus::new(
        bytes,
        obu::Limits {
            max_packet_bytes: 1 << 20,
            max_obus: 1,
        },
    )?
    .next()
    .ok_or("no OBU")??)
}

pub(crate) fn existing(index: u32) -> Vec<u8> {
    let mut w = Writer::default();
    w.field("existing", 1, 1);
    w.field("index", 3, index);
    w.trailing();
    packet(3, &w.bytes())
}

#[test]
fn shown_keyframe_checks_actual_dimensions_and_random_access() -> TestResult {
    for seq in [
        Seq::default(),
        Seq {
            reduced: true,
            hints: 0,
            ..Seq::default()
        },
        Seq {
            filters: true,
            screen: true,
            ..Seq::default()
        },
    ] {
        let mut frames = seq.reader()?;
        let key = Coded {
            lossy: seq.filters,
            ..Coded::default()
        }
        .packet(seq);
        let picture = frames.push(obu(&key)?)?.ok_or("missing picture")?;
        assert_eq!(picture.dimensions, seq.expected());
        assert!(picture.random_access && picture.displayed && !picture.show_existing);
        assert_eq!(picture.identity, 0);
        frames.finish()?;
    }
    Ok(())
}

#[test]
fn hidden_reference_is_displayed_once_with_its_coded_identity() -> TestResult {
    let seq = Seq::default();
    let mut frames = seq.reader()?;
    frames.push(obu(&Coded::default().packet(seq))?)?;
    let hidden = Coded {
        kind: 1,
        show: false,
        hint: 2,
        refresh: 2,
        ..Coded::default()
    };
    assert!(
        !frames
            .push(obu(&hidden.packet(seq))?)?
            .ok_or("missing hidden")?
            .displayed
    );
    let shown = Coded {
        kind: 1,
        hint: 1,
        refresh: 4,
        ..Coded::default()
    };
    assert_eq!(
        frames
            .push(obu(&shown.packet(seq))?)?
            .ok_or("missing shown")?
            .identity,
        2
    );
    let picture = frames.push(obu(&existing(1))?)?.ok_or("missing existing")?;
    assert_eq!(picture.identity, 1);
    assert_eq!(picture.order_hint, 2);
    assert!(picture.displayed && picture.show_existing && !picture.random_access);
    frames.finish()?;
    assert_eq!(
        frames.push(obu(&existing(1))?),
        Err(frame::Error::Reference)
    );
    assert_eq!(
        frames.push(obu(&Coded::default().packet(seq))?),
        Err(frame::Error::Failed)
    );
    Ok(())
}

#[test]
fn hidden_loss_missing_references_and_bad_geometry_are_terminal() -> TestResult {
    let seq = Seq::default();
    let mut frames = seq.reader()?;
    assert_eq!(
        frames.push(obu(&existing(0))?),
        Err(frame::Error::Reference)
    );
    let mut frames = seq.reader()?;
    frames.push(obu(&Coded::default().packet(seq))?)?;
    frames.push(obu(&Coded {
        kind: 1,
        show: false,
        refresh: 2,
        ..Coded::default()
    }
    .packet(seq))?)?;
    assert_eq!(frames.finish(), Err(frame::Error::HiddenFrameLost));
    let mut frames = seq.reader()?;
    frames.push(obu(&Coded::default().packet(seq))?)?;
    frames.push(obu(&Coded {
        kind: 1,
        show: false,
        refresh: 2,
        ..Coded::default()
    }
    .packet(seq))?)?;
    assert_eq!(
        frames.push(obu(&Coded::default().packet(seq))?),
        Err(frame::Error::HiddenFrameLost)
    );
    for config in [
        Coded {
            override_size: Some(frame::Expected {
                width: 63,
                height: 64,
            }),
            ..Coded::default()
        },
        Coded {
            render: Some(frame::Expected {
                width: 64,
                height: 63,
            }),
            ..Coded::default()
        },
    ] {
        let mut frames = seq.reader()?;
        assert_eq!(
            frames.push(obu(&config.packet(seq))?),
            Err(frame::Error::Geometry)
        );
    }
    Ok(())
}

#[test]
fn headers_require_complete_tile_coverage_and_reject_wrong_group_order() -> TestResult {
    let seq = Seq {
        width: 128,
        ..Seq::default()
    };
    let config = Coded {
        tile_cols: 1,
        ..Coded::default()
    };
    let mut frames = seq.reader()?;
    assert!(frames.push(obu(&config.header(seq))?)?.is_none());
    assert!(frames.incomplete());
    let group0 = packet(4, &[0x80, 0x80]);
    assert!(frames.push(obu(&group0)?)?.is_none());
    let group1 = packet(4, &[0xe0, 0x80]);
    assert!(frames.push(obu(&group1)?)?.is_some());
    frames.finish()?;
    let mut frames = seq.reader()?;
    frames.push(obu(&config.header(seq))?)?;
    assert_eq!(frames.push(obu(&group1)?), Err(frame::Error::Ordering));
    let mut frames = seq.reader()?;
    frames.push(obu(&config.header(seq))?)?;
    assert_eq!(frames.finish(), Err(frame::Error::Truncated));
    let mut frames = seq.reader()?;
    assert!(frames.push(obu(&config.packet(seq))?)?.is_some());
    Ok(())
}

#[test]
fn unsupported_header_branches_fail_before_returning_a_picture() -> TestResult {
    let seq = Seq {
        width: 128,
        filters: true,
        ..Seq::default()
    };
    for (field, reason) in [
        ("uniform", frame::Unsupported::NonuniformTiles),
        ("segmentation", frame::Unsupported::Segmentation),
        ("superres", frame::Unsupported::SuperResolution),
    ] {
        let config = Coded::default();
        let mut w = config.syntax(seq);
        w.change(field, u32::from(field != "uniform"))?;
        w.align();
        let mut payload = w.bytes();
        payload.push(0x80);
        assert_eq!(
            seq.reader()?.push(obu(&packet(6, &payload))?),
            Err(frame::Error::Unsupported(reason))
        );
    }
    for (field, reason) in [
        ("short_refs", frame::Unsupported::ShortReferences),
        ("global", frame::Unsupported::GlobalMotion),
    ] {
        let mut frames = seq.reader()?;
        frames.push(obu(&Coded::default().packet(seq))?)?;
        let mut w = Coded {
            kind: 1,
            ..Coded::default()
        }
        .syntax(seq);
        w.change(field, 1)?;
        w.align();
        let mut payload = w.bytes();
        payload.push(0x80);
        assert_eq!(
            frames.push(obu(&packet(6, &payload))?),
            Err(frame::Error::Unsupported(reason))
        );
    }
    Ok(())
}

#[test]
fn every_truncated_keyframe_header_and_empty_tile_is_rejected() -> TestResult {
    let seq = Seq {
        filters: true,
        screen: true,
        ..Seq::default()
    };
    let mut w = Coded {
        lossy: true,
        ..Coded::default()
    }
    .syntax(seq);
    w.align();
    let payload = w.bytes();
    for n in 0..=payload.len() {
        let prefix = payload.get(..n).ok_or("prefix")?;
        assert!(seq.reader()?.push(obu(&packet(6, prefix))?).is_err());
    }
    let mut frames = frame::Frames::new(seq.parse()?, seq.expected(), 0)?;
    assert_eq!(
        frames.push(obu(&Coded::default().packet(seq))?),
        Err(frame::Error::TileLimit)
    );
    Ok(())
}

#[test]
fn inherited_dimensions_and_hidden_key_references_are_checked() -> TestResult {
    let seq = Seq::default();
    let mut frames = seq.reader()?;
    frames.push(obu(&Coded::default().packet(seq))?)?;
    let inter = Coded {
        kind: 1,
        inherited: true,
        override_size: Some(seq.expected()),
        ..Coded::default()
    };
    assert_eq!(
        frames
            .push(obu(&inter.packet(seq))?)?
            .ok_or("inter")?
            .dimensions,
        seq.expected()
    );
    let hidden_key = Coded {
        show: false,
        refresh: 2,
        ..Coded::default()
    };
    frames.push(obu(&hidden_key.packet(seq))?)?;
    assert!(
        frames
            .push(obu(&existing(1))?)?
            .ok_or("show hidden key")?
            .show_existing
    );
    frames.finish()?;
    assert_eq!(
        frames.push(obu(&existing(0))?),
        Err(frame::Error::Reference)
    );
    Ok(())
}

#[test]
fn optional_quantizers_filters_restoration_and_cdf_fields_are_consumed() -> TestResult {
    let seq = Seq {
        filters: true,
        ..Seq::default()
    };
    let config = Coded {
        lossy: true,
        disable_cdf: false,
        ..Coded::default()
    };
    let cases: &[(&str, &[(u8, u32)])] = &[
        ("delta_y", &[(1, 1), (7, 127)]),
        ("delta_q", &[(1, 1), (2, 2), (1, 1), (2, 1), (1, 1)]),
        ("qmatrix", &[(1, 1), (4, 5), (4, 8)]),
        ("filter_delta", &[(1, 1), (1, 1), (1, 1), (7, 63), (9, 0)]),
        (
            "restoration_types",
            &[(2, 1), (2, 2), (2, 0), (1, 1), (1, 1), (1, 1)],
        ),
    ];
    for (field, replacement) in cases {
        let mut w = config.syntax(seq);
        w.replace(field, replacement)?;
        w.align();
        let mut payload = w.bytes();
        payload.push(0x80);
        let picture = seq
            .reader()?
            .push(obu(&packet(6, &payload))?)?
            .ok_or("picture")?;
        assert!(picture.random_access);
    }
    let mut w = config.syntax(seq);
    w.change("filter_y0", 1)?;
    w.replace("filter_y1", &[(6, 2), (6, 3), (6, 4)])?;
    w.align();
    let mut payload = w.bytes();
    payload.push(0x80);
    assert!(seq.reader()?.push(obu(&packet(6, &payload))?)?.is_some());
    let mut w = config.syntax(seq);
    w.change("cdef_bits", 1)?;
    w.replace("cdef_strengths", &[(12, 13), (12, 39)])?;
    w.align();
    let mut payload = w.bytes();
    payload.push(0x80);
    assert!(seq.reader()?.push(obu(&packet(6, &payload))?)?.is_some());
    Ok(())
}

#[test]
fn intrabc_screen_selection_and_nonzero_alignment_do_not_shift_the_header() -> TestResult {
    let seq = Seq {
        screen: true,
        ..Seq::default()
    };
    let mut w = Coded::default().syntax(seq);
    w.replace("render_different", &[(1, 0), (1, 1)])?;
    w.replace("screen", &[(1, 1), (1, 0)])?;
    w.align();
    let mut payload = w.bytes();
    payload.push(0x80);
    assert!(seq.reader()?.push(obu(&packet(6, &payload))?)?.is_some());
    let mut w = Coded::default().syntax(Seq::default());
    w.align();
    let last = w.bits.last_mut().ok_or("alignment")?;
    *last = true;
    let mut payload = w.bytes();
    payload.push(0x80);
    assert_eq!(
        Seq::default().reader()?.push(obu(&packet(6, &payload))?),
        Err(frame::Error::Malformed)
    );
    Ok(())
}

#[test]
fn whole_frame_obus_forbid_existing_frames_and_explicit_tile_group_ranges() -> TestResult {
    let seq = Seq {
        width: 128,
        ..Seq::default()
    };
    let config = Coded {
        tile_cols: 1,
        ..Coded::default()
    };
    let mut w = config.syntax(seq);
    w.align();
    let mut payload = w.bytes();
    payload.extend_from_slice(&[0xa0, 0, 0x80, 0x80]);
    assert_eq!(
        seq.reader()?.push(obu(&packet(6, &payload))?),
        Err(frame::Error::Malformed)
    );
    let existing = existing(0);
    let payload = obu(&existing)?.payload();
    assert_eq!(
        seq.reader()?.push(obu(&packet(6, payload))?),
        Err(frame::Error::Malformed)
    );
    let mut w = config.syntax(seq);
    w.align();
    let mut payload = w.bytes();
    payload.extend_from_slice(&[0, 255, 0x80]);
    assert_eq!(
        seq.reader()?.push(obu(&packet(6, &payload))?),
        Err(frame::Error::Truncated)
    );
    Ok(())
}

#[test]
fn the_fixed_fuzz_sequence_has_the_same_handwritten_configuration() -> TestResult {
    let bytes = [10, 13, 0, 0, 0, 63, 248, 1, 248, 1, 248, 4, 16, 0, 128];
    assert_eq!(parse_sequence(&bytes)?, Seq::default().parse()?);
    Ok(())
}

#[test]
fn sequence_frame_branches_are_retained_and_unsupported_models_are_refused() -> TestResult {
    let seq = Seq::default();
    let fields = seq.parse()?.frame_syntax();
    assert!(!fields.reduced && !fields.frame_ids && !fields.decoder_model);
    assert!(fields.single_operating_point);
    assert_eq!(
        (
            fields.width_bits,
            fields.height_bits,
            fields.order_hint_bits
        ),
        (16, 16, 3)
    );
    assert_eq!((fields.screen_tools, fields.integer_mv), (0, 2));
    let fields = Seq {
        reduced: true,
        ..seq
    }
    .parse()?
    .frame_syntax();
    assert!(fields.reduced && fields.single_operating_point);
    assert_eq!(
        (
            fields.screen_tools,
            fields.integer_mv,
            fields.order_hint_bits
        ),
        (2, 2, 0)
    );
    let mut ids = seq.syntax();
    ids.replace("frame_ids", &[(1, 1), (4, 0), (3, 0)])?;
    let mut op = seq.syntax();
    op.change("op_idc", 1)?;
    let mut model = seq.syntax();
    model.replace("level", &[(5, 7), (1, 0)])?;
    model.replace(
        "timing",
        &[
            (1, 1),
            (32, 1),
            (32, 30),
            (1, 1),
            (1, 1),
            (1, 1),
            (5, 0),
            (32, 1),
            (5, 0),
            (5, 0),
        ],
    )?;
    for (syntax, reason) in [
        (ids, frame::Unsupported::FrameIds),
        (op, frame::Unsupported::OperatingPoints),
        (model, frame::Unsupported::DecoderModel),
    ] {
        let sequence = parse_sequence(&packet(1, &syntax.bytes()))?;
        assert_eq!(
            frame::Frames::new(sequence, seq.expected(), 64).err(),
            Some(frame::Error::Unsupported(reason))
        );
    }
    Ok(())
}
