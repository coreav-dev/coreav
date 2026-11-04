use crate::{
    Error, Limits, NalHeader, OutputEvent, PictureOrder, Result, SeiInfo, SliceHeader,
    bits::Bits,
    dpb::Dpb,
    nal,
    params::{self, Pps, Sps, Vps},
    sei, slice,
};
use sha2::{Digest, Sha256};

/// Geometry and format of an activated Main/Main10 picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PictureInfo {
    /// Coded luma width before the conformance window.
    pub width: u32,
    /// Coded luma height before the conformance window.
    pub height: u32,
    /// Luma width after the conformance window.
    pub visible_width: u32,
    /// Luma height after the conformance window.
    pub visible_height: u32,
    /// Matching component depth, 8 or 10.
    pub bit_depth: u32,
    /// Signalled general profile identifier; compatibility flags also govern admission.
    pub profile_idc: u32,
    /// Base-two logarithm of the CTU side length.
    pub ctb_log2: u32,
    /// Tile syntax enabled in the activated PPS.
    pub tiles: bool,
    /// Wavefront synchronization enabled in the activated PPS.
    pub wpp: bool,
    /// Left, right, top and bottom conformance offsets in chroma samples.
    pub conformance_window: [u32; 4],
    /// VUI declarations, with absence preserved.
    pub vui: crate::Vui,
}

/// Inspection result of one complete NAL unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    clippy::large_enum_variant,
    reason = "One transient Copy result keeps bounded reference arrays inline without per-slice allocation"
)]
pub enum NalInfo {
    /// A fully validated VPS replaced its fixed slot.
    Vps(u32),
    /// A fully validated SPS replaced its fixed slot.
    Sps(u32),
    /// A fully validated PPS replaced its fixed slot.
    Pps(u32),
    /// Slice header and activated picture format; no entropy data were decoded.
    Slice {
        /// Complete slice segment header.
        header: SliceHeader,
        /// Activated picture format and display metadata.
        picture: PictureInfo,
        /// POC bookkeeping for a first slice, absent for later segments.
        order: Option<PictureOrder>,
    },
    /// Informational messages, with defined picture-hash payloads retained.
    Sei(SeiInfo),
    /// An access-unit delimiter.
    AccessUnitDelimiter,
    /// End of a coded video sequence.
    EndOfSequence,
    /// End of the byte stream.
    EndOfBitstream,
    /// Filler or a reserved non-VCL NAL ignored under clause 7.4.2.2.
    Ignored,
}

/// Counts of successfully parsed syntax, not reconstructed or output pictures.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// Complete NAL units validated or explicitly ignored.
    pub nals: u64,
    /// Coded picture boundaries from first-slice flags.
    pub pictures: u64,
    /// Pictures with pic_output_flag set; this is not a DPB output count.
    pub output_flag_pictures: u64,
    /// Pictures emitted by header-only Annex C bookkeeping, including final drain.
    pub output_pictures: u64,
    /// Independently coded slice segments.
    pub independent_slices: u64,
    /// Dependent slice segments.
    pub dependent_slices: u64,
    /// Defined decoded-picture hash messages, not pixel comparisons.
    pub picture_hashes: u64,
    /// Reserved hash types skipped, never counted as checked hashes.
    pub reserved_hashes: u64,
}

#[derive(Clone, Copy)]
struct PictureState {
    pps_id: u32,
    sps_id: u32,
    vps_id: u32,
    nal: NalHeader,
    poc_lsb: u32,
    output: bool,
    no_output_prior: bool,
    reference_syntax: Option<(usize, [u8; 32])>,
    info: PictureInfo,
    last_address: u32,
}

/// Stateful, sans-IO parameter activation and header validation.
///
/// After an error the parser is terminal. Construct a new parser for a new
/// stream rather than continuing with partially accepted sequence state.
pub struct HeaderParser {
    limits: Limits,
    vps: [Option<Vps>; 16],
    vps_hash: [[u8; 32]; 16],
    active_vps_hash: Option<[u8; 32]>,
    sps: [Option<Sps>; 16],
    sps_hash: [[u8; 32]; 16],
    active_sps: Option<(u32, [u8; 32])>,
    pps: [Option<Pps>; 64],
    pps_hash: [[u8; 32]; 64],
    picture: Option<PictureState>,
    summary: Summary,
    failed: bool,
    ended: bool,
    dpb: Dpb,
    finished: bool,
}

