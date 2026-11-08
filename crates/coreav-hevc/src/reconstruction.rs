use crate::dpb::References;
use crate::intra::{IntraPicture, IntraPlane, IntraStats};
use crate::{Error, PictureInfo, Result, SeiInfo, parser::SliceInput, picture_hash};
use coreav_kernels_hevc::{self as kernels, BitDepth, IntraMode, MotionVector};

mod filter;
mod inter;
mod parallel;
pub(crate) mod raster;
mod rows;
mod syntax;

pub(crate) use parallel::Cache;
pub(crate) use rows::{ReadSupport, ReferenceRead, RowPublisher, RowStore};

#[cfg(hevc_thread_diagnostics)]
pub(crate) use inter::diagnostic_active_references;

pub(crate) fn caught<T>(work: impl FnOnce() -> Result<T>) -> Result<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
        .unwrap_or(Err(Error::Internal("reconstruction_panic")))
}

/// Effective scalar inter reconstruction work, without timing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InterStats {
    /// Skip coding units with completed prediction samples.
    pub skip_units: u64,
    /// Completed inter prediction units.
    pub prediction_units: u64,
    /// Completed prediction units in P slices.
    pub p_units: u64,
    /// Completed prediction units in B slices.
    pub b_units: u64,
    /// Prediction units combining both reference lists.
    pub bi_units: u64,
    /// Prediction units using a selected merge candidate.
    pub merge_units: u64,
    /// Reference lists using an AMVP predictor and decoded motion difference.
    pub amvp_lists: u64,
    /// Prediction units selecting a temporal merge or AMVP candidate.
    pub temporal_units: u64,
    /// Prediction units applying at least one nondefault effective weight or offset.
    pub weighted_units: u64,
    /// Component samples differing from the same prediction without explicit weights.
    pub weighted_changed_samples: [u64; 3],
    /// Prediction units with fractional luma displacement in an active list.
    pub fractional_luma_units: u64,
    /// Prediction units with fractional chroma displacement in an active list.
    pub fractional_chroma_units: u64,
    /// Prediction units using at least one negative motion component.
    pub negative_motion_units: u64,
    /// Prediction units using a long-term reference.
    pub long_term_units: u64,
    /// Prediction units using a list with signalled modifications.
    pub list_modification_units: u64,
    /// Completed partitions in 2Nx2N, 2NxN, Nx2N, NxN, 2NxnU, 2NxnD, nLx2N, nRx2N order.
    pub partitions: [u64; 8],
    /// Eligible luma edge segments indexed by boundary strength zero, one or two.
    pub boundary_strengths: [u64; 3],
    /// Changed deblocking samples indexed by boundary strength, then component.
    pub boundary_changed_samples: [[u64; 3]; 3],
}

#[derive(Clone, Copy)]
struct ReferenceState {
    poc: i32,
    references: References,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MotionReference {
    reference: u8,
    index: u8,
    vector: MotionVector,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Motion {
    L0(MotionReference),
    L1(MotionReference),
    Bi([MotionReference; 2]),
}

impl Motion {
    fn lists(self) -> [Option<MotionReference>; 2] {
        match self {
            Self::L0(value) => [Some(value), None],
            Self::L1(value) => [None, Some(value)],
            Self::Bi([first, second]) => [Some(first), Some(second)],
        }
    }
}

pub(crate) trait ReclaimMemory {
    fn retained_bytes(&mut self, mandatory: usize, limit: usize) -> Result<usize>;
}

pub(crate) struct Memory<'a> {
    pub(crate) used: usize,
    limit: usize,
    peak: usize,
    reclaim: Option<&'a mut dyn ReclaimMemory>,
}

impl<'a> Memory<'a> {
    pub(crate) fn new(used: usize, limit: usize) -> Result<Self> {
        Self::with_reclaim(used, limit, None)
    }

