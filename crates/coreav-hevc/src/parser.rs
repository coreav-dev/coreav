use crate::{
    Error, Limits, NalHeader, OutputEvent, PictureOrder, Result, SeiInfo, SliceHeader,
    bits::Bits,
    ctu::Layout,
    dpb::{Dpb, Emission},
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
    /// Tolerated metadata declarations that do not change reconstructed samples.
    pub warnings: crate::Warnings,
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
    independent: SliceSyntax,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SliceSyntax {
    pub kind: u32,
    pub independent_address: u32,
    pub qp_y: i32,
    pub chroma_qp_offsets: [i32; 2],
    pub initialization: crate::cabac::Initialization,
    pub sao: [bool; 2],
    pub deblock_disabled: bool,
    pub deblock_offsets: [i32; 2],
    pub loop_filter_slices: bool,
    pub inter: Option<crate::slice::InterSlice>,
}

impl SliceSyntax {
    fn independent(header: &SliceHeader) -> Result<Self> {
        Ok(Self {
            kind: header
                .slice_type
                .ok_or(Error::InvalidData("missing_slice_type"))?,
            independent_address: header.address,
            qp_y: header
                .slice_qp_y
                .ok_or(Error::InvalidData("missing_slice_qp"))?,
            chroma_qp_offsets: header
                .chroma_qp_offsets
                .ok_or(Error::InvalidData("missing_chroma_qp"))?,
            initialization: crate::cabac::Initialization::for_slice(
                header
                    .slice_type
                    .ok_or(Error::InvalidData("missing_slice_type"))?,
                header
                    .cabac_init_flag
                    .ok_or(Error::InvalidData("missing_cabac_init"))?,
            )?,
            inter: header.inter,
            sao: header.sao.ok_or(Error::InvalidData("missing_sao_flags"))?,
            deblock_disabled: header
                .deblock_disabled
                .ok_or(Error::InvalidData("missing_deblock_flag"))?,
            deblock_offsets: header
                .deblock_offsets
                .ok_or(Error::InvalidData("missing_deblock_offsets"))?,
            loop_filter_slices: header
                .loop_filter_slices
                .ok_or(Error::InvalidData("missing_loop_filter_flag"))?,
        })
    }
}

/// Stateful, sans-IO parameter activation and header validation.
///
/// After an error the parser is terminal. Construct a new parser for a new
/// stream rather than continuing with partially accepted sequence state.
pub struct HeaderParser {
    core: StreamCore,
}

impl HeaderParser {
    /// Fixed parameter-set slots plus at most `max_nal_bytes` of RBSP scratch.
    pub fn new(limits: Limits) -> Self {
        Self {
            core: StreamCore::new(limits),
        }
    }

    /// Parse one complete NAL, without framing; never falls back to another decoder.
    pub fn read_nal(
        &mut self,
        bytes: &[u8],
        emit: impl FnMut(OutputEvent) -> Result<()>,
    ) -> Result<NalInfo> {
        self.core.read_nal(bytes, emit)
    }

    /// Complete a stream inspection. Empty or failed streams cannot pass a gate.
    pub fn finish(&mut self, emit: impl FnMut(OutputEvent) -> Result<()>) -> Result<Summary> {
        self.core.finish(emit)
    }
}

pub(crate) struct StreamCore {
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

#[derive(Clone, Copy)]
pub(crate) struct SliceInput<'a> {
    pub index: u64,
    pub header: SliceHeader,
    pub picture: PictureInfo,
    pub original: &'a [u8],
    pub rbsp: &'a [u8],
    pub entry_lengths: &'a [u64],
    pub syntax: SliceSyntax,
    pub sps: &'a Sps,
    pub pps: &'a Pps,
    pub layout: &'a Layout,
    pub scratch_bytes: usize,
}

pub(crate) trait PictureObserver {
    fn boundary(&mut self, dpb: &mut Dpb, scratch_bytes: usize) -> Result<()>;
    fn slice(&mut self, input: SliceInput<'_>, dpb: &Dpb) -> Result<()>;
    fn sei(&mut self, info: SeiInfo, dpb: &Dpb, scratch_bytes: usize) -> Result<()>;
    fn output(&mut self, emission: Emission<'_>, scratch_bytes: usize) -> Result<()>;
}

fn output(
    emission: Emission<'_>,
    observer: &mut Option<&mut dyn PictureObserver>,
    emit: &mut impl FnMut(OutputEvent) -> Result<()>,
    summary: &mut Summary,
    scratch_bytes: usize,
) -> Result<()> {
    if let Some(observer) = observer.as_mut() {
        observer.output(emission, scratch_bytes)?;
    } else {
        emit(emission.event)?;
    }
    summary.output_pictures += 1;
    Ok(())
}

fn boundary(
    dpb: &mut Dpb,
    observer: &mut Option<&mut dyn PictureObserver>,
    emit: &mut impl FnMut(OutputEvent) -> Result<()>,
    summary: &mut Summary,
    scratch_bytes: usize,
) -> Result<()> {
    if let Some(visitor) = observer.as_mut() {
        visitor.boundary(dpb, scratch_bytes)?;
        dpb.resume(|emission| output(emission, observer, emit, summary, scratch_bytes))?;
    }
    Ok(())
}

impl StreamCore {
    pub(crate) fn new(limits: Limits) -> Self {
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

    pub(crate) fn read_nal(
        &mut self,
        bytes: &[u8],
        emit: impl FnMut(OutputEvent) -> Result<()>,
    ) -> Result<NalInfo> {
        self.read_selected(bytes, emit, None, usize::MAX, |_| Ok(()))
    }

    pub(crate) fn new_reconstruction(limits: Limits) -> Self {
        Self {
            dpb: Dpb::new_reconstruction(),
            ..Self::new(limits)
        }
    }

    pub(crate) fn heap_bytes(&self) -> Result<usize> {
        self.dpb.heap_bytes()
    }

    pub(crate) fn fail(&mut self) {
        self.failed = true;
        self.dpb.fail();
    }

    pub(crate) fn read_reconstructed(
        &mut self,
        bytes: &[u8],
        memory_limit: usize,
        observer: &mut dyn PictureObserver,
    ) -> Result<NalInfo> {
        if self.failed {
            return Err(Error::InvalidData("parser_failed"));
        }
        if self.finished {
            return Err(Error::InvalidData("parser_finished"));
        }
        let result = self.read(
            bytes,
            |_| Ok(()),
            None,
            memory_limit,
            |_| Ok(()),
            Some(observer),
        );
        if result.is_err() {
            self.fail();
        }
        result
    }

    pub(crate) fn read_selected(
        &mut self,
        bytes: &[u8],
        emit: impl FnMut(OutputEvent) -> Result<()>,
        target: Option<u64>,
        memory_limit: usize,
        observe: impl FnMut(SliceInput<'_>) -> Result<()>,
    ) -> Result<NalInfo> {
        if self.failed {
            return Err(Error::InvalidData("parser_failed"));
        }
        if self.finished {
            return Err(Error::InvalidData("parser_finished"));
        }
        let result = self.read(bytes, emit, target, memory_limit, observe, None);
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    pub(crate) fn finish(
        &mut self,
        emit: impl FnMut(OutputEvent) -> Result<()>,
    ) -> Result<Summary> {
        self.finish_inner(emit, None)
    }

    pub(crate) fn finish_reconstructed(
        &mut self,
        observer: &mut dyn PictureObserver,
    ) -> Result<Summary> {
        let result = self.finish_inner(|_| Ok(()), Some(observer));
        if result.is_err() {
            self.fail();
        }
        result
    }

    fn finish_inner(
        &mut self,
        mut emit: impl FnMut(OutputEvent) -> Result<()>,
        mut observer: Option<&mut dyn PictureObserver>,
    ) -> Result<Summary> {
        if self.failed {
            return Err(Error::InvalidData("parser_failed"));
        }
        if self.summary.pictures == 0 {
            return Err(Error::InvalidData("no_coded_pictures"));
        }
        if !self.finished {
            boundary(
                &mut self.dpb,
                &mut observer,
                &mut emit,
                &mut self.summary,
                0,
            )?;
            let result = self
                .dpb
                .finish(|event| output(event, &mut observer, &mut emit, &mut self.summary, 0));
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
        target: Option<u64>,
        memory_limit: usize,
        mut observe: impl FnMut(SliceInput<'_>) -> Result<()>,
        mut observer: Option<&mut dyn PictureObserver>,
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
        if bytes.len().saturating_sub(2) > memory_limit {
            return Err(Error::Limit("reconstruction_memory"));
        }
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
                self.summary.warnings.profile_bit_depth |= set.profile_bit_depth_warning;
                self.summary.warnings.invalid_sample_aspect_ratio |=
                    set.vui.warnings.invalid_sample_aspect_ratio;
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
                let layout = activation(v, s, p, nal)?;
                let index = self
                    .summary
                    .pictures
                    .saturating_sub(u64::from(!prefix.first));
                let retain_entries = if target == Some(index) || observer.is_some() {
                    Some(
                        memory_limit
                            .checked_sub(rbsp.capacity())
                            .ok_or(Error::Limit("reconstruction_memory"))?,
                    )
                } else {
                    None
                };
                let parsed =
                    slice::parse(&mut bits, nal, prefix, s, p, &self.limits, retain_entries)?;
                let header = parsed.header;
                if header.rbsp_header_bytes >= rbsp.len() {
                    return Err(Error::InvalidData("missing_slice_data"));
                }
                let syntax = if header.dependent {
                    self.picture
                        .as_ref()
                        .ok_or(Error::InvalidData("missing_first_slice"))?
                        .independent
                } else {
                    SliceSyntax::independent(&header)?
                };
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
                        independent: syntax,
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
                        || layout.at_raster(header.address)?.tile_scan
                            <= layout.at_raster(current.last_address)?.tile_scan
                    {
                        return Err(Error::InvalidData("inconsistent_picture_headers"));
                    }
                    current.last_address = header.address;
                    if !header.dependent {
                        current.independent = syntax;
                    }
                }
                let order = if header.first {
                    let scratch_bytes = rbsp
                        .capacity()
                        .checked_add(
                            parsed
                                .entry_lengths
                                .capacity()
                                .checked_mul(core::mem::size_of::<u64>())
                                .ok_or(Error::Limit("reconstruction_memory"))?,
                        )
                        .ok_or(Error::Limit("reconstruction_memory"))?;
                    boundary(
                        &mut self.dpb,
                        &mut observer,
                        &mut emit,
                        &mut self.summary,
                        rbsp.capacity()
                            .checked_add(
                                parsed
                                    .entry_lengths
                                    .capacity()
                                    .checked_mul(core::mem::size_of::<u64>())
                                    .ok_or(Error::Limit("reconstruction_memory"))?,
                            )
                            .ok_or(Error::Limit("reconstruction_memory"))?,
                    )?;
                    let hash = *self
                        .vps_hash
                        .get(s.vps_id as usize)
                        .ok_or(Error::InvalidData("vps_id"))?;
                    if self
                        .active_vps_hash
                        .is_some_and(|previous| previous != hash)
                    {
                        self.dpb.start_cvsg(nal, &header, |event| {
                            output(
                                event,
                                &mut observer,
                                &mut emit,
                                &mut self.summary,
                                scratch_bytes,
                            )
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
                        output(
                            event,
                            &mut observer,
                            &mut emit,
                            &mut self.summary,
                            scratch_bytes,
                        )
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
                if retain_entries.is_some() {
                    let input = SliceInput {
                        index,
                        header,
                        picture,
                        original: bytes,
                        rbsp: &rbsp,
                        entry_lengths: &parsed.entry_lengths,
                        syntax,
                        sps: s,
                        pps: p,
                        layout: &layout,
                        scratch_bytes: rbsp
                            .capacity()
                            .checked_add(
                                parsed
                                    .entry_lengths
                                    .capacity()
                                    .checked_mul(core::mem::size_of::<u64>())
                                    .ok_or(Error::Limit("reconstruction_memory"))?,
                            )
                            .ok_or(Error::Limit("reconstruction_memory"))?,
                    };
                    if let Some(observer) = observer.as_mut() {
                        observer.slice(input, &self.dpb)?;
                    } else {
                        observe(input)?;
                    }
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
                boundary(
                    &mut self.dpb,
                    &mut observer,
                    &mut emit,
                    &mut self.summary,
                    rbsp.capacity(),
                )?;
                self.picture = None;
                NalInfo::AccessUnitDelimiter
            }
            36 | 37 => {
                if !rbsp.is_empty() {
                    return Err(Error::InvalidData("end_nal_payload"));
                }
                boundary(
                    &mut self.dpb,
                    &mut observer,
                    &mut emit,
                    &mut self.summary,
                    rbsp.capacity(),
                )?;
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
                if nal.unit_type == 40 {
                    if let Some(observer) = observer.as_mut() {
                        observer.sei(info, &self.dpb, rbsp.capacity())?;
                    }
                }
                NalInfo::Sei(info)
            }
            10..=15 | 22..=31 => return Err(Error::Unsupported("reserved_vcl")),
            _ => NalInfo::Ignored,
        };
        self.summary.nals = nals;
        self.summary.warnings.temporal_nesting_nal_type =
            self.dpb.warnings.temporal_nesting_nal_type;
        Ok(info)
    }
}

fn activation(v: &Vps, s: &Sps, p: &Pps, nal: NalHeader) -> Result<Layout> {
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
        || p.scaling_lists.is_some() && s.scaling_lists.is_none()
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
    Layout::new(s, p)
}
