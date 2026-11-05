use crate::{
    Error, Limits, NalInfo, PictureInfo, Result, SeiInfo, Summary,
    parser::{SliceInput, StreamCore},
    picture_hash,
};
use coreav_kernels_hevc::{self as kernels, BitDepth, IntraMode};

mod syntax;

/// Independent header, reconstruction-memory and selected-picture entropy budgets.
#[derive(Clone, Copy, Debug)]
pub struct IntraLimits {
    /// Existing header admission limits, also applied to unselected pictures.
    pub headers: Limits,
    /// Maximum simultaneous reconstruction state, scratch and retained syntax bytes.
    pub max_memory_bytes: usize,
    /// Maximum regular, bypass and termination bins across the selected picture.
    pub max_bins: u64,
}

impl Default for IntraLimits {
    fn default() -> Self {
        Self {
            headers: Limits::default(),
            max_memory_bytes: 256 * 1024 * 1024,
            max_bins: 268_435_456,
        }
    }
}

/// Decoded syntax and numerical work for one selected coded picture, without timing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IntraStats {
    /// Completed coding tree units.
    pub ctus: u64,
    /// Decoded leaf coding units, including PCM.
    pub coding_units: u64,
    /// Reconstructed component transform blocks, including blocks with no coefficients.
    pub transform_units: u64,
    /// Component blocks whose coded-block flag signalled residual coefficients.
    pub nonzero_transform_units: u64,
    /// Effective transform-skip blocks.
    pub transform_skips: u64,
    /// Coding units with transquant bypass selected.
    pub transquant_bypass_units: u64,
    /// Coding units containing raw PCM samples.
    pub pcm_units: u64,
    /// Decoded dependent segments.
    pub dependent_segments: u64,
    /// Entropy initialization at tile starts, including the first tile.
    pub tile_restarts: u64,
    /// Entropy initialization or synchronization at noninitial WPP rows.
    pub wpp_restarts: u64,
    /// Coded QP deltas, including zero deltas.
    pub qp_deltas: u64,
    /// Coded QP deltas whose value is nonzero.
    pub nonzero_qp_deltas: u64,
    /// Scaled residual blocks using an enabled SPS or PPS scaling list.
    pub scaling_list_blocks: u64,
    /// Coefficient groups whose first sign was inferred by sign-data hiding.
    pub sign_hidden_groups: u64,
    /// Effective intra-luma DST blocks.
    pub dst_blocks: u64,
    /// Consumed picture-wide CABAC bins, including termination and restarts.
    pub bins: u64,
    /// Defined suffix hashes checked over all three coded components.
    pub picture_hashes: u64,
    /// Reserved suffix hashes, not credited as checked hashes.
    pub reserved_hashes: u64,
    /// Accounted peak simultaneous bytes before picture publication.
    pub memory_bytes: usize,
    /// Effective luma prediction blocks indexed by resolved mode.
    pub luma_modes: [u64; 35],
    /// Effective chroma prediction blocks indexed by resolved mode.
    pub chroma_modes: [u64; 35],
    /// Effective inverse DCT blocks at sides 4, 8, 16 and 32.
    pub dct_blocks: [u64; 4],
}

impl Default for IntraStats {
    fn default() -> Self {
        Self {
            ctus: 0,
            coding_units: 0,
            transform_units: 0,
            nonzero_transform_units: 0,
            transform_skips: 0,
            transquant_bypass_units: 0,
            pcm_units: 0,
            dependent_segments: 0,
            tile_restarts: 0,
            wpp_restarts: 0,
            qp_deltas: 0,
            nonzero_qp_deltas: 0,
            scaling_list_blocks: 0,
            sign_hidden_groups: 0,
            dst_blocks: 0,
            bins: 0,
            picture_hashes: 0,
            reserved_hashes: 0,
            memory_bytes: 0,
            luma_modes: [0; 35],
            chroma_modes: [0; 35],
            dct_blocks: [0; 4],
        }
    }
}

/// Borrowed coded-component samples; cropping is described by the picture metadata.
#[derive(Clone, Copy, Debug)]
pub struct IntraPlane<'a> {
    /// Samples in row order, without an 8-bit narrowing conversion.
    pub samples: &'a [u16],
    /// Coded component width in samples.
    pub width: usize,
    /// Coded component height in samples.
    pub height: usize,
    /// Distance between rows in samples.
    pub stride: usize,
}