impl HeaderParser {
    /// Fixed parameter-set slots plus at most `max_nal_bytes` of RBSP scratch.
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            vps: core::array::from_fn(|_| None),
            sps: core::array::from_fn(|_| None),
            sps_hash: [[0; 32]; 16],
            active_sps: None,
            vps_hash: [[0; 32]; 16],
            active_vps_hash: None,
            pps: core::array::from_fn(|_| None),
            pps_hash: [[0; 32]; 64],
            picture: None,
            summary: Summary::default(),
            failed: false,
            ended: false,
            dpb: Dpb::new(),
            finished: false,
        }
    }

    /// Parse one complete NAL, without framing; never falls back to another decoder.
    pub fn read_nal(
        &mut self,
        bytes: &[u8],
        emit: impl FnMut(OutputEvent) -> Result<()>,
    ) -> Result<NalInfo> {
        if self.failed {
            return Err(Error::InvalidData("parser_failed"));
        }
        if self.finished {
            return Err(Error::InvalidData("parser_finished"));
        }
        let result = self.read(bytes, emit);
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    /// Complete a stream inspection. Empty or failed streams cannot pass a gate.
    pub fn finish(&mut self, mut emit: impl FnMut(OutputEvent) -> Result<()>) -> Result<Summary> {
        if self.failed {
            return Err(Error::InvalidData("parser_failed"));
        }
        if self.summary.pictures == 0 {
            return Err(Error::InvalidData("no_coded_pictures"));
        }
        if !self.finished {
            let result = self.dpb.finish(|event| {
                emit(event)?;
                self.summary.output_pictures += 1;
                Ok(())
            });
            if let Err(error) = result {
                self.failed = true;
                return Err(error);
            }
            self.finished = true;
        }
        Ok(self.summary)
    }

    fn read(
        &mut self,
        bytes: &[u8],
        mut emit: impl FnMut(OutputEvent) -> Result<()>,
    ) -> Result<NalInfo> {
        if self.ended {
            return Err(Error::InvalidData("nal_after_eob"));
        }
        let nals = self
            .summary
            .nals
            .checked_add(1)
            .ok_or(Error::Limit("nal_count"))?;
        if bytes.len() > self.limits.max_nal_bytes {
            return Err(Error::Limit("max_nal_bytes"));
        }
        let nal = NalHeader::parse(bytes)?;
        let rbsp = nal::rbsp(bytes, &self.limits)?;
        let mut bits = Bits::new(&rbsp);
        let info = match nal.unit_type {
            32 => {
                let set = params::vps(&mut bits, &self.limits)?;
                let id = set.id;
                *self
                    .vps
                    .get_mut(id as usize)
                    .ok_or(Error::InvalidData("vps_id"))? = Some(set);
                let hash = Sha256::digest(&rbsp).into();
                let previous = self
                    .vps_hash
                    .get_mut(id as usize)
                    .ok_or(Error::InvalidData("vps_id"))?;
                if *previous != hash && self.picture.is_some_and(|picture| picture.vps_id == id) {
                    self.picture = None;
                }
                *previous = hash;
                NalInfo::Vps(id)
            }
            33 => {
                let set = params::sps(&mut bits, &self.limits)?;
                let id = set.id;
                *self
                    .sps
                    .get_mut(id as usize)
                    .ok_or(Error::InvalidData("sps_id"))? = Some(set);
                let hash = Sha256::digest(&rbsp).into();
                let previous = self
                    .sps_hash
                    .get_mut(id as usize)
                    .ok_or(Error::InvalidData("sps_id"))?;
                if *previous != hash && self.picture.is_some_and(|picture| picture.sps_id == id) {
                    self.picture = None;
                }
                *previous = hash;
                NalInfo::Sps(id)
            }
            34 => {
                let set = params::pps(&mut bits, &self.limits)?;
                let id = set.id;
                *self
                    .pps
                    .get_mut(id as usize)
                    .ok_or(Error::InvalidData("pps_id"))? = Some(set);
                let hash = Sha256::digest(&rbsp).into();
                let previous = self
                    .pps_hash
                    .get_mut(id as usize)
                    .ok_or(Error::InvalidData("pps_id"))?;
                if *previous != hash && self.picture.is_some_and(|picture| picture.pps_id == id) {
                    self.picture = None;
                }
                *previous = hash;
                NalInfo::Pps(id)
            }
            0..=9 | 16..=21 => {
                let prefix = slice::prefix(&mut bits, nal)?;
                let p = self
                    .pps
                    .get(prefix.pps_id as usize)
                    .and_then(Option::as_ref)
                    .ok_or(Error::InvalidData("missing_pps"))?;
                let s = self
                    .sps
                    .get(p.sps_id as usize)
                    .and_then(Option::as_ref)
                    .ok_or(Error::InvalidData("missing_sps"))?;
                let v = self
                    .vps
                    .get(s.vps_id as usize)
                    .and_then(Option::as_ref)
                    .ok_or(Error::InvalidData("missing_vps"))?;
                activation(v, s, p, nal)?;
                let header = slice::parse(&mut bits, nal, prefix, s, p, &self.limits)?;
                if header.rbsp_header_bytes >= rbsp.len() {
                    return Err(Error::InvalidData("missing_slice_data"));
                }
                let picture = PictureInfo {
                    width: s.width,
                    height: s.height,
                    visible_width: s.visible_width,
                    visible_height: s.visible_height,
                    bit_depth: s.bit_depth,
                    profile_idc: s.profile_idc,
                    ctb_log2: s.ctb_log2,
                    tiles: p.tiles,
                    wpp: p.wpp,
                    conformance_window: s.conformance_window,
                    vui: s.vui,
                };
                if header.first {
                    if self.summary.pictures >= self.limits.max_pictures {
                        return Err(Error::Limit("max_pictures"));
                    }
                    self.picture = Some(PictureState {
                        pps_id: header.pps_id,
                        sps_id: p.sps_id,
                        vps_id: s.vps_id,
                        nal,
                        poc_lsb: header
                            .poc_lsb
                            .ok_or(Error::InvalidData("first_slice_poc"))?,
                        output: header
                            .output
                            .ok_or(Error::InvalidData("first_slice_output"))?,
                        no_output_prior: header.no_output_of_prior_pics,
                        reference_syntax: header.reference_syntax,
                        info: picture,
                        last_address: 0,
                    });
                    self.summary.pictures += 1;
                    self.summary.output_flag_pictures += u64::from(header.output == Some(true));
                } else {
                    let current = self
                        .picture
                        .as_mut()
                        .ok_or(Error::InvalidData("missing_first_slice"))?;
                    if current.pps_id != header.pps_id
                        || current.nal != nal
                        || current.info != picture
                        || header.poc_lsb.is_some_and(|poc| poc != current.poc_lsb)
                        || header.output.is_some_and(|output| output != current.output)
                        || header.no_output_of_prior_pics != current.no_output_prior
                        || !header.dependent && header.reference_syntax != current.reference_syntax
                        || tile_key(s, p, header.address)? <= tile_key(s, p, current.last_address)?
                    {
                        return Err(Error::InvalidData("inconsistent_picture_headers"));
                    }
                    current.last_address = header.address;
                }
                let order = if header.first {
                    let hash = *self
                        .vps_hash
                        .get(s.vps_id as usize)
                        .ok_or(Error::InvalidData("vps_id"))?;
                    if self
                        .active_vps_hash
                        .is_some_and(|previous| previous != hash)
                    {
                        self.dpb.start_cvsg(nal, &header, |event| {
                            emit(event)?;
                            self.summary.output_pictures += 1;
                            Ok(())
                        })?;
                    }
                    self.active_vps_hash = Some(hash);
                    let sps_identity = (
                        p.sps_id,
                        *self
                            .sps_hash
                            .get(p.sps_id as usize)
                            .ok_or(Error::InvalidData("sps_id"))?,
                    );
                    let starts_cvs = self.dpb.starts_cvs(nal);
                    if !starts_cvs
                        && self
                            .active_sps
                            .is_some_and(|previous| previous != sps_identity)
                    {
                        return Err(Error::InvalidData("sps_change_within_cvs"));
                    }
                    let order = self.dpb.picture(nal, &header, s, &self.limits, |event| {
                        emit(event)?;
                        self.summary.output_pictures += 1;
                        Ok(())
                    })?;
                    if starts_cvs {
                        self.active_sps = Some(sps_identity);
                    }
                    Some(order)
                } else {
                    None
                };
                if header.dependent {
                    self.summary.dependent_slices += 1;
                } else {
                    self.summary.independent_slices += 1;
                }
                NalInfo::Slice {
                    header,
                    picture,
                    order,
                }
            }
            35 => {
                if bits.u(3)? > 2 {
                    return Err(Error::InvalidData("aud_pic_type"));
                }
                bits.trailing()?;
                self.picture = None;
                NalInfo::AccessUnitDelimiter
            }
            36 | 37 => {
                if !rbsp.is_empty() {
                    return Err(Error::InvalidData("end_nal_payload"));
                }
                self.picture = None;
                if nal.unit_type == 37 {
                    self.ended = true;
                    NalInfo::EndOfBitstream
                } else {
                    self.dpb.end_sequence();
                    NalInfo::EndOfSequence
                }
            }
            38 => {
                let mut data = rbsp.as_slice();
                while data.first() == Some(&255) {
                    data = data.get(1..).unwrap_or_default();
                }
                Bits::new(data).trailing()?;
                NalInfo::Ignored
            }
            39 | 40 => {
                let info = sei::parse(&rbsp, nal.unit_type == 40)?;
                if nal.unit_type == 40 && self.picture.is_none() {
                    return Err(Error::InvalidData("suffix_sei_without_picture"));
                }
                self.summary.picture_hashes += u64::from(info.picture_hash.is_some());
                self.summary.reserved_hashes += u64::from(info.reserved_hashes);
                NalInfo::Sei(info)
            }
            10..=15 | 22..=31 => return Err(Error::Unsupported("reserved_vcl")),
            _ => NalInfo::Ignored,
        };
        self.summary.nals = nals;
        Ok(info)
    }
}

