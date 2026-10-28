//! The encoder program's H.264 byte stream (ITU-T H.264 Annex B) read back
//! into MP4 samples (`docs/design/transcode.md`, "From Annex B to samples").
//!
//! [`AnnexBReader`] takes the byte stream in chunks of any size
//! ([`AnnexBReader::push`], then [`AnnexBReader::finish`]) and returns the
//! coded pictures in decode order ([`AnnexBReader::pop`]), each as an
//! [`EncodedPicture`]: the access unit's NAL units with 4-byte big-endian
//! length prefixes (an `avc1` sample with `lengthSizeMinusOne` 3), whether
//! it is an IDR picture (a sync sample), and its index in presentation
//! order. [`AnnexBReader::config`] gives the stream's [`Config`], with the
//! `avcC` record, once the first coded slice has been read.
//!
//! What is read, all from the specification and in safe Rust:
//!
//! - **NAL units** (Annex B.2): split on 3- and 4-byte start codes, which
//!   may straddle chunks; trailing zero bytes are not part of a unit, and
//!   three zero bytes inside a unit, or bytes before the first start code
//!   other than zero, are errors. Emulation prevention bytes are skipped
//!   only while parsing: a sample keeps each unit as written.
//! - **Access units** (7.4.1.2.3): once the current access unit holds a
//!   coded slice, the next access unit delimiter, sequence or picture
//!   parameter set, SEI, NAL unit of types 14 to 18, or slice with
//!   `first_mb_in_slice` 0 starts a new one. A slice with another
//!   `first_mb_in_slice` must agree with the picture's first slice in the
//!   fields 7.4.1.2.4 compares (otherwise a picture lost its first slice,
//!   which is an error rather than a guess).
//! - **Parameter sets**: the sequence parameter set (7.3.2.1.1, scaling
//!   lists skipped, the cropped size, the VUI as far as
//!   `aspect_ratio_info`) and the picture parameter set as far as the slice
//!   header needs it (7.3.2.2, slice groups included, up to
//!   `redundant_pic_cnt_present_flag`). Those seen before the first slice
//!   are the configuration; a later copy that equals the stored one is
//!   dropped from the sample, and any other later parameter set, or one
//!   that changes before the first slice, is
//!   [`Error::ParameterSetsChanged`]: x264 writes one of each and repeats
//!   them before IDR pictures, and an MP4 sample entry describes one
//!   configuration.
//! - **Slice headers** (7.3.3), through `dec_ref_pic_marking` (with
//!   `ref_pic_list_modification` and `pred_weight_table` skipped over), for
//!   the picture order count fields and
//!   `memory_management_control_operation` 5.
//! - **Picture order counts** (8.2.1, types 0, 1 and 2) of frame pictures;
//!   field pictures and separate colour planes are [`Error::Unsupported`].
//!
//! **Presentation order.** The pictures form periods: an IDR picture, or a
//! picture with `memory_management_control_operation` 5, starts one. A
//! decoder outputs every picture of a period before the first picture of
//! the next (C.4.4 and C.4.5.3 empty the decoded picture buffer there), and
//! within a period in increasing picture order count; a picture with
//! operation 5 has its order count reset to 0 (8.2.1), so it is the new
//! period's, not the old one's. So a picture's presentation index is the
//! number of pictures in earlier periods plus its rank by order count in
//! its own; two pictures of one period with the same order count are an
//! error. An IDR picture with `no_output_of_prior_pics_flag` 1, other than
//! the first picture, is an error: a decoder would never output the
//! pictures before it.
//!
//! Pictures are held until their period is closed (by the next period's
//! first picture, or by [`AnnexBReader::finish`]), because only then is
//! every rank known; [`Limits`] caps what one open period may hold. A
//! decoded picture buffer model with the VUI's `num_reorder_frames` could
//! release them earlier, but it would rest on optional VUI fields and on
//! the encoder's own bound being right, and x264's periods are short (an
//! IDR picture at least every `--keyint`, 250 frames by default); an
//! open-GOP stream (`--open-gop`) is one period, bounded by the same caps.
//!
//! **`avcC`** (ISO/IEC 14496-15 5.3.3.1): version 1, the profile,
//! compatibility flags and level of the sequence parameter set the first
//! slice uses, 4-byte lengths, every stored sequence and picture parameter
//! set in id order, and, for every `profile_idc` other than 66, 77 and 88
//! (High, 100, is x264's common output; 110, 122 and 244 its 10-bit, 4:2:2
//! and 4:4:4 ones), `chroma_format`, the bit depths and no sequence
//! parameter set extensions.

use std::collections::VecDeque;
use std::fmt;

use Error::Invalid;

/// What the reader may hold; every violation is an [`Error::Limit`].
///
/// The parameter set id ranges (32 sequence and 256 picture parameter
/// sets) are fixed by the syntax and checked as such.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// The largest NAL unit, without its start code: 128 MiB, the container
    /// limit for a packet.
    pub max_nal_unit_bytes: usize,
    /// The largest access unit as a sample (NAL units with their 4-byte
    /// lengths, parameter sets removed): 128 MiB.
    pub max_access_unit_bytes: usize,
    /// NAL units in one access unit, dropped parameter sets included: 1,024.
    pub max_nal_units_per_access_unit: usize,
    /// Pictures held in one open period: 65,536 (x264 writes an IDR picture
    /// at least every 250 frames by default).
    pub max_period_pictures: usize,
    /// Sample bytes held in one open period: 1 GiB.
    pub max_period_bytes: u64,
    /// Pictures in the whole stream: the number of frames sent to the
    /// encoder, which can return no more. No limit by default; the caller
    /// sets it, or raises it as it sends frames
    /// ([`AnnexBReader::set_max_pictures`]).
    pub max_pictures: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_nal_unit_bytes: 128 << 20,
            max_access_unit_bytes: 128 << 20,
            max_nal_units_per_access_unit: 1024,
            max_period_pictures: 65_536,
            max_period_bytes: 1 << 30,
            max_pictures: u64::MAX,
        }
    }
}