/// A complete, unfiltered intra picture tied to its original coded-picture index.
#[derive(Clone, Copy, Debug)]
pub struct IntraPicture<'a> {
    /// Zero-based index in the original coded NAL sequence, not output order.
    pub index: u64,
    /// Activated coded/visible geometry, crop and sample depth.
    pub info: PictureInfo,
    /// Coded Y, Cb and Cr planes, valid only during the callback.
    pub planes: [IntraPlane<'a>; 3],
    /// Effective syntax counts, verified hash counts and runtime budgets.
    pub stats: IntraStats,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Active,
    Finished,
    Failed,
}

/// Reconstructs one original Main/Main10 intra picture while inspecting all headers.
///
/// The selected picture must disable deblocking and SAO in its original syntax.
/// Inter pictures and filtered pictures return `Unsupported`; no fallback runs.
/// Input NAL storage belongs to the caller and is not included in reconstruction
/// memory. Every error is terminal and discards unpublished samples. A later
/// error cannot revoke an earlier callback, so callers must also require EOF.
pub struct IntraPictureDecoder {
    core: StreamCore,
    limits: IntraLimits,
    target: u64,
    remaining_bins: u64,
    frame: Option<Frame>,
    delivered: bool,
    state: State,
    peak_memory: usize,
}

impl IntraPictureDecoder {
    /// Creates a sans-IO session with fixed parameter slots and no picture allocation.
    pub fn new(limits: IntraLimits, target_picture_index: u64) -> Self {
        Self {
            core: StreamCore::new(limits.headers),
            limits,
            target: target_picture_index,
            remaining_bins: limits.max_bins,
            frame: None,
            delivered: false,
            state: State::Active,
            peak_memory: 0,
        }
    }

    fn fixed_memory() -> Result<usize> {
        core::mem::size_of::<Self>()
            .checked_add(kernels::SCRATCH_BYTES)
            .and_then(|n| n.checked_add(4 * core::mem::size_of::<crate::params::Sps>()))
            .and_then(|n| n.checked_add(2 * core::mem::size_of::<crate::params::Pps>()))
            .ok_or(Error::Limit("reconstruction_memory"))
    }