    pub(crate) fn with_reclaim(
        used: usize,
        limit: usize,
        reclaim: Option<&'a mut dyn ReclaimMemory>,
    ) -> Result<Self> {
        let mut memory = Self {
            used: 0,
            limit,
            peak: 0,
            reclaim,
        };
        memory.charge(used)?;
        Ok(memory)
    }

    pub(crate) fn peak(&self) -> usize {
        self.peak
    }

    pub(crate) fn charge(&mut self, bytes: usize) -> Result<()> {
        let mandatory = self
            .used
            .checked_add(bytes)
            .filter(|n| *n <= self.limit)
            .ok_or(Error::Limit("reconstruction_memory"))?;
        let retained = match &mut self.reclaim {
            Some(reclaim) => reclaim.retained_bytes(mandatory, self.limit)?,
            None => 0,
        };
        let physical = mandatory
            .checked_add(retained)
            .filter(|n| *n <= self.limit)
            .ok_or(Error::Limit("reconstruction_memory"))?;
        self.used = mandatory;
        self.peak = self.peak.max(physical);
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
    motion: Option<Motion>,
    skipped: bool,
    luma_residual: bool,
    depth: u8,
    reconstructed: u8,
    qp: i8,
    slice: u32,
    edges: u8,
    transform_edges: u8,
    protected: bool,
}

impl Cell {
    const fn empty() -> Self {
        Self {
            mode: None,
            motion: None,
            skipped: false,
            luma_residual: false,
            depth: 0,
            reconstructed: 0,
            qp: 0,
            slice: 0,
            edges: 0,
            transform_edges: 0,
            protected: false,
        }
    }
}

struct Scratch {
    coefficients: Vec<i16>,
    residual: Vec<i32>,
    prediction: Vec<u16>,
    output: Vec<u16>,
    scaling: Vec<u8>,
    references: Vec<Option<u16>>,
    predictors: [Vec<i32>; 2],
    inter_output: Vec<u16>,
}

#[cfg(feature = "native-hevc-rows-private")]
pub(crate) struct ContinuationCapacity {
    pub(crate) retained: usize,
    pub(crate) base: usize,
    pub(crate) row_extra: usize,
}

#[cfg(feature = "native-hevc-rows-private")]
pub(crate) fn qualify_capacities(
    info: PictureInfo,
    ctus: usize,
    probe: &mut crate::native_rows::CapacityProbe,
) -> bool {
    let width = info.width as usize;
    let height = info.height as usize;
    let Some(area) = width.checked_mul(height) else {
        return false;
    };
    let Some(ctb) = 1usize.checked_shl(info.ctb_log2) else {
        return false;
    };
    let Some(band) = ctb.checked_mul(2).and_then(|n| n.checked_add(4)) else {
        return false;
    };
    let Some(band_area) = width.checked_mul(band.min(height)) else {
        return false;
    };
    [
        area,
        area / 4,
        band_area,
        band_area / 4,
        width,
        width / 2,
        1024,
        4096,
    ]
    .into_iter()
    .all(|count| probe.check::<u16>(count))
        && [area / 16, band_area / 16]
            .into_iter()
            .all(|count| probe.check::<Cell>(count))
        && probe.check::<filter::Ctu>(ctus)
        && probe.check::<i16>(1024)
        && [1024, 4096]
            .into_iter()
            .all(|count| probe.check::<i32>(count))
        && probe.check::<u8>(1024)
        && probe.check::<Option<u16>>(129)
        && parallel::qualify_capacities(info, probe)
}

#[cfg(feature = "native-hevc-rows-private")]
pub(crate) fn continuation_capacity(
    info: PictureInfo,
    ctus: usize,
) -> Result<ContinuationCapacity> {
    let overflow = || Error::Limit("reconstruction_memory");
    let width = info.width as usize;
    let height = info.height as usize;
    let area = width.checked_mul(height).ok_or_else(overflow)?;
    let retained = area
        .checked_mul(3)
        .and_then(|n| n.checked_add((area / 16).checked_mul(core::mem::size_of::<Cell>())?))
        .ok_or_else(overflow)?;
    let scratch = 1024
        * (core::mem::size_of::<i16>()
            + core::mem::size_of::<i32>()
            + 2 * core::mem::size_of::<u16>()
            + core::mem::size_of::<u8>())
        + 129 * core::mem::size_of::<Option<u16>>()
        + 2 * 4096 * core::mem::size_of::<i32>()
        + 4096 * core::mem::size_of::<u16>();
    let base = ctus
        .checked_mul(core::mem::size_of::<filter::Ctu>())
        .and_then(|n| n.checked_add(scratch))
        .ok_or_else(overflow)?;
    let band = 1usize
        .checked_shl(info.ctb_log2)
        .and_then(|n| n.checked_mul(2))
        .and_then(|n| n.checked_add(4))
        .ok_or_else(overflow)?
        .min(height);
    let band_area = width.checked_mul(band).ok_or_else(overflow)?;
    let row_extra = band_area
        .checked_mul(5)
        .and_then(|n| n.checked_add((band_area / 16).checked_mul(core::mem::size_of::<Cell>())?))
        .and_then(|n| n.checked_add(width.checked_mul(8)?))
        .ok_or_else(overflow)?;
    Ok(ContinuationCapacity {
        retained,
        base,
        row_extra,
    })
}

impl Scratch {
    fn heap_bytes(&self) -> Result<usize> {
        let bytes = [
            allocation_bytes(&self.coefficients)?,
            allocation_bytes(&self.residual)?,
            allocation_bytes(&self.prediction)?,
            allocation_bytes(&self.output)?,
            allocation_bytes(&self.scaling)?,
            allocation_bytes(&self.references)?,
            allocation_bytes(&self.inter_output)?,
        ]
        .into_iter()
        .try_fold(0usize, |total, bytes| {
            total
                .checked_add(bytes)
                .ok_or(Error::Limit("reconstruction_memory"))
        })?;
        self.predictors.iter().try_fold(bytes, |total, predictor| {
            total
                .checked_add(allocation_bytes(predictor)?)
                .ok_or(Error::Limit("reconstruction_memory"))
        })
    }
}

fn allocation_bytes<T>(values: &Vec<T>) -> Result<usize> {
    values
        .capacity()
        .checked_mul(core::mem::size_of::<T>())
        .ok_or(Error::Limit("reconstruction_memory"))
}

pub(crate) struct Frame {
    #[cfg(test)]
    test_hook: Option<std::sync::Arc<parallel::TestHook>>,
    pub(crate) workers: usize,
    pub(crate) deadline: Option<std::time::Instant>,
    index: u64,
    info: PictureInfo,
    depth: BitDepth,
    planes: [Vec<u16>; 3],
    cells: Vec<Cell>,
    filter_ctus: Vec<filter::Ctu>,
    filtered: bool,
    next_ctu: u32,
    segment_closed: bool,
    contexts: syntax::Contexts,
    wpp: Option<syntax::Checkpoint>,
    qp: syntax::Qp,
    pub(crate) stats: IntraStats,
    inter_stats: InterStats,
    reference: Option<ReferenceState>,
    hash_seen: bool,
    scratch: Scratch,
}

pub(crate) struct RetainedPicture {
    index: u64,
    info: PictureInfo,
    planes: [Vec<u16>; 3],
    cells: Vec<Cell>,
    timestamp: Option<i64>,
    origin: Option<u64>,
    stats: IntraStats,
    inter_stats: InterStats,
    reference: Option<ReferenceState>,
}

impl RetainedPicture {
    #[cfg(test)]
    pub(crate) fn for_test(index: u64, info: PictureInfo, timestamp: Option<i64>) -> Self {
        assert!((8..=64).contains(&info.width) && (8..=64).contains(&info.height));
        assert!(matches!(info.bit_depth, 8 | 10));
        let width = info.width as usize;
        let height = info.height as usize;
        Self {
            index,
            info,
            planes: [
                vec![42; width * height],
                vec![43; width * height / 4],
                vec![44; width * height / 4],
            ],
            cells: vec![
                Cell {
                    mode: IntraMode::new(1).ok(),
                    motion: None,
                    skipped: false,
                    luma_residual: false,
                    depth: 0,
                    reconstructed: 7,
                    qp: 26,
                    slice: 1,
                    edges: 0,
                    transform_edges: 0,
                    protected: false,
                };
                width * height / 16
            ],
            timestamp,
            origin: None,
            stats: IntraStats::default(),
            inter_stats: InterStats::default(),
            reference: None,
        }
    }