/// Why the byte stream cannot be read. Once [`AnnexBReader::push`] or
/// [`AnnexBReader::finish`] has failed, every later call fails with
/// [`Error::Closed`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// Not a valid H.264 byte stream: the rule broken.
    Invalid(&'static str),
    /// Valid H.264 this reader does not take (field pictures, data
    /// partitioning, scalable or multiview layers, and similar).
    Unsupported(&'static str),
    /// A parameter set that differs from the stored one with its id, or a
    /// new one after the first coded slice.
    ParameterSetsChanged,
    /// A [`Limits`] value or a field of the `avcC` record was exceeded.
    Limit(&'static str),
    /// The reader failed earlier or was finished.
    Closed,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(what) => write!(f, "invalid H.264 byte stream: {what}"),
            Self::Unsupported(what) => write!(f, "unsupported H.264 byte stream: {what}"),
            Self::ParameterSetsChanged => {
                f.write_str("the encoder's parameter sets changed within the stream")
            }
            Self::Limit(what) => write!(f, "H.264 byte stream over a limit: {what}"),
            Self::Closed => f.write_str("the H.264 byte stream reader is closed"),
        }
    }
}

impl std::error::Error for Error {}

/// One coded picture: an access unit as an MP4 sample.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodedPicture {
    /// The access unit's NAL units in stream order, each with a 4-byte
    /// big-endian length instead of its start code, and without its
    /// sequence and picture parameter sets (they equal the ones in `avcC`).
    pub data: Vec<u8>,
    /// An IDR picture: a sync sample. Other I pictures (an open GOP's
    /// recovery points) are not.
    pub idr: bool,
    /// The picture's index in presentation order over the whole stream,
    /// from 0.
    pub presentation: u64,
}

/// The stream's configuration, from the parameter sets before the first
/// coded slice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// The `AVCDecoderConfigurationRecord` (ISO/IEC 14496-15 5.3.3.1).
    pub avcc: Vec<u8>,
    /// Width in luma samples after the frame cropping.
    pub width: u32,
    /// Height in luma samples after the frame cropping.
    pub height: u32,
    /// `profile_idc` of the sequence parameter set the first slice uses.
    pub profile_idc: u8,
    /// The byte of `constraint_set0_flag` to `reserved_zero_2bits`.
    pub constraint_flags: u8,
    /// `level_idc`.
    pub level_idc: u8,
    /// `chroma_format_idc`: 0 monochrome, 1 4:2:0, 2 4:2:2, 3 4:4:4.
    pub chroma_format_idc: u8,
    /// Luma bit depth, 8 to 14.
    pub bit_depth_luma: u8,
    /// Chroma bit depth, 8 to 14.
    pub bit_depth_chroma: u8,
    /// Sample aspect ratio (horizontal, vertical) from the VUI, when it is
    /// stated and not "unspecified".
    pub sample_aspect_ratio: Option<(u16, u16)>,
}

/// Reads an H.264 Annex B byte stream incrementally into [`EncodedPicture`]s.
#[derive(Debug)]
pub struct AnnexBReader {
    limits: Limits,
    /// The NAL unit being read, possibly with up to two trailing zero bytes.
    nal: Vec<u8>,
    /// Zero bytes just read.
    zeros: usize,
    /// A start code has been read.
    started: bool,
    closed: bool,
    sps: Vec<Option<Sps>>,
    pps: Vec<Option<Pps>>,
    config: Option<Config>,
    /// The sequence parameter set id of the first slice.
    active_sps: Option<u32>,
    au: AccessUnit,
    poc: PocState,
    period: Vec<Held>,
    period_bytes: u64,
    /// Pictures in closed periods.
    presented_before: u64,
    pictures: u64,
    ready: VecDeque<EncodedPicture>,
}

#[derive(Debug, Default)]
struct AccessUnit {
    data: Vec<u8>,
    nal_units: usize,
    /// The first slice's picture fields, once the unit holds a slice.
    picture: Option<PictureKey>,
    poc: i64,
    idr: bool,
    mmco5: bool,
}

#[derive(Debug)]
struct Held {
    data: Vec<u8>,
    poc: i64,
    idr: bool,
}

