use crate::{
    Error, Limits, NalInfo, PictureInfo, Result, Summary,
    parser::StreamCore,
    reconstruction::{Frame, Memory},
};
use coreav_kernels_hevc as kernels;

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
    /// Eligible intra deblocking edge segments per component.
    pub deblock_edges: [u64; 3],
    /// Samples changed by the deblocking passes per component.
    pub deblock_changed_samples: [u64; 3],
    /// CTBs with a decoded nonzero SAO type per component.
    pub sao_blocks: [u64; 3],
    /// Samples changed by SAO per component.
    pub sao_changed_samples: [u64; 3],
    /// CTUs whose SAO parameters were merged from an available neighbour.
    pub sao_merges: u64,
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
            deblock_edges: [0; 3],
            deblock_changed_samples: [0; 3],
            sao_blocks: [0; 3],
            sao_changed_samples: [0; 3],
            sao_merges: 0,
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

/// A complete, in-loop-filtered intra picture tied to its original coded-picture index.
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
/// Original syntax controls deblocking and SAO before hashes or publication.
/// Inter pictures return `Unsupported`; no fallback runs.
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
                    self.peak_memory = self.peak_memory.max(frame.hash(sei, fixed, limit)?);
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
        self.peak_memory = self
            .peak_memory
            .max(frame.finalize(Self::fixed_memory()?, self.limits.max_memory_bytes)?);
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