fn activation(v: &Vps, s: &Sps, p: &Pps, nal: NalHeader) -> Result<()> {
    if s.sub_layers > v.sub_layers
        || u32::from(nal.temporal_id) > s.sub_layers
        || v.temporal_nesting && !s.temporal_nesting
    {
        return Err(Error::InvalidData("active_sub_layers"));
    }
    for i in 0..=s.sub_layers as usize {
        let s_dpb = s.max_dpb.get(i).ok_or(Error::InvalidData("sps_dpb"))?;
        let v_dpb = v.max_dpb.get(i).ok_or(Error::InvalidData("vps_dpb"))?;
        if s_dpb > v_dpb
            || s.max_reorder.get(i) > v.max_reorder.get(i)
            || match (s.max_latency.get(i), v.max_latency.get(i)) {
                (Some(s), Some(v)) => *v != 0 && (*s == 0 || s > v),
                _ => true,
            }
        {
            return Err(Error::InvalidData("active_dpb_limits"));
        }
    }
    if p.qp_delta_depth > s.ctb_log2 - s.min_cb_log2
        || p.parallel_merge_log2 > s.ctb_log2
        || p.init_qp < -6 * (s.bit_depth as i32 - 8)
        || p.init_qp > 51
        || p.scaling_list_present && !s.scaling_list_enabled
    {
        return Err(Error::InvalidData("active_pps"));
    }
    if p.tiles && p.wpp {
        return Err(Error::InvalidData("main_tiles_and_wpp"));
    }
    if let (Some(vps), Some(vui)) = (v.timing, s.vui.timing) {
        if vps.num_units_in_tick != vui.num_units_in_tick || vps.time_scale != vui.time_scale {
            return Err(Error::InvalidData("vps_vui_clock"));
        }
    }
    let width = s.width.div_ceil(1 << s.ctb_log2);
    let height = s.height.div_ceil(1 << s.ctb_log2);
    if p.tile_columns > width || p.tile_rows > height {
        return Err(Error::InvalidData("tile_grid"));
    }
    if !p.uniform_tiles {
        let columns: u64 = p
            .column_width
            .iter()
            .take(p.tile_columns.saturating_sub(1) as usize)
            .map(|v| u64::from(*v))
            .sum();
        let rows: u64 = p
            .row_height
            .iter()
            .take(p.tile_rows.saturating_sub(1) as usize)
            .map(|v| u64::from(*v))
            .sum();
        if columns >= u64::from(width) || rows >= u64::from(height) {
            return Err(Error::InvalidData("tile_grid"));
        }
    }
    Ok(())
}

fn tile_key(s: &Sps, p: &Pps, address: u32) -> Result<(u32, u32, u32, u32)> {
    let width = s.width.div_ceil(1 << s.ctb_log2);
    let height = s.height.div_ceil(1 << s.ctb_log2);
    let (x, y) = (address % width, address / width);
    let col = tile_index(x, width, p.tile_columns, p.uniform_tiles, &p.column_width)?;
    let row = tile_index(y, height, p.tile_rows, p.uniform_tiles, &p.row_height)?;
    Ok((row, col, y, x))
}

fn tile_index(
    position: u32,
    length: u32,
    count: u32,
    uniform: bool,
    widths: &[u32],
) -> Result<u32> {
    let mut boundary = 0u64;
    for i in 0..count.saturating_sub(1) {
        if uniform {
            boundary = u64::from(i + 1) * u64::from(length) / u64::from(count);
        } else {
            boundary += u64::from(
                *widths
                    .get(i as usize)
                    .ok_or(Error::InvalidData("tile_grid"))?,
            );
        }
        if u64::from(position) < boundary {
            return Ok(i);
        }
    }
    Ok(count.saturating_sub(1))
}