impl AnnexBReader {
    /// A reader with these limits.
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            nal: Vec::new(),
            zeros: 0,
            started: false,
            closed: false,
            sps: vec![None; 32],
            pps: vec![None; 256],
            config: None,
            active_sps: None,
            au: AccessUnit::default(),
            poc: PocState::default(),
            period: Vec::new(),
            period_bytes: 0,
            presented_before: 0,
            pictures: 0,
            ready: VecDeque::new(),
        }
    }

    /// Sets [`Limits::max_pictures`], for example to the frames sent so
    /// far: a picture that starts above it is an error.
    pub fn set_max_pictures(&mut self, max: u64) {
        self.limits.max_pictures = max;
    }

    /// The configuration, once the first coded slice has been read.
    pub fn config(&self) -> Option<&Config> {
        self.config.as_ref()
    }

    /// Coded pictures started so far.
    pub fn pictures(&self) -> u64 {
        self.pictures
    }

    /// Reads the next bytes of the stream.
    pub fn push(&mut self, bytes: &[u8]) -> Result<(), Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        let result = self.push_bytes(bytes);
        if result.is_err() {
            self.closed = true;
        }
        result
    }

    /// Ends the stream: its last NAL unit, access unit and period. Every
    /// picture can then be popped.
    pub fn finish(&mut self) -> Result<(), Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        self.closed = true;
        if self.started {
            self.end_nal()?;
        }
        if self.au.picture.is_some() {
            self.end_access_unit()?;
        } else if !self.au.data.is_empty() {
            return Err(Error::Invalid("NAL units after the last coded picture"));
        }
        self.close_period()
    }

    /// The next picture in decode order whose presentation index is known.
    pub fn pop(&mut self) -> Option<EncodedPicture> {
        self.ready.pop_front()
    }

    fn push_bytes(&mut self, mut rest: &[u8]) -> Result<(), Error> {
        while let Some((&byte, tail)) = rest.split_first() {
            if self.zeros == 0 && self.started && byte != 0 {
                // A run of the unit's bytes up to the next zero byte.
                let run = rest.iter().position(|b| *b == 0).unwrap_or(rest.len());
                let (bytes, tail) = rest.split_at(run);
                self.extend_nal(bytes)?;
                rest = tail;
                continue;
            }
            rest = tail;
            if byte == 1 && self.zeros >= 2 {
                if self.started {
                    self.end_nal()?;
                }
                self.started = true;
                self.zeros = 0;
            } else if byte == 0 {
                self.zeros = self.zeros.saturating_add(1);
                // A third zero byte ends the unit (7.4.2 and B.2): the
                // rest, up to the next start code, is trailing zeros.
                if self.started && self.zeros <= 2 {
                    self.extend_nal(&[0])?;
                }
            } else if !self.started {
                return Err(Invalid("bytes before the first start code"));
            } else if self.zeros > 2 {
                return Err(Invalid("three zero bytes inside a NAL unit"));
            } else if self.zeros == 2 && byte == 2 {
                return Err(Invalid("the bytes 0x000002 inside a NAL unit"));
            } else {
                self.zeros = 0;
                self.extend_nal(&[byte])?;
            }
        }
        Ok(())
    }

    fn extend_nal(&mut self, bytes: &[u8]) -> Result<(), Error> {
        // Up to two zero bytes at the end may still turn out to be part of
        // the next start code.
        let max = self.limits.max_nal_unit_bytes.saturating_add(2);
        if bytes.len() > max.saturating_sub(self.nal.len()) {
            return Err(Error::Limit("NAL unit size"));
        }
        self.nal.extend_from_slice(bytes);
        Ok(())
    }

    fn end_nal(&mut self) -> Result<(), Error> {
        while self.nal.last() == Some(&0) {
            self.nal.pop();
        }
        if self.nal.is_empty() {
            return Err(Invalid("an empty NAL unit"));
        }
        if self.nal.len() > self.limits.max_nal_unit_bytes {
            return Err(Error::Limit("NAL unit size"));
        }
        let nal = std::mem::take(&mut self.nal);
        let result = self.nal_unit(&nal);
        // Keep the allocation for the next unit.
        self.nal = nal;
        self.nal.clear();
        result
    }

    fn nal_unit(&mut self, nal: &[u8]) -> Result<(), Error> {
        let Some(&header) = nal.first() else {
            return Err(Invalid("an empty NAL unit"));
        };
        if header & 0x80 != 0 {
            return Err(Invalid("forbidden_zero_bit is set"));
        }
        let ref_idc = (header >> 5) & 3;
        let kind = header & 0x1f;
        if matches!(kind, 7 | 8) && nal.len() > usize::from(u16::MAX) {
            // The avcC record's length field; the 32 + 256 stored sets are
            // held outside the access unit and period caps, so this bounds
            // them (18 MiB in all).
            return Err(Error::Limit("parameter set size"));
        }
        let mut keep = true;
        match kind {
            1 | 5 => {
                let slice = self.slice_header(nal, kind, ref_idc)?;
                self.slice(slice)?;
            }
            7 => {
                self.boundary()?;
                self.sequence_parameter_set(nal)?;
                keep = false;
            }
            8 => {
                self.boundary()?;
                self.picture_parameter_set(nal)?;
                keep = false;
            }
            6 | 9 | 16..=18 => self.boundary()?,
            2..=4 => return Err(Error::Unsupported("slice data partitioning")),
            13 | 19 => return Err(Error::Unsupported("auxiliary coded pictures")),
            14 | 15 | 20 | 21 => {
                return Err(Error::Unsupported("scalable or multiview layers"));
            }
            // End of sequence or stream, filler data, and unspecified or
            // reserved types belong to the current access unit.
            _ => {}
        }
        self.au.nal_units = self.au.nal_units.saturating_add(1);
        if self.au.nal_units > self.limits.max_nal_units_per_access_unit {
            return Err(Error::Limit("NAL units in an access unit"));
        }
        if keep {
            let free = self
                .limits
                .max_access_unit_bytes
                .saturating_sub(self.au.data.len());
            if nal.len().saturating_add(4) > free {
                return Err(Error::Limit("access unit size"));
            }
            let length = u32::try_from(nal.len()).map_err(|_| Error::Limit("NAL unit size"))?;
            self.au.data.extend_from_slice(&length.to_be_bytes());
            self.au.data.extend_from_slice(nal);
        }
        Ok(())
    }

    /// A NAL unit that starts an access unit when the current one holds a
    /// coded slice.
    fn boundary(&mut self) -> Result<(), Error> {
        if self.au.picture.is_some() {
            self.end_access_unit()?;
        }
        Ok(())
    }

    fn sequence_parameter_set(&mut self, nal: &[u8]) -> Result<(), Error> {
        let sps = parse_sps(nal)?;
        let slot = self
            .sps
            .get_mut(sps.id as usize)
            .ok_or(Invalid("seq_parameter_set_id above 31"))?;
        match slot {
            Some(stored) if stored.raw == sps.raw => Ok(()),
            Some(_) => Err(Error::ParameterSetsChanged),
            None if self.config.is_some() => Err(Error::ParameterSetsChanged),
            None => {
                *slot = Some(sps);
                Ok(())
            }
        }
    }

    fn picture_parameter_set(&mut self, nal: &[u8]) -> Result<(), Error> {
        let pps = parse_pps(nal)?;
        let slot = self
            .pps
            .get_mut(pps.id as usize)
            .ok_or(Invalid("pic_parameter_set_id above 255"))?;
        match slot {
            Some(stored) if stored.raw == pps.raw => Ok(()),
            Some(_) => Err(Error::ParameterSetsChanged),
            None if self.config.is_some() => Err(Error::ParameterSetsChanged),
            None => {
                *slot = Some(pps);
                Ok(())
            }
        }
    }

    fn slice_header(&self, nal: &[u8], kind: u8, ref_idc: u8) -> Result<Slice, Error> {
        let mut r = Bits::new(nal.get(1..).unwrap_or_default());
        let first_mb = r.ue()?;
        let slice_type = r.ue()?;
        if slice_type > 9 {
            return Err(Invalid("slice_type above 9"));
        }
        let slice_type = slice_type % 5;
        if kind == 5 && !matches!(slice_type, 2 | 4) {
            return Err(Invalid("an IDR picture with a P or B slice"));
        }
        let pps_id = r.ue()?;
        let pps = self
            .pps
            .get(pps_id as usize)
            .and_then(Option::as_ref)
            .ok_or(Invalid("a slice refers to a missing picture parameter set"))?;
        if self.active_sps.is_some_and(|id| id != pps.sps_id) {
            return Err(Error::ParameterSetsChanged);
        }
        let sps = self
            .sps
            .get(pps.sps_id as usize)
            .and_then(Option::as_ref)
            .ok_or(Invalid(
                "a slice refers to a missing sequence parameter set",
            ))?;
        let idr = kind == 5;
        if idr && ref_idc == 0 {
            return Err(Invalid("an IDR picture with nal_ref_idc 0"));
        }
        let mut key = PictureKey {
            pps_id,
            frame_num: r.bits(sps.log2_max_frame_num)?,
            idr,
            reference: ref_idc != 0,
            idr_pic_id: 0,
            poc_lsb: 0,
            delta_bottom: 0,
            delta: [0, 0],
        };
        if !sps.frame_mbs_only && r.flag()? {
            return Err(Error::Unsupported("field pictures"));
        }
        if idr {
            key.idr_pic_id = r.ue()?;
        }
        match sps.poc {
            PocType::Lsb { log2_max_lsb } => {
                key.poc_lsb = r.bits(log2_max_lsb)?;
                if pps.bottom_field_pic_order {
                    key.delta_bottom = r.se()?;
                }
            }
            PocType::Cycle {
                delta_always_zero: false,
                ..
            } => {
                key.delta[0] = r.se()?;
                if pps.bottom_field_pic_order {
                    key.delta[1] = r.se()?;
                }
            }
            PocType::Cycle { .. } | PocType::FrameNum => {}
        }
        if pps.redundant_pic_cnt_present && r.ue()? != 0 {
            return Err(Error::Unsupported("redundant coded pictures"));
        }
        let (p, b) = (matches!(slice_type, 0 | 3), slice_type == 1);
        if b {
            r.flag()?; // direct_spatial_mv_pred_flag
        }
        let (mut l0, mut l1) = (pps.num_ref_idx_default[0], pps.num_ref_idx_default[1]);
        if (p || b) && r.flag()? {
            l0 = r.ue()?;
            if b {
                l1 = r.ue()?;
            }
        }
        // Frame pictures only: at most 16 references a list (7.4.3).
        if ((p || b) && l0 > 15) || (b && l1 > 15) {
            return Err(Invalid("num_ref_idx_active_minus1 above 15"));
        }
        // ref_pic_list_modification (7.3.3.1).
        if p || b {
            ref_pic_list_modification(&mut r, l0)?;
        }
        if b {
            ref_pic_list_modification(&mut r, l1)?;
        }
        if (pps.weighted_pred && p) || (pps.weighted_bipred_idc == 1 && b) {
            pred_weight_table(&mut r, sps.chroma_array_type != 0, l0, b.then_some(l1))?;
        }
        let (mut mmco5, mut no_output_of_prior_pics) = (false, false);
        if ref_idc != 0 {
            // dec_ref_pic_marking (7.3.3.3).
            if idr {
                no_output_of_prior_pics = r.flag()?;
                r.flag()?; // long_term_reference_flag
            } else if r.flag()? {
                loop {
                    match r.ue()? {
                        0 => break,
                        1 | 2 | 4 | 6 => {
                            r.ue()?;
                        }
                        3 => {
                            r.ue()?;
                            r.ue()?;
                        }
                        5 => mmco5 = true,
                        _ => {
                            return Err(Invalid("memory_management_control_operation above 6"));
                        }
                    }
                }
            }
        }
        Ok(Slice {
            first_mb,
            sps_id: pps.sps_id,
            key,
            mmco5,
            no_output_of_prior_pics,
        })
    }

    fn slice(&mut self, slice: Slice) -> Result<(), Error> {
        if self.au.picture.is_some() && slice.first_mb == 0 {
            self.end_access_unit()?;
        }
        if let Some(first) = &self.au.picture {
            if *first != slice.key {
                return Err(Invalid(
                    "a slice of a picture without its first slice (first_mb_in_slice 0)",
                ));
            }
            if slice.mmco5 != self.au.mmco5 {
                return Err(Invalid(
                    "memory_management_control_operation 5 in some slices of a picture only",
                ));
            }
            return Ok(());
        }
        if slice.first_mb != 0 {
            return Err(Error::Unsupported(
                "a picture whose first slice does not start at macroblock 0",
            ));
        }
        if self.config.is_none() {
            self.config = Some(self.build_config(slice.sps_id)?);
            self.active_sps = Some(slice.sps_id);
        }
        if slice.no_output_of_prior_pics && self.pictures > 0 {
            // The pictures before it would never be output (C.4.4).
            return Err(Error::Unsupported(
                "an IDR picture with no_output_of_prior_pics_flag 1",
            ));
        }
        self.pictures = self.pictures.saturating_add(1);
        if self.pictures > self.limits.max_pictures {
            return Err(Error::Limit("more pictures than frames sent"));
        }
        let sps = self
            .sps
            .get(slice.sps_id as usize)
            .and_then(Option::as_ref)
            .ok_or(Invalid(
                "a slice refers to a missing sequence parameter set",
            ))?;
        let poc = self.poc.next(sps, &slice)?;
        if slice.key.idr || slice.mmco5 {
            self.close_period()?;
        }
        self.au.picture = Some(slice.key);
        self.au.poc = poc;
        self.au.idr = slice.key.idr;
        self.au.mmco5 = slice.mmco5;
        Ok(())
    }

    fn end_access_unit(&mut self) -> Result<(), Error> {
        let au = std::mem::take(&mut self.au);
        if self.period.len() >= self.limits.max_period_pictures {
            return Err(Error::Limit("pictures in one period"));
        }
        let bytes = u64::try_from(au.data.len()).unwrap_or(u64::MAX);
        self.period_bytes = self.period_bytes.saturating_add(bytes);
        if self.period_bytes > self.limits.max_period_bytes {
            return Err(Error::Limit("bytes in one period"));
        }
        self.period.push(Held {
            data: au.data,
            poc: au.poc,
            idr: au.idr,
        });
        Ok(())
    }

    fn close_period(&mut self) -> Result<(), Error> {
        let mut by_poc: Vec<(i64, usize)> = self
            .period
            .iter()
            .enumerate()
            .map(|(k, held)| (held.poc, k))
            .collect();
        by_poc.sort_unstable();
        if by_poc.windows(2).any(|w| matches!(w, [a, b] if a.0 == b.0)) {
            return Err(Invalid("two pictures of one period with one order count"));
        }
        let mut rank = vec![0u64; by_poc.len()];
        for (r, (_, k)) in (0u64..).zip(&by_poc) {
            if let Some(slot) = rank.get_mut(*k) {
                *slot = r;
            }
        }
        for (held, r) in self.period.drain(..).zip(rank) {
            self.ready.push_back(EncodedPicture {
                data: held.data,
                idr: held.idr,
                presentation: self.presented_before.saturating_add(r),
            });
        }
        let count = u64::try_from(by_poc.len()).unwrap_or(u64::MAX);
        self.presented_before = self.presented_before.saturating_add(count);
        self.period_bytes = 0;
        Ok(())
    }

    fn build_config(&self, active: u32) -> Result<Config, Error> {
        let sps = self
            .sps
            .get(active as usize)
            .and_then(Option::as_ref)
            .ok_or(Invalid(
                "a slice refers to a missing sequence parameter set",
            ))?;
        let all_sps: Vec<&Sps> = self.sps.iter().flatten().collect();
        let all_pps: Vec<&Pps> = self.pps.iter().flatten().collect();
        let sps_count = u8::try_from(all_sps.len())
            .ok()
            .filter(|n| *n <= 31)
            .ok_or(Error::Limit(
                "more than 31 sequence parameter sets for avcC",
            ))?;
        let pps_count = u8::try_from(all_pps.len())
            .map_err(|_| Error::Limit("more than 255 picture parameter sets for avcC"))?;
        let mut avcc = vec![
            1,
            sps.profile_idc,
            sps.constraint_flags,
            sps.level_idc,
            0xFC | 3, // lengthSizeMinusOne: 4-byte lengths
            0xE0 | sps_count,
        ];
        for set in &all_sps {
            push_parameter_set(&mut avcc, &set.raw)?;
        }
        avcc.push(pps_count);
        for set in &all_pps {
            push_parameter_set(&mut avcc, &set.raw)?;
        }
        if !matches!(sps.profile_idc, 66 | 77 | 88) {
            avcc.push(0xFC | sps.chroma_format_idc);
            avcc.push(0xF8 | (sps.bit_depth_luma - 8));
            avcc.push(0xF8 | (sps.bit_depth_chroma - 8));
            avcc.push(0); // numOfSequenceParameterSetExt
        }
        Ok(Config {
            avcc,
            width: sps.width,
            height: sps.height,
            profile_idc: sps.profile_idc,
            constraint_flags: sps.constraint_flags,
            level_idc: sps.level_idc,
            chroma_format_idc: sps.chroma_format_idc,
            bit_depth_luma: sps.bit_depth_luma,
            bit_depth_chroma: sps.bit_depth_chroma,
            sample_aspect_ratio: sps.sample_aspect_ratio,
        })
    }
}