    /// Feeds one original, complete NAL and publishes only a completed selected picture.
    pub fn read_nal(
        &mut self,
        bytes: &[u8],
        emit: impl FnMut(IntraPicture<'_>) -> Result<()>,
    ) -> Result<()> {
        if self.state != State::Active {
            self.state = State::Failed;
            return Err(Error::InvalidData("intra_decoder_closed"));
        }
        let result = self.read(bytes, emit);
        if result.is_err() {
            self.state = State::Failed;
            self.frame = None;
        }
        result
    }

    fn read(
        &mut self,
        bytes: &[u8],
        mut emit: impl FnMut(IntraPicture<'_>) -> Result<()>,
    ) -> Result<()> {
        let fixed = Self::fixed_memory()?;
        let heap = self.frame.as_ref().map_or(Ok(0), Frame::heap_bytes)?;
        let live = fixed
            .checked_add(heap)
            .ok_or(Error::Limit("reconstruction_memory"))?;
        let available = self
            .limits
            .max_memory_bytes
            .checked_sub(live)
            .ok_or(Error::Limit("reconstruction_memory"))?;
        let mut peak = Memory::new(live, self.limits.max_memory_bytes)?;
        peak.charge(bytes.len().saturating_sub(2))?;
        self.peak_memory = self.peak_memory.max(peak.used);
        let mut selected = false;
        let frame = &mut self.frame;
        let remaining = &mut self.remaining_bins;
        let mut observed_peak = self.peak_memory;
        let limit = self.limits.max_memory_bytes;
        let info = self.core.read_selected(
            bytes,
            |_| Ok(()),
            Some(self.target),
            available,
            |input| {
                selected = true;
                if frame.is_none() {
                    *frame = Some(Frame::new(&input, fixed, limit)?);
                }
                let frame = frame
                    .as_mut()
                    .ok_or(Error::InvalidData("selected_picture_state"))?;
                observed_peak = observed_peak.max(frame.decode(input, remaining, fixed, limit)?);
                Ok(())
            },
        )?;
        self.peak_memory = observed_peak;
        match info {
            NalInfo::Sei(sei) => {
                if let Some(frame) = self.frame.as_mut() {
                    frame.hash(sei)?;
                }
            }
            NalInfo::Slice { header, .. } if header.first && !selected && self.frame.is_some() => {
                self.publish(&mut emit)?
            }
            NalInfo::AccessUnitDelimiter | NalInfo::EndOfSequence | NalInfo::EndOfBitstream
                if self.frame.is_some() =>
            {
                self.publish(&mut emit)?
            }
            _ => {}
        }
        Ok(())
    }

    fn publish(&mut self, emit: &mut impl FnMut(IntraPicture<'_>) -> Result<()>) -> Result<()> {
        let frame = self
            .frame
            .as_mut()
            .ok_or(Error::InvalidData("target_picture_missing"))?;
        if self.delivered || !frame.complete() {
            return Err(Error::InvalidData("selected_picture_incomplete"));
        }
        frame.stats.bins = self.limits.max_bins - self.remaining_bins;
        frame.stats.memory_bytes = self.peak_memory;
        emit(frame.view())?;
        self.frame = None;
        self.delivered = true;
        Ok(())
    }

    /// Requires complete header EOF and exactly one completed selected-picture callback.
    pub fn finish(
        &mut self,
        mut emit: impl FnMut(IntraPicture<'_>) -> Result<()>,
    ) -> Result<Summary> {
        if self.state == State::Failed {
            return Err(Error::InvalidData("intra_decoder_failed"));
        }
        let result = (|| {
            let summary = self.core.finish(|_| Ok(()))?;
            if !self.delivered {
                self.publish(&mut emit)?;
            }
            Ok(summary)
        })();
        if result.is_err() {
            self.state = State::Failed;
            self.frame = None;
        } else {
            self.state = State::Finished;
        }
        result
    }
}

struct Memory {
    used: usize,
    limit: usize,
}

impl Memory {
    fn new(used: usize, limit: usize) -> Result<Self> {
        if used > limit {
            return Err(Error::Limit("reconstruction_memory"));
        }
        Ok(Self { used, limit })
    }

    fn charge(&mut self, bytes: usize) -> Result<()> {
        self.used = self
            .used
            .checked_add(bytes)
            .filter(|n| *n <= self.limit)
            .ok_or(Error::Limit("reconstruction_memory"))?;
        Ok(())
    }

    fn allocate<T: Copy>(&mut self, count: usize, value: T) -> Result<Vec<T>> {
        let bytes = count
            .checked_mul(core::mem::size_of::<T>())
            .ok_or(Error::Limit("reconstruction_memory"))?;
        self.charge(bytes)?;
        let mut data = Vec::new();
        data.try_reserve_exact(count)
            .map_err(|_| Error::Limit("picture_allocation"))?;
        self.charge(
            (data.capacity() - count)
                .checked_mul(core::mem::size_of::<T>())
                .ok_or(Error::Limit("reconstruction_memory"))?,
        )?;
        data.resize(count, value);
        Ok(data)
    }
}

#[derive(Clone, Copy)]
struct Cell {
    mode: Option<IntraMode>,
    depth: u8,
    reconstructed: u8,
    qp: i8,
    slice: u32,
}

struct Scratch {
    coefficients: [i16; 1024],
    residual: [i32; 1024],
    prediction: [u16; 1024],
    output: [u16; 1024],
    scaling: [u8; 1024],
    references: [Option<u16>; 129],
}

struct Frame {
    index: u64,
    info: PictureInfo,
    depth: BitDepth,
    planes: [Vec<u16>; 3],
    cells: Vec<Cell>,
    next_ctu: u32,
    segment_closed: bool,
    contexts: syntax::Contexts,
    wpp: Option<syntax::Checkpoint>,
    qp: syntax::Qp,
    stats: IntraStats,
    hash_seen: bool,
    scratch: Scratch,
}

impl Frame {
    fn new(input: &SliceInput<'_>, fixed: usize, limit: usize) -> Result<Self> {
        Self::admit(input)?;
        if !input.header.first || input.header.address != 0 {
            return Err(Error::InvalidData("selected_picture_start"));
        }
        let mut memory = Memory::new(fixed, limit)?;
        memory.charge(input.scratch_bytes)?;
        let width = input.picture.width as usize;
        let height = input.picture.height as usize;
        let luma = width
            .checked_mul(height)
            .ok_or(Error::Limit("picture_samples"))?;
        let chroma = (width / 2)
            .checked_mul(height / 2)
            .ok_or(Error::Limit("picture_samples"))?;
        let cells = (width / 4)
            .checked_mul(height / 4)
            .ok_or(Error::Limit("picture_cells"))?;
        Ok(Self {
            index: input.index,
            info: input.picture,
            depth: BitDepth::new(input.picture.bit_depth as u8)?,
            planes: [
                memory.allocate(luma, 0)?,
                memory.allocate(chroma, 0)?,
                memory.allocate(chroma, 0)?,
            ],
            cells: memory.allocate(
                cells,
                Cell {
                    mode: None,
                    depth: 0,
                    reconstructed: 0,
                    qp: 0,
                    slice: 0,
                },
            )?,
            next_ctu: 0,
            segment_closed: false,
            contexts: syntax::Contexts::new(input.syntax.qp_y),
            wpp: None,
            qp: syntax::Qp::new(input.syntax.qp_y),
            stats: IntraStats::default(),
            hash_seen: false,
            scratch: Scratch {
                coefficients: [0; 1024],
                residual: [0; 1024],
                prediction: [0; 1024],
                output: [0; 1024],
                scaling: [16; 1024],
                references: [None; 129],
            },
        })
    }

    fn admit(input: &SliceInput<'_>) -> Result<()> {
        if input.syntax.kind != 2 || input.syntax.cabac_init {
            return Err(Error::Unsupported("selected_inter_picture"));
        }
        if !input.syntax.deblock_disabled || input.syntax.sao.into_iter().any(|v| v) {
            return Err(Error::Unsupported("selected_filtered_picture"));
        }
        Ok(())
    }

    fn heap_bytes(&self) -> Result<usize> {
        let samples = self
            .planes
            .iter()
            .try_fold(0usize, |n, p| n.checked_add(p.capacity()))
            .and_then(|n| n.checked_mul(core::mem::size_of::<u16>()))
            .ok_or(Error::Limit("reconstruction_memory"))?;
        samples
            .checked_add(
                self.cells
                    .capacity()
                    .checked_mul(core::mem::size_of::<Cell>())
                    .ok_or(Error::Limit("reconstruction_memory"))?,
            )
            .ok_or(Error::Limit("reconstruction_memory"))
    }

    fn complete(&self) -> bool {
        let ctb = 1 << self.info.ctb_log2;
        self.segment_closed
            && self.next_ctu == self.info.width.div_ceil(ctb) * self.info.height.div_ceil(ctb)
            && self.cells.iter().all(|c| c.reconstructed == 7)
    }

    fn hash(&mut self, sei: SeiInfo) -> Result<()> {
        if sei.picture_hash.is_none() && sei.reserved_hashes == 0 {
            return Ok(());
        }
        if !self.complete() || self.hash_seen {
            return Err(Error::InvalidData("selected_picture_hash_placement"));
        }
        self.hash_seen = true;
        if let Some(expected) = sei.picture_hash {
            let [y, cb, cr] = &self.planes;
            picture_hash::verify(
                &expected,
                [y, cb, cr],
                self.info.width as usize,
                self.info.height as usize,
                self.info.bit_depth,
            )?;
            self.stats.picture_hashes += 1;
        }
        self.stats.reserved_hashes += u64::from(sei.reserved_hashes);
        Ok(())
    }

    fn view(&self) -> IntraPicture<'_> {
        let width = self.info.width as usize;
        let height = self.info.height as usize;
        let [y, cb, cr] = &self.planes;
        IntraPicture {
            index: self.index,
            info: self.info,
            stats: self.stats,
            planes: [
                IntraPlane {
                    samples: y,
                    width,
                    height,
                    stride: width,
                },
                IntraPlane {
                    samples: cb,
                    width: width / 2,
                    height: height / 2,
                    stride: width / 2,
                },
                IntraPlane {
                    samples: cr,
                    width: width / 2,
                    height: height / 2,
                    stride: width / 2,
                },
            ],
        }
    }
}

impl From<kernels::Error> for Error {
    fn from(_: kernels::Error) -> Self {
        Self::InvalidData("intra_kernel_domain")
    }
}