    pub(crate) fn index(&self) -> u64 {
        self.index
    }

    pub(crate) fn info(&self) -> PictureInfo {
        self.info
    }

    pub(crate) fn timestamp(&self) -> Option<i64> {
        self.timestamp
    }

    pub(crate) fn origin(&self) -> Option<u64> {
        self.origin
    }

    pub(crate) fn inter_stats(&self) -> InterStats {
        self.inter_stats
    }

    pub(crate) fn heap_bytes(&self) -> Result<usize> {
        sample_bytes(&self.planes, self.cells.capacity())
    }

    pub(crate) fn view(&self) -> IntraPicture<'_> {
        picture_view(self.index, self.info, self.stats, &self.planes)
    }
}

fn sample_bytes(planes: &[Vec<u16>; 3], cells: usize) -> Result<usize> {
    planes
        .iter()
        .try_fold(0usize, |n, p| n.checked_add(p.capacity()))
        .and_then(|n| n.checked_mul(core::mem::size_of::<u16>()))
        .and_then(|n| {
            cells
                .checked_mul(core::mem::size_of::<Cell>())
                .and_then(|m| n.checked_add(m))
        })
        .ok_or(Error::Limit("reconstruction_memory"))
}

impl Frame {
    pub(crate) fn index(&self) -> u64 {
        self.index
    }