fn push_parameter_set(avcc: &mut Vec<u8>, raw: &[u8]) -> Result<(), Error> {
    let length = u16::try_from(raw.len())
        .map_err(|_| Error::Limit("a parameter set longer than avcC can hold"))?;
    avcc.extend_from_slice(&length.to_be_bytes());
    avcc.extend_from_slice(raw);
    Ok(())
}

/// The fields 7.4.1.2.4 compares between the slices of one picture (those
/// this reader reads).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PictureKey {
    pps_id: u32,
    frame_num: u32,
    idr: bool,
    /// `nal_ref_idc` is not 0.
    reference: bool,
    idr_pic_id: u32,
    poc_lsb: u32,
    delta_bottom: i32,
    delta: [i32; 2],
}

#[derive(Clone, Copy, Debug)]
struct Slice {
    first_mb: u32,
    sps_id: u32,
    key: PictureKey,
    mmco5: bool,
    no_output_of_prior_pics: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PocType {
    /// Type 0.
    Lsb { log2_max_lsb: u32 },
    /// Type 1.
    Cycle {
        delta_always_zero: bool,
        offset_for_non_ref_pic: i32,
        offset_for_top_to_bottom_field: i32,
        offset_for_ref_frame: Vec<i32>,
    },
    /// Type 2.
    FrameNum,
}

/// A sequence parameter set, as far as this reader needs it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Sps {
    raw: Vec<u8>,
    id: u32,
    profile_idc: u8,
    constraint_flags: u8,
    level_idc: u8,
    chroma_format_idc: u8,
    chroma_array_type: u8,
    bit_depth_luma: u8,
    bit_depth_chroma: u8,
    log2_max_frame_num: u32,
    poc: PocType,
    frame_mbs_only: bool,
    width: u32,
    height: u32,
    sample_aspect_ratio: Option<(u16, u16)>,
}

/// The profiles whose sequence parameter sets carry `chroma_format_idc`
/// and the bit depths (7.3.2.1.1).
const HIGH_PROFILES: [u8; 13] = [100, 110, 122, 244, 44, 83, 86, 118, 128, 138, 139, 134, 135];

/// Table E-1, `aspect_ratio_idc` 1 to 16.
const SAMPLE_ASPECT_RATIOS: [(u16, u16); 16] = [
    (1, 1),
    (12, 11),
    (10, 11),
    (16, 11),
    (40, 33),
    (24, 11),
    (20, 11),
    (32, 11),
    (80, 33),
    (18, 11),
    (15, 11),
    (64, 33),
    (160, 99),
    (4, 3),
    (3, 2),
    (2, 1),
];

fn parse_sps(nal: &[u8]) -> Result<Sps, Error> {
    let mut r = Bits::new(nal.get(1..).unwrap_or_default());
    let profile_idc = r.byte()?;
    let constraint_flags = r.byte()?;
    let level_idc = r.byte()?;
    let id = r.ue()?;
    if id > 31 {
        return Err(Invalid("seq_parameter_set_id above 31"));
    }
    let (mut chroma_format_idc, mut depth_luma, mut depth_chroma) = (1, 8, 8);
    if HIGH_PROFILES.contains(&profile_idc) {
        chroma_format_idc = r.ue()?;
        if chroma_format_idc > 3 {
            return Err(Invalid("chroma_format_idc above 3"));
        }
        if chroma_format_idc == 3 && r.flag()? {
            return Err(Error::Unsupported("separate colour planes"));
        }
        depth_luma = r.ue()?.saturating_add(8);
        depth_chroma = r.ue()?.saturating_add(8);
        if depth_luma > 14 || depth_chroma > 14 {
            return Err(Invalid("bit depth above 14"));
        }
        r.flag()?; // qpprime_y_zero_transform_bypass_flag
        if r.flag()? {
            let lists = if chroma_format_idc == 3 { 12 } else { 8 };
            for i in 0..lists {
                if r.flag()? {
                    scaling_list(&mut r, if i < 6 { 16 } else { 64 })?;
                }
            }
        }
    }
    let log2_max_frame_num = r.ue()?.saturating_add(4);
    if log2_max_frame_num > 16 {
        return Err(Invalid("log2_max_frame_num_minus4 above 12"));
    }
    let poc = match r.ue()? {
        0 => {
            let log2_max_lsb = r.ue()?.saturating_add(4);
            if log2_max_lsb > 16 {
                return Err(Invalid("log2_max_pic_order_cnt_lsb_minus4 above 12"));
            }
            PocType::Lsb { log2_max_lsb }
        }
        1 => {
            let delta_always_zero = r.flag()?;
            let offset_for_non_ref_pic = r.se()?;
            let offset_for_top_to_bottom_field = r.se()?;
            let cycle = r.ue()?;
            if cycle > 255 {
                return Err(Invalid("num_ref_frames_in_pic_order_cnt_cycle above 255"));
            }
            let mut offset_for_ref_frame = Vec::with_capacity(cycle as usize);
            for _ in 0..cycle {
                offset_for_ref_frame.push(r.se()?);
            }
            PocType::Cycle {
                delta_always_zero,
                offset_for_non_ref_pic,
                offset_for_top_to_bottom_field,
                offset_for_ref_frame,
            }
        }
        2 => PocType::FrameNum,
        _ => return Err(Invalid("pic_order_cnt_type above 2")),
    };
    r.ue()?; // max_num_ref_frames
    r.flag()?; // gaps_in_frame_num_value_allowed_flag
    let width_mbs = u64::from(r.ue()?) + 1;
    let height_map_units = u64::from(r.ue()?) + 1;
    let frame_mbs_only = r.flag()?;
    if !frame_mbs_only {
        r.flag()?; // mb_adaptive_frame_field_flag
    }
    r.flag()?; // direct_8x8_inference_flag
    let mut crop = [0u64; 4];
    if r.flag()? {
        for edge in &mut crop {
            *edge = u64::from(r.ue()?);
        }
    }
    let chroma_array_type = chroma_format_idc;
    let field_factor = if frame_mbs_only { 1 } else { 2 };
    let (unit_x, unit_y) = match chroma_array_type {
        0 => (1, field_factor),
        1 => (2, 2 * field_factor),
        2 => (2, field_factor),
        _ => (1, field_factor),
    };
    let [left, right, top, bottom] = crop;
    let width = (width_mbs * 16)
        .checked_sub((left + right) * unit_x)
        .filter(|w| *w > 0)
        .ok_or(Invalid("frame cropping wider than the frame"))?;
    let height = (height_map_units * field_factor * 16)
        .checked_sub((top + bottom) * unit_y)
        .filter(|h| *h > 0)
        .ok_or(Invalid("frame cropping taller than the frame"))?;
    let width = u32::try_from(width).map_err(|_| Error::Unsupported("frame width"))?;
    let height = u32::try_from(height).map_err(|_| Error::Unsupported("frame height"))?;
    let mut sample_aspect_ratio = None;
    if r.flag()? && r.flag()? {
        // vui_parameters (E.1.1): aspect_ratio_info only.
        sample_aspect_ratio = match r.byte()? {
            255 => {
                let w = r.bits(16)?;
                let h = r.bits(16)?;
                match (u16::try_from(w), u16::try_from(h)) {
                    (Ok(w), Ok(h)) if w != 0 && h != 0 => Some((w, h)),
                    _ => None,
                }
            }
            idc => SAMPLE_ASPECT_RATIOS
                .get(usize::from(idc).wrapping_sub(1))
                .copied(),
        };
    }
    Ok(Sps {
        raw: nal.to_vec(),
        id,
        profile_idc,
        constraint_flags,
        level_idc,
        chroma_format_idc: small(chroma_format_idc),
        chroma_array_type: small(chroma_array_type),
        bit_depth_luma: small(depth_luma),
        bit_depth_chroma: small(depth_chroma),
        log2_max_frame_num,
        poc,
        frame_mbs_only,
        width,
        height,
        sample_aspect_ratio,
    })
}