    pub(crate) fn info(&self) -> PictureInfo {
        self.info
    }

    pub(crate) fn new(input: &SliceInput<'_>, fixed: usize, limit: usize) -> Result<Self> {
        Self::admit(input)?;
        Self::allocate(input, None, fixed, limit, None)
    }

    pub(crate) fn new_full(
        input: &SliceInput<'_>,
        poc: i32,
        references: References,
        fixed: usize,
        limit: usize,
        reclaim: Option<&mut dyn ReclaimMemory>,
    ) -> Result<Self> {
        Self::allocate(
            input,
            Some(ReferenceState { poc, references }),
            fixed,
            limit,
            reclaim,
        )
    }

    fn allocate(
        input: &SliceInput<'_>,
        reference: Option<ReferenceState>,
        fixed: usize,
        limit: usize,
        reclaim: Option<&mut dyn ReclaimMemory>,
    ) -> Result<Self> {
        if !input.header.first || input.header.address != 0 {
            return Err(Error::InvalidData("selected_picture_start"));
        }
        let mut memory = Memory::with_reclaim(fixed, limit, reclaim)?;
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
            deadline: None,
            #[cfg(test)]
            test_hook: None,
            workers: 1,
            index: input.index,
            info: input.picture,
            depth: BitDepth::new(input.picture.bit_depth as u8)?,
            planes: [
                memory.allocate(luma, 0)?,
                memory.allocate(chroma, 0)?,
                memory.allocate(chroma, 0)?,
            ],
            cells: memory.allocate(cells, Cell::empty())?,
            filter_ctus: memory.allocate(input.sps.ctu_count as usize, filter::Ctu::default())?,
            filtered: false,
            next_ctu: 0,
            segment_closed: false,
            contexts: syntax::Contexts::new(input.syntax.qp_y, input.syntax.initialization),
            wpp: None,
            qp: syntax::Qp::new(input.syntax.qp_y),
            stats: IntraStats::default(),
            inter_stats: InterStats::default(),
            reference,
            hash_seen: false,
            scratch: Scratch {
                coefficients: memory.allocate(1024, 0)?,
                residual: memory.allocate(1024, 0)?,
                prediction: memory.allocate(1024, 0)?,
                output: memory.allocate(1024, 0)?,
                scaling: memory.allocate(1024, 16)?,
                references: memory.allocate(129, None)?,
                predictors: [
                    memory.allocate(if reference.is_some() { 4096 } else { 0 }, 0)?,
                    memory.allocate(if reference.is_some() { 4096 } else { 0 }, 0)?,
                ],
                inter_output: memory.allocate(if reference.is_some() { 4096 } else { 0 }, 0)?,
            },
        })
    }

    fn admit(input: &SliceInput<'_>) -> Result<()> {
        if input.syntax.kind != 2 || input.syntax.initialization != crate::cabac::Initialization::I
        {
            return Err(Error::Unsupported("selected_inter_picture"));
        }
        Ok(())
    }

    fn check_deadline(&self) -> Result<()> {
        if self
            .deadline
            .is_some_and(|deadline| std::time::Instant::now() >= deadline)
        {
            return Err(Error::Limit("decode_deadline"));
        }
        Ok(())
    }

    pub(crate) fn heap_bytes(&self) -> Result<usize> {
        let scratch = self.scratch.heap_bytes()?;
        sample_bytes(&self.planes, self.cells.capacity())?
            .checked_add(
                self.filter_ctus
                    .capacity()
                    .checked_mul(core::mem::size_of::<filter::Ctu>())
                    .ok_or(Error::Limit("reconstruction_memory"))?,
            )
            .and_then(|n| n.checked_add(scratch))
            .ok_or(Error::Limit("reconstruction_memory"))
    }

    pub(crate) fn complete(&self) -> bool {
        let ctb = 1 << self.info.ctb_log2;
        self.segment_closed
            && self.next_ctu == self.info.width.div_ceil(ctb) * self.info.height.div_ceil(ctb)
            && self.cells.iter().all(|c| c.reconstructed == 7)
    }

    pub(crate) fn hash(&mut self, sei: SeiInfo, fixed: usize, limit: usize) -> Result<usize> {
        self.hash_reclaiming(sei, fixed, limit, None)
    }

    pub(crate) fn hash_reclaiming(
        &mut self,
        sei: SeiInfo,
        fixed: usize,
        limit: usize,
        reclaim: Option<&mut dyn ReclaimMemory>,
    ) -> Result<usize> {
        if sei.picture_hash.is_none() && sei.reserved_hashes == 0 {
            return Ok(0);
        }
        let peak = self.finalize_reclaiming(fixed, limit, reclaim)?;
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
        Ok(peak)
    }

    pub(crate) fn view(&self) -> IntraPicture<'_> {
        picture_view(self.index, self.info, self.stats, &self.planes)
    }

    pub(crate) fn retain(
        self,
        timestamp: Option<i64>,
        origin: Option<u64>,
    ) -> Result<RetainedPicture> {
        if !self.complete() || !self.filtered {
            return Err(Error::InvalidData("unfinalized_reference_picture"));
        }
        Ok(RetainedPicture {
            index: self.index,
            info: self.info,
            planes: self.planes,
            cells: self.cells,
            timestamp,
            origin,
            stats: self.stats,
            inter_stats: self.inter_stats,
            reference: self.reference,
        })
    }

    pub(crate) fn inter_stats(&self) -> InterStats {
        self.inter_stats
    }
}

fn picture_view(
    index: u64,
    info: PictureInfo,
    stats: IntraStats,
    planes: &[Vec<u16>; 3],
) -> IntraPicture<'_> {
    let width = info.width as usize;
    let height = info.height as usize;
    let [y, cb, cr] = planes;
    IntraPicture {
        index,
        info,
        stats,
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

impl From<kernels::Error> for Error {
    fn from(_: kernels::Error) -> Self {
        Self::InvalidData("intra_kernel_domain")
    }
}