/// A value already checked to be small.
fn small(value: u32) -> u8 {
    u8::try_from(value).unwrap_or(u8::MAX)
}

/// Skips `scaling_list` (7.3.2.1.1.1).
fn scaling_list(r: &mut Bits<'_>, size: usize) -> Result<(), Error> {
    let (mut last, mut next) = (8i32, 8i32);
    for _ in 0..size {
        if next != 0 {
            let delta = r.se()?;
            if !(-128..=127).contains(&delta) {
                return Err(Invalid("delta_scale outside -128 to 127"));
            }
            next = (last + delta + 256).rem_euclid(256);
        }
        if next != 0 {
            last = next;
        }
    }
    Ok(())
}

/// A picture parameter set, as far as the slice header needs it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Pps {
    raw: Vec<u8>,
    id: u32,
    sps_id: u32,
    bottom_field_pic_order: bool,
    num_ref_idx_default: [u32; 2],
    weighted_pred: bool,
    weighted_bipred_idc: u32,
    redundant_pic_cnt_present: bool,
}

fn parse_pps(nal: &[u8]) -> Result<Pps, Error> {
    let mut r = Bits::new(nal.get(1..).unwrap_or_default());
    let id = r.ue()?;
    if id > 255 {
        return Err(Invalid("pic_parameter_set_id above 255"));
    }
    let sps_id = r.ue()?;
    if sps_id > 31 {
        return Err(Invalid("seq_parameter_set_id above 31"));
    }
    r.flag()?; // entropy_coding_mode_flag
    let bottom_field_pic_order = r.flag()?;
    let groups_minus1 = r.ue()?;
    if groups_minus1 > 7 {
        return Err(Invalid("num_slice_groups_minus1 above 7"));
    }
    if groups_minus1 > 0 {
        match r.ue()? {
            0 => {
                for _ in 0..=groups_minus1 {
                    r.ue()?; // run_length_minus1
                }
            }
            1 => {}
            2 => {
                for _ in 0..groups_minus1 {
                    r.ue()?; // top_left
                    r.ue()?; // bottom_right
                }
            }
            3..=5 => {
                r.flag()?; // slice_group_change_direction_flag
                r.ue()?; // slice_group_change_rate_minus1
            }
            6 => {
                let units = u64::from(r.ue()?) + 1;
                // Ceil(Log2(num_slice_groups_minus1 + 1)) bits each.
                let bits = 32 - groups_minus1.leading_zeros();
                if units.saturating_mul(u64::from(bits)) > r.remaining() {
                    return Err(Invalid("slice_group_id past the end of its NAL unit"));
                }
                for _ in 0..units {
                    r.bits(bits)?;
                }
            }
            _ => return Err(Invalid("slice_group_map_type above 6")),
        }
    }
    let l0 = r.ue()?;
    let l1 = r.ue()?;
    if l0 > 31 || l1 > 31 {
        return Err(Invalid("num_ref_idx_default_active_minus1 above 31"));
    }
    let weighted_pred = r.flag()?;
    let weighted_bipred_idc = r.bits(2)?;
    if weighted_bipred_idc > 2 {
        return Err(Invalid("weighted_bipred_idc 3"));
    }
    r.se()?; // pic_init_qp_minus26
    r.se()?; // pic_init_qs_minus26
    r.se()?; // chroma_qp_index_offset
    r.flag()?; // deblocking_filter_control_present_flag
    r.flag()?; // constrained_intra_pred_flag
    let redundant_pic_cnt_present = r.flag()?;
    Ok(Pps {
        raw: nal.to_vec(),
        id,
        sps_id,
        bottom_field_pic_order,
        num_ref_idx_default: [l0, l1],
        weighted_pred,
        weighted_bipred_idc,
        redundant_pic_cnt_present,
    })
}

/// Skips one list of `ref_pic_list_modification` (7.3.3.1).
fn ref_pic_list_modification(r: &mut Bits<'_>, active_minus1: u32) -> Result<(), Error> {
    if !r.flag()? {
        return Ok(());
    }
    // At most num_ref_idx_active entries before the closing 3 (7.4.3.1).
    for _ in 0..=active_minus1.saturating_add(1) {
        match r.ue()? {
            0..=2 => {
                r.ue()?;
            }
            3 => return Ok(()),
            _ => return Err(Invalid("modification_of_pic_nums_idc above 3")),
        }
    }
    Err(Invalid("more reference list modifications than references"))
}

/// Skips `pred_weight_table` (7.3.3.2).
fn pred_weight_table(
    r: &mut Bits<'_>,
    chroma: bool,
    l0: u32,
    l1: Option<u32>,
) -> Result<(), Error> {
    r.ue()?; // luma_log2_weight_denom
    if chroma {
        r.ue()?; // chroma_log2_weight_denom
    }
    for active_minus1 in std::iter::once(l0).chain(l1) {
        for _ in 0..=active_minus1 {
            if r.flag()? {
                r.se()?;
                r.se()?;
            }
            if chroma && r.flag()? {
                for _ in 0..4 {
                    r.se()?;
                }
            }
        }
    }
    Ok(())
}

/// The picture order count state of 8.2.1 between pictures.
#[derive(Debug, Default)]
struct PocState {
    /// `prevPicOrderCntMsb` and `prevPicOrderCntLsb` (type 0): of the
    /// previous reference picture, after its operation 5 if any.
    prev_msb: i64,
    prev_lsb: i64,
    /// `prevFrameNumOffset` and `prevFrameNum` (types 1 and 2): of the
    /// previous picture, after its operation 5 if any.
    prev_frame_num_offset: i64,
    prev_frame_num: i64,
}

const OVERFLOW: Error = Invalid("a picture order count out of range");

impl PocState {
    /// The picture order count of the next frame picture in decode order,
    /// `Min(TopFieldOrderCnt, BottomFieldOrderCnt)` (8.2.1), with the state
    /// updated for the picture after it. A picture with operation 5 gets 0,
    /// its count after the reset.
    fn next(&mut self, sps: &Sps, slice: &Slice) -> Result<i64, Error> {
        let key = &slice.key;
        let frame_num = i64::from(key.frame_num);
        let max_frame_num = 1i64 << sps.log2_max_frame_num;
        // FrameNumOffset (8.2.1.2 and 8.2.1.3).
        let frame_num_offset = if key.idr {
            0
        } else if self.prev_frame_num > frame_num {
            self.prev_frame_num_offset
                .checked_add(max_frame_num)
                .ok_or(OVERFLOW)?
        } else {
            self.prev_frame_num_offset
        };
        let (top, bottom) = match &sps.poc {
            PocType::Lsb { log2_max_lsb } => {
                // 8.2.1.1.
                let (prev_msb, prev_lsb) = if key.idr {
                    (0, 0)
                } else {
                    (self.prev_msb, self.prev_lsb)
                };
                let max_lsb = 1i64 << log2_max_lsb;
                let lsb = i64::from(key.poc_lsb);
                let msb = if lsb < prev_lsb && prev_lsb - lsb >= max_lsb / 2 {
                    prev_msb.checked_add(max_lsb)
                } else if lsb > prev_lsb && lsb - prev_lsb > max_lsb / 2 {
                    prev_msb.checked_sub(max_lsb)
                } else {
                    Some(prev_msb)
                }
                .ok_or(OVERFLOW)?;
                let top = msb.checked_add(lsb).ok_or(OVERFLOW)?;
                let bottom = top
                    .checked_add(i64::from(key.delta_bottom))
                    .ok_or(OVERFLOW)?;
                if key.reference {
                    if slice.mmco5 {
                        // The frame's top field count after the reset.
                        self.prev_msb = 0;
                        self.prev_lsb = top.checked_sub(top.min(bottom)).ok_or(OVERFLOW)?;
                    } else {
                        self.prev_msb = msb;
                        self.prev_lsb = lsb;
                    }
                }
                (top, bottom)
            }
            PocType::Cycle {
                offset_for_non_ref_pic,
                offset_for_top_to_bottom_field,
                offset_for_ref_frame,
                ..
            } => {
                // 8.2.1.2.
                let cycle = i64::try_from(offset_for_ref_frame.len()).map_err(|_| OVERFLOW)?;
                let mut abs_frame_num = if cycle != 0 {
                    frame_num_offset.checked_add(frame_num).ok_or(OVERFLOW)?
                } else {
                    0
                };
                if !key.reference && abs_frame_num > 0 {
                    abs_frame_num -= 1;
                }
                let mut expected = 0i64;
                if abs_frame_num > 0 {
                    let cycle_count = (abs_frame_num - 1) / cycle;
                    let in_cycle = (abs_frame_num - 1) % cycle;
                    let per_cycle = offset_for_ref_frame
                        .iter()
                        .try_fold(0i64, |sum, o| sum.checked_add(i64::from(*o)))
                        .ok_or(OVERFLOW)?;
                    expected = cycle_count.checked_mul(per_cycle).ok_or(OVERFLOW)?;
                    let in_cycle = usize::try_from(in_cycle).map_err(|_| OVERFLOW)?;
                    for o in offset_for_ref_frame.iter().take(in_cycle.saturating_add(1)) {
                        expected = expected.checked_add(i64::from(*o)).ok_or(OVERFLOW)?;
                    }
                }
                if !key.reference {
                    expected = expected
                        .checked_add(i64::from(*offset_for_non_ref_pic))
                        .ok_or(OVERFLOW)?;
                }
                let top = expected
                    .checked_add(i64::from(key.delta[0]))
                    .ok_or(OVERFLOW)?;
                let bottom = top
                    .checked_add(i64::from(*offset_for_top_to_bottom_field))
                    .and_then(|b| b.checked_add(i64::from(key.delta[1])))
                    .ok_or(OVERFLOW)?;
                (top, bottom)
            }
            PocType::FrameNum => {
                // 8.2.1.3.
                let count = if key.idr {
                    0
                } else {
                    let doubled = frame_num_offset
                        .checked_add(frame_num)
                        .and_then(|n| n.checked_mul(2))
                        .ok_or(OVERFLOW)?;
                    if key.reference { doubled } else { doubled - 1 }
                };
                (count, count)
            }
        };
        if slice.mmco5 {
            // The picture is inferred to have had frame_num 0 (7.4.3).
            self.prev_frame_num_offset = 0;
            self.prev_frame_num = 0;
            Ok(0)
        } else {
            self.prev_frame_num_offset = frame_num_offset;
            self.prev_frame_num = frame_num;
            Ok(top.min(bottom))
        }
    }
}

/// Reads RBSP bits from a NAL unit payload, skipping emulation prevention
/// bytes (7.4.1). Every read past the end is an error.
struct Bits<'a> {
    data: &'a [u8],
    at: usize,
    zeros: u8,
    byte: u8,
    left: u8,
}

const TRUNCATED: Error = Invalid("a syntax element past the end of its NAL unit");

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            at: 0,
            zeros: 0,
            byte: 0,
            left: 0,
        }
    }

    /// An upper bound of the bits left.
    fn remaining(&self) -> u64 {
        let bytes = self.data.len().saturating_sub(self.at);
        u64::try_from(bytes)
            .unwrap_or(u64::MAX)
            .saturating_mul(8)
            .saturating_add(u64::from(self.left))
    }

    fn flag(&mut self) -> Result<bool, Error> {
        if self.left == 0 {
            loop {
                let byte = *self.data.get(self.at).ok_or(TRUNCATED)?;
                self.at += 1;
                if self.zeros >= 2 && byte == 3 {
                    self.zeros = 0;
                    continue;
                }
                self.zeros = if byte == 0 {
                    self.zeros.saturating_add(1)
                } else {
                    0
                };
                self.byte = byte;
                self.left = 8;
                break;
            }
        }
        self.left -= 1;
        Ok((self.byte >> self.left) & 1 == 1)
    }

    /// `u(n)`, `n` at most 32.
    fn bits(&mut self, n: u32) -> Result<u32, Error> {
        let mut value = 0u64;
        for _ in 0..n.min(32) {
            value = (value << 1) | u64::from(self.flag()?);
        }
        u32::try_from(value).map_err(|_| TRUNCATED)
    }

    fn byte(&mut self) -> Result<u8, Error> {
        u8::try_from(self.bits(8)?).map_err(|_| TRUNCATED)
    }

    /// `ue(v)` (9.1), at most 2^32 − 2.
    fn ue(&mut self) -> Result<u32, Error> {
        let mut zeros = 0u32;
        while !self.flag()? {
            zeros += 1;
            if zeros > 31 {
                return Err(Invalid("an Exp-Golomb code longer than 32 bits"));
            }
        }
        let value = (1u64 << zeros) - 1 + u64::from(self.bits(zeros)?);
        u32::try_from(value).map_err(|_| Invalid("an Exp-Golomb code longer than 32 bits"))
    }

    /// `se(v)` (9.1.1).
    fn se(&mut self) -> Result<i32, Error> {
        let k = i64::from(self.ue()?);
        let value = if k % 2 == 1 { (k + 1) / 2 } else { -(k / 2) };
        i32::try_from(value).map_err(|_| Invalid("an Exp-Golomb code longer than 32 bits"))
    }
}

#[cfg(test)]
mod tests;
