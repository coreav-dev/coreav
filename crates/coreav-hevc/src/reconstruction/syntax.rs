use super::{
    Cell, Frame, Memory, Motion,
    filter::{Ctu, Sao},
    inter::{self, Partition, Pu},
};
use crate::{
    Error, Result,
    cabac::{Cabac, Context, Initialization},
    ctu::Location,
    parser::SliceInput,
    residual, substream,
};
use core::ops::Range;
use coreav_kernels_hevc::{
    self as kernels, Block, BlockMut, BlockSize, Component, IntraMode, IntraPrediction,
    InverseTransform, Reconstruction,
};

#[derive(Clone, Copy)]
pub(super) struct Contexts {
    split_cu: [Context; 3],
    bypass: Context,
    part: [Context; 4],
    inter: Option<InterContexts>,
    previous_mode: Context,
    chroma_mode: Context,
    split_transform: [Context; 3],
    cbf_luma: [Context; 2],
    cbf_chroma: [Context; 4],
    qp_delta: [Context; 2],
    residual: residual::Contexts,
    sao_merge: Context,
    sao_type: Context,
}

#[derive(Clone, Copy)]
struct InterContexts {
    skip: [Context; 3],
    pred_mode: Context,
    root_cbf: Context,
    merge: Context,
    merge_idx: Context,
    direction: [Context; 5],
    ref_idx: [Context; 2],
    mvp: Context,
    mvd: [Context; 2],
}

impl InterContexts {
    fn new(qp: i32, initialization: Initialization) -> Option<Self> {
        if initialization == Initialization::I {
            return None;
        }
        Some(Self {
            skip: [197, 185, 201].map(|value| Context::new(value, qp)),
            pred_mode: Context::new(initialization.select([154, 149, 134]), qp),
            root_cbf: Context::new(79, qp),
            merge: Context::new(initialization.select([154, 110, 154]), qp),
            merge_idx: Context::new(initialization.select([154, 122, 137]), qp),
            direction: [95, 79, 63, 31, 31].map(|value| Context::new(value, qp)),
            ref_idx: [Context::new(153, qp); 2],
            mvp: Context::new(168, qp),
            mvd: initialization
                .select([[154; 2], [140, 198], [169, 198]])
                .map(|value| Context::new(value, qp)),
        })
    }
}

impl Contexts {
    pub(super) fn new(slice_qp: i32, initialization: Initialization) -> Self {
        Self {
            split_cu: initialization
                .select([[139, 141, 157], [107, 139, 126], [107, 139, 126]])
                .map(|value| Context::new(value, slice_qp)),
            bypass: Context::new(154, slice_qp),
            part: initialization
                .select([
                    [184, 154, 154, 154],
                    [154, 139, 154, 154],
                    [154, 139, 154, 154],
                ])
                .map(|value| Context::new(value, slice_qp)),
            inter: InterContexts::new(slice_qp, initialization),
            previous_mode: Context::new(initialization.select([184, 154, 183]), slice_qp),
            chroma_mode: Context::new(initialization.select([63, 152, 152]), slice_qp),
            split_transform: initialization
                .select([[153, 138, 138], [124, 138, 94], [224, 167, 122]])
                .map(|value| Context::new(value, slice_qp)),
            cbf_luma: initialization
                .select([[111, 141], [153, 111], [153, 111]])
                .map(|value| Context::new(value, slice_qp)),
            cbf_chroma: initialization
                .select([
                    [94, 138, 182, 154],
                    [149, 107, 167, 154],
                    [149, 92, 167, 154],
                ])
                .map(|value| Context::new(value, slice_qp)),
            qp_delta: [Context::new(154, slice_qp); 2],
            residual: residual::Contexts::new(slice_qp, initialization),
            sao_merge: Context::new(153, slice_qp),
            sao_type: Context::new(initialization.select([200, 185, 160]), slice_qp),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Checkpoint {
    contexts: Contexts,
    tile: u32,
    row: u32,
    slice: u32,
}

#[derive(Clone, Copy)]
pub(super) struct Qp {
    current: i32,
    predictor: i32,
    origin: Option<[usize; 2]>,
    coded: bool,
}

impl Qp {
    pub(super) fn new(slice_qp: i32) -> Self {
        Self {
            current: slice_qp,
            predictor: slice_qp,
            origin: None,
            coded: false,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Area {
    pub(super) x: usize,
    pub(super) y: usize,
    pub(super) log2: u32,
}

impl Area {
    pub(super) fn side(self) -> usize {
        1 << self.log2
    }

    fn children(self) -> Result<[Self; 4]> {
        let log2 = self
            .log2
            .checked_sub(1)
            .ok_or(Error::InvalidData("intra_tree_depth"))?;
        let half = 1 << log2;
        Ok([
            Self {
                x: self.x,
                y: self.y,
                log2,
            },
            Self {
                x: self.x + half,
                y: self.y,
                log2,
            },
            Self {
                x: self.x,
                y: self.y + half,
                log2,
            },
            Self {
                x: self.x + half,
                y: self.y + half,
                log2,
            },
        ])
    }
}

#[derive(Clone, Copy)]
struct Cu {
    area: Area,
    nxn: bool,
    bypass: bool,
    chroma_mode: Option<IntraMode>,
    inter_split: bool,
}

#[derive(Clone, Copy)]
struct Transform {
    area: Area,
    base: [usize; 2],
    depth: u32,
    index: usize,
    parent_chroma: [bool; 2],
}

struct ReconstructionBlock {
    area: Area,
    component: usize,
    mode: Option<IntraMode>,
    coded: bool,
    bypass: bool,
}

trait Bins {
    fn decision(&mut self, context: &mut Context) -> Result<bool>;
    fn bypass(&mut self) -> Result<bool>;
}

impl Bins for Cabac<'_, '_> {
    fn decision(&mut self, context: &mut Context) -> Result<bool> {
        Cabac::decision(self, context)
    }

    fn bypass(&mut self) -> Result<bool> {
        Cabac::bypass(self)
    }
}

fn decision(cabac: &mut impl Bins, contexts: &mut [Context], index: usize) -> Result<bool> {
    cabac.decision(
        contexts
            .get_mut(index)
            .ok_or(Error::InvalidData("intra_context"))?,
    )
}

fn bypass_bits(cabac: &mut impl Bins, width: u32) -> Result<u32> {
    if width > 32 {
        return Err(Error::InvalidData("intra_bin_width"));
    }
    let mut value = 0;
    for _ in 0..width {
        value = (value << 1) | u32::from(cabac.bypass()?);
    }
    Ok(value)
}

fn partition(
    bins: &mut impl Bins,
    contexts: &mut [Context; 4],
    log2: u32,
    minimum: u32,
    amp: bool,
) -> Result<Partition> {
    if !(3..=6).contains(&minimum) || !(minimum..=6).contains(&log2) {
        return Err(Error::InvalidData("inter_partition_size"));
    }
    if decision(bins, contexts, 0)? {
        return Ok(Partition::Full);
    }
    let horizontal = decision(bins, contexts, 1)?;
    if log2 == minimum {
        return Ok(if horizontal {
            Partition::Horizontal
        } else if log2 == 3 || decision(bins, contexts, 2)? {
            Partition::Vertical
        } else {
            Partition::Quad
        });
    }
    if !amp || decision(bins, contexts, 3)? {
        return Ok(if horizontal {
            Partition::Horizontal
        } else {
            Partition::Vertical
        });
    }
    Ok(match (horizontal, bins.bypass()?) {
        (true, false) => Partition::Up,
        (true, true) => Partition::Down,
        (false, false) => Partition::Left,
        (false, true) => Partition::Right,
    })
}

fn merge_index(bins: &mut impl Bins, context: &mut Context, count: u8) -> Result<u8> {
    if !(1..=5).contains(&count) {
        return Err(Error::InvalidData("inter_merge_count"));
    }
    let mut index = 0;
    while index + 1 < count {
        let more = if index == 0 {
            bins.decision(context)?
        } else {
            bins.bypass()?
        };
        if !more {
            break;
        }
        index += 1;
    }
    Ok(index)
}

fn reference_index(bins: &mut impl Bins, contexts: &mut [Context; 2], count: u8) -> Result<u8> {
    if !(1..=15).contains(&count) {
        return Err(Error::InvalidData("inter_reference_count"));
    }
    let mut index = 0;
    while index + 1 < count {
        let more = if index < 2 {
            decision(bins, contexts, usize::from(index))?
        } else {
            bins.bypass()?
        };
        if !more {
            break;
        }
        index += 1;
    }
    Ok(index)
}

fn inter_direction(
    bins: &mut impl Bins,
    contexts: &mut [Context; 5],
    depth: u8,
    pu: Pu,
) -> Result<u8> {
    if depth > 3 || !(4..=64).contains(&pu.width) || !(4..=64).contains(&pu.height) {
        return Err(Error::InvalidData("inter_direction"));
    }
    if pu.width + pu.height != 12 && decision(bins, contexts, usize::from(depth))? {
        return Ok(3);
    }
    Ok(if decision(bins, contexts, 4)? { 2 } else { 1 })
}

fn motion_difference(
    bins: &mut impl Bins,
    contexts: &mut [Context; 2],
) -> Result<kernels::MotionVector> {
    let nonzero = [decision(bins, contexts, 0)?, decision(bins, contexts, 0)?];
    let mut greater1 = [false; 2];
    for (greater, nonzero) in greater1.iter_mut().zip(nonzero) {
        *greater = nonzero && decision(bins, contexts, 1)?;
    }
    let mut components = [0i16; 2];
    for ((component, nonzero), greater) in components.iter_mut().zip(nonzero).zip(greater1) {
        if !nonzero {
            continue;
        }
        let mut magnitude = 1;
        if greater {
            let mut value = 0u32;
            let mut width = 1;
            while bins.bypass()? {
                value += 1 << width;
                if value > 32766 {
                    return Err(Error::InvalidData("inter_mvd"));
                }
                width += 1;
            }
            magnitude = 2 + value + bypass_bits(bins, width)?;
            if magnitude > 32768 {
                return Err(Error::InvalidData("inter_mvd"));
            }
        }
        let signed = if bins.bypass()? {
            -(magnitude as i32)
        } else {
            magnitude as i32
        };
        *component = i16::try_from(signed).map_err(|_| Error::InvalidData("inter_mvd"))?;
    }
    let [x, y] = components;
    Ok(kernels::MotionVector { x, y })
}

fn alignment(cabac: &mut Cabac<'_, '_>) -> Result<()> {
    while cabac.position() % 8 != 0 {
        if cabac.raw_bits(1)? != 0 {
            return Err(Error::InvalidData("intra_alignment_zero"));
        }
    }
    Ok(())
}

pub(super) fn end_range(cabac: &mut Cabac<'_, '_>, bytes: usize, final_slice: bool) -> Result<()> {
    alignment(cabac)?;
    let remaining = bytes
        .checked_sub(cabac.position() / 8)
        .ok_or(Error::InvalidData("intra_substream_end"))?;
    if !final_slice && remaining != 0 || remaining & 1 != 0 {
        return Err(Error::InvalidData("intra_substream_end"));
    }
    for _ in 0..remaining / 2 {
        if cabac.raw_bits(16)? != 0 {
            return Err(Error::InvalidData("cabac_zero_word"));
        }
    }
    Ok(())
}

fn mpm_candidates(left: u8, above: u8) -> [u8; 3] {
    if left == above {
        if left < 2 {
            [0, 1, 26]
        } else {
            [left, 2 + (left + 29) % 32, 2 + (left - 1) % 32]
        }
    } else {
        [
            left,
            above,
            if left != 0 && above != 0 {
                0
            } else if left != 1 && above != 1 {
                1
            } else {
                26
            },
        ]
    }
}

fn remaining_mode(mut candidates: [u8; 3], mut remaining: u8) -> u8 {
    candidates.sort_unstable();
    for candidate in candidates {
        if remaining >= candidate {
            remaining += 1;
        }
    }
    remaining
}

fn chroma_mode(index: u32, luma: u8) -> Result<IntraMode> {
    let mode = match index {
        0 => 0,
        1 => 26,
        2 => 10,
        3 => 1,
        4 => return Ok(IntraMode::new(luma)?),
        _ => return Err(Error::InvalidData("intra_chroma_mode")),
    };
    Ok(IntraMode::new(if mode == luma { 34 } else { mode })?)
}

fn luma_qp(predictor: i32, delta: i32, offset: i32) -> i32 {
    (predictor + delta + 52 + 2 * offset).rem_euclid(52 + offset) - offset
}

fn component_qp(qp: i32, offset: i32, chroma_offset: Option<i32>) -> Result<u8> {
    let qp = if let Some(chroma_offset) = chroma_offset {
        let index = (qp + chroma_offset).clamp(-offset, 57);
        kernels::chroma_qp_index(index)?
    } else {
        qp
    };
    u8::try_from(qp + offset).map_err(|_| Error::InvalidData("intra_component_qp"))
}

impl Frame {
    fn read_sao(
        &mut self,
        input: &SliceInput<'_>,
        location: Location,
        slice: u32,
        cabac: &mut Cabac<'_, '_>,
    ) -> Result<()> {
        let raster = location.y * input.layout.width + location.x;
        let mut sao = [Sao::default(); 3];
        let mut merged = false;
        if input.syntax.sao.into_iter().any(|v| v) {
            let left = location.x > 0
                && raster > input.syntax.independent_address
                && input.layout.at_raster(raster - 1)?.tile == location.tile;
            let above = location.y > 0
                && raster - input.layout.width >= input.syntax.independent_address
                && input.layout.at_raster(raster - input.layout.width)?.tile == location.tile;
            let merge = if left && cabac.decision(&mut self.contexts.sao_merge)? {
                Some(raster - 1)
            } else if above && cabac.decision(&mut self.contexts.sao_merge)? {
                Some(raster - input.layout.width)
            } else {
                None
            };
            if let Some(index) = merge {
                sao = self
                    .filter_ctus
                    .get(index as usize)
                    .filter(|c| c.seen)
                    .ok_or(Error::InvalidData("sao_merge_source"))?
                    .sao;
                merged = true;
            } else {
                for component in 0..3 {
                    if !input
                        .syntax
                        .sao
                        .get(usize::from(component != 0))
                        .copied()
                        .ok_or(Error::InvalidData("sao_component"))?
                    {
                        continue;
                    }
                    let kind = if component == 2 {
                        sao[1].kind
                    } else if cabac.decision(&mut self.contexts.sao_type)? {
                        1 + u8::from(cabac.bypass()?)
                    } else {
                        0
                    };
                    let mut value = Sao {
                        kind,
                        ..Sao::default()
                    };
                    if value.kind != 0 {
                        let max = (1i16 << (input.sps.bit_depth.min(10) - 5)) - 1;
                        for offset in &mut value.offsets {
                            while *offset < max && cabac.bypass()? {
                                *offset += 1;
                            }
                        }
                        if value.kind == 1 {
                            for offset in &mut value.offsets {
                                if *offset != 0 && cabac.bypass()? {
                                    *offset = -*offset;
                                }
                            }
                            value.band = bypass_bits(cabac, 5)? as u8;
                        } else {
                            for offset in value.offsets.iter_mut().skip(2) {
                                *offset = -*offset;
                            }
                            value.class = if component == 2 {
                                sao[1].class
                            } else {
                                bypass_bits(cabac, 2)? as u8
                            };
                        }
                    }
                    *sao.get_mut(component)
                        .ok_or(Error::InvalidData("sao_component"))? = value;
                }
            }
        }
        self.stats.sao_merges += u64::from(merged);
        *self
            .filter_ctus
            .get_mut(raster as usize)
            .ok_or(Error::InvalidData("filter_ctu_address"))? = Ctu {
            seen: true,
            order: location.tile_scan,
            tile: location.tile,
            slice,
            across_tiles: input.pps.loop_filter_tiles,
            across_slices: input.syntax.loop_filter_slices,
            deblock_disabled: input.syntax.deblock_disabled,
            offsets: input.syntax.deblock_offsets,
            chroma_offsets: [input.pps.cb_qp_offset, input.pps.cr_qp_offset],
            sao,
        };
        Ok(())
    }

    pub(crate) fn decode(
        &mut self,
        input: SliceInput<'_>,
        remaining: &mut u64,
        fixed: usize,
        limit: usize,
    ) -> Result<usize> {
        Self::admit(&input)?;
        self.decode_common(input, None, remaining, fixed, limit, 1)
    }

    pub(crate) fn decode_full(
        &mut self,
        input: SliceInput<'_>,
        dpb: &crate::dpb::Dpb,
        remaining: &mut u64,
        fixed: usize,
        limit: usize,
        threads: usize,
    ) -> Result<usize> {
        if self.reference.is_none()
            || input.syntax.kind > 2
            || input.syntax.kind < 2 && input.syntax.inter.is_none()
        {
            return Err(Error::InvalidData("inter_segment_state"));
        }
        self.decode_common(input, Some(dpb), remaining, fixed, limit, threads)
    }

    fn decode_common(
        &mut self,
        input: SliceInput<'_>,
        dpb: Option<&crate::dpb::Dpb>,
        remaining: &mut u64,
        fixed: usize,
        limit: usize,
        threads: usize,
    ) -> Result<usize> {
        if self.index != input.index
            || self.info != input.picture
            || self.hash_seen
            || self.filtered
            || self.next_ctu != 0 && !self.segment_closed
        {
            return Err(Error::InvalidData("selected_segment_state"));
        }
        let start = input.layout.at_raster(input.header.address)?;
        if start.tile_scan != self.next_ctu || input.header.first != (self.next_ctu == 0) {
            return Err(Error::InvalidData("intra_ctu_order"));
        }
        let count = input
            .entry_lengths
            .len()
            .checked_add(1)
            .ok_or(Error::Limit("reconstruction_memory"))?;
        let total_ctus = input.layout.width * input.layout.height;
        if count > (total_ctus - self.next_ctu) as usize {
            return Err(Error::InvalidData("intra_substream_count"));
        }
        let mut memory = Memory::new(fixed, limit)?;
        memory.charge(self.heap_bytes()?)?;
        memory.charge(input.scratch_bytes)?;
        memory.charge(
            count
                .checked_mul(core::mem::size_of::<Range<usize>>())
                .ok_or(Error::Limit("reconstruction_memory"))?,
        )?;
        let ranges = substream::ranges(
            input.original,
            input.header.rbsp_header_bytes,
            input.entry_lengths,
            count,
        )?;
        memory.charge(
            (ranges.capacity() - count)
                .checked_mul(core::mem::size_of::<Range<usize>>())
                .ok_or(Error::Limit("reconstruction_memory"))?,
        )?;
        if let Some(peak) =
            self.decode_wpp(&input, dpb, remaining, &ranges, memory.used, limit, threads)?
        {
            return Ok(peak);
        }
        if !input.header.dependent {
            self.contexts = Contexts::new(input.syntax.qp_y, input.syntax.initialization);
            self.qp = Qp::new(input.syntax.qp_y);
            self.wpp = None;
        } else {
            self.stats.dependent_segments += 1;
        }
        self.segment_closed = false;
        let slice = input
            .syntax
            .independent_address
            .checked_add(1)
            .ok_or(Error::InvalidData("intra_slice_address"))?;
        for (range_index, range) in ranges.iter().enumerate() {
            let bytes = input
                .rbsp
                .get(range.clone())
                .ok_or(Error::InvalidData("intra_substream_range"))?;
            let mut cabac = Cabac::new(bytes, remaining)?;
            loop {
                let location = self.decode_ctu(&input, dpb, slice, &mut cabac)?;
                if cabac.terminate()? {
                    if range_index + 1 != ranges.len() {
                        return Err(Error::InvalidData("intra_unconsumed_substreams"));
                    }
                    end_range(&mut cabac, bytes.len(), true)?;
                    self.segment_closed = true;
                    return Ok(memory.used);
                }
                if self.next_ctu == total_ctus {
                    return Err(Error::InvalidData("intra_picture_termination"));
                }
                let next = input.layout.at_tile_scan(self.next_ctu)?;
                let new_subset = next.tile != location.tile
                    || input.pps.wpp
                        && next.x
                            == *next
                                .tile_origin
                                .first()
                                .ok_or(Error::InvalidData("tile_grid"))?;
                if new_subset {
                    if !cabac.terminate()? {
                        return Err(Error::InvalidData("end_of_subset_one_bit"));
                    }
                    end_range(&mut cabac, bytes.len(), false)?;
                    break;
                }
            }
        }
        Err(Error::InvalidData("intra_missing_substream"))
    }

    pub(super) fn decode_ctu(
        &mut self,
        input: &SliceInput<'_>,
        dpb: Option<&crate::dpb::Dpb>,
        slice: u32,
        cabac: &mut Cabac<'_, '_>,
    ) -> Result<Location> {
        self.check_deadline()?;
        let location = input.layout.at_tile_scan(self.next_ctu)?;
        #[cfg(test)]
        if let Some(hook) = &self.test_hook {
            hook.compute(location.y, location.x)?;
        }
        self.start_ctu(input, location, slice)?;
        self.read_sao(input, location, slice, cabac)?;
        Walker {
            frame: self,
            input,
            location,
            slice,
            dpb,
        }
        .coding_tree(
            cabac,
            Area {
                x: (location.x << input.sps.ctb_log2) as usize,
                y: (location.y << input.sps.ctb_log2) as usize,
                log2: input.sps.ctb_log2,
            },
            0,
        )?;
        self.stats.ctus += 1;
        self.save_wpp(input, location, slice);
        self.next_ctu += 1;
        Ok(location)
    }

    pub(super) fn cell_index(&self, x: usize, y: usize) -> Result<usize> {
        if x >= self.info.width as usize || y >= self.info.height as usize {
            return Err(Error::InvalidData("intra_cell_address"));
        }
        Ok((y / 4) * (self.info.width as usize / 4) + x / 4)
    }

    pub(super) fn cell(&self, x: usize, y: usize) -> Result<&Cell> {
        self.cells
            .get(self.cell_index(x, y)?)
            .ok_or(Error::InvalidData("intra_cell_address"))
    }

    fn start_ctu(&mut self, input: &SliceInput<'_>, location: Location, slice: u32) -> Result<()> {
        let [tile_x, tile_y] = location.tile_origin;
        if location.x == tile_x && location.y == tile_y {
            self.contexts = Contexts::new(input.syntax.qp_y, input.syntax.initialization);
            self.qp = Qp::new(input.syntax.qp_y);
            self.wpp = None;
            self.stats.tile_restarts += 1;
        } else if input.pps.wpp && location.x == tile_x {
            let above_right = location.x + 1
                < tile_x
                    + *location
                        .tile_size
                        .first()
                        .ok_or(Error::InvalidData("tile_grid"))?
                && self
                    .cell(
                        ((location.x + 1) << input.sps.ctb_log2) as usize,
                        ((location.y - 1) << input.sps.ctb_log2) as usize,
                    )?
                    .slice
                    == slice;
            self.contexts = if above_right {
                self.wpp
                    .filter(|saved| {
                        saved.tile == location.tile
                            && saved.row + 1 == location.y
                            && saved.slice == slice
                    })
                    .ok_or(Error::InvalidData("intra_wpp_checkpoint"))?
                    .contexts
            } else {
                Contexts::new(input.syntax.qp_y, input.syntax.initialization)
            };
            self.qp = Qp::new(input.syntax.qp_y);
            self.stats.wpp_restarts += 1;
        }
        Ok(())
    }

    fn save_wpp(&mut self, input: &SliceInput<'_>, location: Location, slice: u32) {
        let [tile_x, _] = location.tile_origin;
        if input.pps.wpp && location.x == tile_x + 1 {
            self.wpp = Some(Checkpoint {
                contexts: self.contexts,
                tile: location.tile,
                row: location.y,
                slice,
            });
        }
    }
}

pub(super) struct Walker<'f, 'i, 'data> {
    pub(super) frame: &'f mut Frame,
    pub(super) input: &'i SliceInput<'data>,
    pub(super) location: Location,
    pub(super) slice: u32,
    pub(super) dpb: Option<&'i crate::dpb::Dpb>,
}

impl Walker<'_, '_, '_> {
    fn protect(&mut self, area: Area) -> Result<()> {
        for y in (area.y..area.y + area.side()).step_by(4) {
            for x in (area.x..area.x + area.side()).step_by(4) {
                let index = self.frame.cell_index(x, y)?;
                self.frame
                    .cells
                    .get_mut(index)
                    .ok_or(Error::InvalidData("intra_cell_address"))?
                    .protected = true;
            }
        }
        Ok(())
    }

    pub(super) fn neighbour(&self, x: Option<usize>, y: Option<usize>) -> Result<Option<Cell>> {
        let (Some(x), Some(y)) = (x, y) else {
            return Ok(None);
        };
        if x >= self.frame.info.width as usize || y >= self.frame.info.height as usize {
            return Ok(None);
        }
        let cell = *self.frame.cell(x, y)?;
        if cell.slice != self.slice
            || cell.mode.is_none() && cell.motion.is_none() && cell.reconstructed == 0
        {
            return Ok(None);
        }
        let raster = (y as u32 >> self.input.sps.ctb_log2) * self.input.layout.width
            + (x as u32 >> self.input.sps.ctb_log2);
        if self.input.layout.at_raster(raster)?.tile != self.location.tile {
            return Ok(None);
        }
        Ok(Some(cell))
    }

    fn coding_tree(&mut self, cabac: &mut Cabac<'_, '_>, area: Area, depth: u8) -> Result<()> {
        if depth > 6 || area.log2 < self.input.sps.min_cb_log2 || area.log2 > 6 {
            return Err(Error::InvalidData("intra_coding_tree"));
        }
        let inside = area.x + area.side() <= self.frame.info.width as usize
            && area.y + area.side() <= self.frame.info.height as usize;
        let split = if area.log2 > self.input.sps.min_cb_log2 {
            if inside {
                let left = self.neighbour(area.x.checked_sub(1), Some(area.y))?;
                let above = self.neighbour(Some(area.x), area.y.checked_sub(1))?;
                let index = usize::from(left.is_some_and(|cell| cell.depth > depth))
                    + usize::from(above.is_some_and(|cell| cell.depth > depth));
                decision(cabac, &mut self.frame.contexts.split_cu, index)?
            } else {
                true
            }
        } else {
            false
        };
        if split {
            for child in area.children()? {
                if child.x < self.frame.info.width as usize
                    && child.y < self.frame.info.height as usize
                {
                    self.coding_tree(cabac, child, depth + 1)?;
                }
            }
            return Ok(());
        }
        if !inside {
            return Err(Error::InvalidData("intra_cu_extent"));
        }
        self.coding_unit(cabac, area, depth)
    }

    fn begin_qp_group(&mut self, area: Area) -> Result<()> {
        let group_log2 = self
            .input
            .sps
            .ctb_log2
            .checked_sub(self.input.pps.qp_delta_depth)
            .ok_or(Error::InvalidData("intra_qp_group"))?;
        let side = area.side().max(1 << group_log2);
        let origin = [area.x & !(side - 1), area.y & !(side - 1)];
        if self.frame.qp.origin == Some(origin) {
            return Ok(());
        }
        let [x, y] = origin;
        let previous = self.frame.qp.current;
        let ctu_side = 1usize << self.input.sps.ctb_log2;
        let left = if x % ctu_side != 0 {
            self.neighbour(x.checked_sub(1), Some(y))?
                .map(|cell| i32::from(cell.qp))
        } else {
            None
        };
        let above = if y % ctu_side != 0 {
            self.neighbour(Some(x), y.checked_sub(1))?
                .map(|cell| i32::from(cell.qp))
        } else {
            None
        };
        let predictor = (left.unwrap_or(previous) + above.unwrap_or(previous) + 1) >> 1;
        self.frame.qp = Qp {
            current: predictor,
            predictor,
            origin: Some(origin),
            coded: !self.input.pps.qp_delta_enabled,
        };
        Ok(())
    }

    fn initialize_cells(&mut self, area: Area, depth: u8) -> Result<()> {
        for y in (area.y..area.y + area.side()).step_by(4) {
            for x in (area.x..area.x + area.side()).step_by(4) {
                let index = self.frame.cell_index(x, y)?;
                let cell = self
                    .frame
                    .cells
                    .get_mut(index)
                    .ok_or(Error::InvalidData("intra_cell_address"))?;
                if cell.slice != 0 || cell.reconstructed != 0 {
                    return Err(Error::InvalidData("intra_cu_overlap"));
                }
                *cell = Cell {
                    mode: None,
                    motion: None,
                    skipped: false,
                    luma_residual: false,
                    depth,
                    reconstructed: 0,
                    qp: self.frame.qp.current as i8,
                    slice: self.slice,
                    edges: u8::from(x == area.x) | (u8::from(y == area.y) << 1),
                    transform_edges: 0,
                    protected: false,
                };
            }
        }
        Ok(())
    }

    fn set_mode(&mut self, area: Area, mode: IntraMode) -> Result<()> {
        for y in (area.y..area.y + area.side()).step_by(4) {
            for x in (area.x..area.x + area.side()).step_by(4) {
                let index = self.frame.cell_index(x, y)?;
                self.frame
                    .cells
                    .get_mut(index)
                    .ok_or(Error::InvalidData("intra_cell_address"))?
                    .mode = Some(mode);
            }
        }
        Ok(())
    }

    fn luma_mode(
        &mut self,
        cabac: &mut Cabac<'_, '_>,
        area: Area,
        previous: bool,
    ) -> Result<IntraMode> {
        let left = self
            .neighbour(area.x.checked_sub(1), Some(area.y))?
            .and_then(|cell| cell.mode)
            .map_or(1, IntraMode::number);
        let above = if area.y % (1usize << self.input.sps.ctb_log2) == 0 {
            1
        } else {
            self.neighbour(Some(area.x), area.y.checked_sub(1))?
                .and_then(|cell| cell.mode)
                .map_or(1, IntraMode::number)
        };
        let candidates = mpm_candidates(left, above);
        let mode = if previous {
            let index = if cabac.bypass()? {
                1 + usize::from(cabac.bypass()?)
            } else {
                0
            };
            *candidates
                .get(index)
                .ok_or(Error::InvalidData("intra_mpm_index"))?
        } else {
            remaining_mode(candidates, bypass_bits(cabac, 5)? as u8)
        };
        Ok(IntraMode::new(mode)?)
    }

    fn coding_unit(&mut self, cabac: &mut Cabac<'_, '_>, area: Area, depth: u8) -> Result<()> {
        self.begin_qp_group(area)?;
        self.initialize_cells(area, depth)?;
        self.frame.stats.coding_units += 1;
        let bypass =
            self.input.pps.transquant_bypass && cabac.decision(&mut self.frame.contexts.bypass)?;
        if bypass {
            self.frame.stats.transquant_bypass_units += 1;
            self.protect(area)?;
        }
        let skipped = if self.input.syntax.kind != 2 {
            let left = self.neighbour(area.x.checked_sub(1), Some(area.y))?;
            let above = self.neighbour(Some(area.x), area.y.checked_sub(1))?;
            let index = usize::from(left.is_some_and(|cell| cell.skipped))
                + usize::from(above.is_some_and(|cell| cell.skipped));
            decision(
                cabac,
                &mut self
                    .frame
                    .contexts
                    .inter
                    .as_mut()
                    .ok_or(Error::InvalidData("inter_context"))?
                    .skip,
                index,
            )?
        } else {
            false
        };
        let intra = self.input.syntax.kind == 2
            || !skipped
                && cabac.decision(
                    &mut self
                        .frame
                        .contexts
                        .inter
                        .as_mut()
                        .ok_or(Error::InvalidData("inter_context"))?
                        .pred_mode,
                )?;
        if !intra {
            return self.inter_coding_unit(cabac, area, bypass, skipped);
        }
        let nxn = area.log2 == self.input.sps.min_cb_log2
            && !decision(cabac, &mut self.frame.contexts.part, 0)?;
        if !nxn {
            if let Some(pcm) = self.input.sps.pcm {
                if (pcm.min_log2..=pcm.max_log2).contains(&area.log2) && cabac.terminate()? {
                    return self.pcm(cabac, area);
                }
            }
        }
        let mut previous = [false; 4];
        let blocks = if nxn { area.children()? } else { [area; 4] };
        let count = if nxn { 4 } else { 1 };
        for flag in previous.iter_mut().take(count) {
            *flag = cabac.decision(&mut self.frame.contexts.previous_mode)?;
        }
        for (block, previous) in blocks.into_iter().zip(previous).take(count) {
            let mode = self.luma_mode(cabac, block, previous)?;
            self.set_mode(block, mode)?;
        }
        let chroma_index = if cabac.decision(&mut self.frame.contexts.chroma_mode)? {
            bypass_bits(cabac, 2)?
        } else {
            4
        };
        let luma = self
            .frame
            .cell(area.x, area.y)?
            .mode
            .ok_or(Error::InvalidData("intra_luma_mode"))?
            .number();
        let cu = Cu {
            area,
            nxn,
            bypass,
            chroma_mode: Some(chroma_mode(chroma_index, luma)?),
            inter_split: false,
        };
        self.transform_tree(
            cabac,
            cu,
            Transform {
                area,
                base: [area.x, area.y],
                depth: 0,
                index: 0,
                parent_chroma: [false; 2],
            },
        )
    }

    fn inter_coding_unit(
        &mut self,
        cabac: &mut Cabac<'_, '_>,
        area: Area,
        bypass: bool,
        skipped: bool,
    ) -> Result<()> {
        let partition = if skipped {
            Partition::Full
        } else {
            partition(
                cabac,
                &mut self.frame.contexts.part,
                area.log2,
                self.input.sps.min_cb_log2,
                self.input.sps.amp,
            )?
        };
        if skipped {
            for y in (area.y..area.y + area.side()).step_by(4) {
                for x in (area.x..area.x + area.side()).step_by(4) {
                    let index = self.frame.cell_index(x, y)?;
                    self.frame
                        .cells
                        .get_mut(index)
                        .ok_or(Error::InvalidData("inter_cell"))?
                        .skipped = true;
                }
            }
        }
        let (units, count) = partition.units(area)?;
        let mut merged_full = false;
        for (index, pu) in units.into_iter().take(count).enumerate() {
            let merged = self.prediction_unit(cabac, pu, area, partition, index)?;
            merged_full = matches!(partition, Partition::Full) && merged;
        }
        let root = !skipped
            && (merged_full
                || cabac.decision(
                    &mut self
                        .frame
                        .contexts
                        .inter
                        .as_mut()
                        .ok_or(Error::InvalidData("inter_context"))?
                        .root_cbf,
                )?);
        if root {
            self.transform_tree(
                cabac,
                Cu {
                    area,
                    nxn: false,
                    bypass,
                    chroma_mode: None,
                    inter_split: !matches!(partition, Partition::Full),
                },
                Transform {
                    area,
                    base: [area.x, area.y],
                    depth: 0,
                    index: 0,
                    parent_chroma: [false; 2],
                },
            )?;
        } else {
            self.mark(area, 7)?;
        }
        self.frame.inter_stats.skip_units += u64::from(skipped);
        *self
            .frame
            .inter_stats
            .partitions
            .get_mut(partition.index())
            .ok_or(Error::InvalidData("inter_partition"))? += 1;
        Ok(())
    }

    fn prediction_unit(
        &mut self,
        cabac: &mut Cabac<'_, '_>,
        pu: Pu,
        cu: Area,
        partition: Partition,
        index: usize,
    ) -> Result<bool> {
        let syntax = self
            .input
            .syntax
            .inter
            .ok_or(Error::InvalidData("inter_slice"))?;
        let cell = *self.frame.cell(cu.x, cu.y)?;
        let contexts = self
            .frame
            .contexts
            .inter
            .as_mut()
            .ok_or(Error::InvalidData("inter_context"))?;
        if cell.skipped || cabac.decision(&mut contexts.merge)? {
            let merge_index =
                merge_index(cabac, &mut contexts.merge_idx, syntax.max_merge_candidates)?;
            let resolved = inter::merge(self, pu, cu, partition, index, merge_index)?;
            inter::predict(self, pu, resolved.motion, resolved.temporal, true)?;
            return Ok(true);
        }
        let direction = match self.input.syntax.kind {
            0 => inter_direction(cabac, &mut contexts.direction, cell.depth, pu)?,
            1 => 1,
            _ => return Err(Error::InvalidData("inter_slice")),
        };
        let mut lists = [None; 2];
        let mut temporal = false;
        for (list, value) in lists.iter_mut().enumerate() {
            if direction & (1 << list) == 0 {
                continue;
            }
            let contexts = self
                .frame
                .contexts
                .inter
                .as_mut()
                .ok_or(Error::InvalidData("inter_context"))?;
            let count = *syntax
                .active
                .get(list)
                .ok_or(Error::InvalidData("inter_list"))?;
            let reference = reference_index(cabac, &mut contexts.ref_idx, count)?;
            let difference = if list == 1 && direction == 3 && syntax.mvd_l1_zero {
                kernels::MotionVector { x: 0, y: 0 }
            } else {
                motion_difference(cabac, &mut contexts.mvd)?
            };
            let mvp = cabac.decision(&mut contexts.mvp)?;
            let resolved = inter::amvp(
                self, pu, cu, partition, index, list, reference, mvp, difference,
            )?;
            *value = Some(resolved.value);
            temporal |= resolved.temporal;
        }
        let motion = match lists {
            [Some(first), Some(second)] => Motion::Bi([first, second]),
            [Some(first), None] => Motion::L0(first),
            [None, Some(second)] => Motion::L1(second),
            _ => return Err(Error::InvalidData("inter_direction")),
        };
        inter::predict(self, pu, motion, temporal, false)?;
        self.frame.inter_stats.amvp_lists += u64::from(direction.count_ones());
        Ok(false)
    }

    fn pcm(&mut self, cabac: &mut Cabac<'_, '_>, area: Area) -> Result<()> {
        alignment(cabac)?;
        self.set_mode(area, IntraMode::new(1)?)?;
        let pcm = self.input.sps.pcm.ok_or(Error::InvalidData("intra_pcm"))?;
        if pcm.loop_filter_disabled {
            self.protect(area)?;
        }
        for component in 0..3 {
            let scale = if component == 0 { 1 } else { 2 };
            let depth = if component == 0 {
                pcm.luma_depth
            } else {
                pcm.chroma_depth
            };
            let shift = self
                .input
                .sps
                .bit_depth
                .checked_sub(depth)
                .ok_or(Error::InvalidData("intra_pcm_depth"))?;
            let side = area.side() / scale;
            let width = self.frame.info.width as usize / scale;
            let plane = self
                .frame
                .planes
                .get_mut(component)
                .ok_or(Error::InvalidData("intra_plane"))?;
            for y in 0..side {
                for x in 0..side {
                    let sample = (cabac.raw_bits(depth)? << shift) as u16;
                    *plane
                        .get_mut((area.y / scale + y) * width + area.x / scale + x)
                        .ok_or(Error::InvalidData("intra_sample_address"))? = sample;
                }
            }
        }
        self.mark(
            Area {
                x: area.x,
                y: area.y,
                log2: area.log2,
            },
            7,
        )?;
        self.frame.stats.pcm_units += 1;
        cabac.restart()
    }

    fn delta_qp(&mut self, cabac: &mut Cabac<'_, '_>, area: Area) -> Result<()> {
        if self.frame.qp.coded {
            return Ok(());
        }
        let mut magnitude = 0i32;
        while magnitude < 5 {
            if !decision(
                cabac,
                &mut self.frame.contexts.qp_delta,
                usize::from(magnitude != 0),
            )? {
                break;
            }
            magnitude += 1;
        }
        if magnitude == 5 {
            let mut width = 0;
            while cabac.bypass()? {
                if width >= 5 {
                    return Err(Error::InvalidData("intra_qp_delta"));
                }
                magnitude += 1 << width;
                width += 1;
            }
            magnitude += bypass_bits(cabac, width)? as i32;
        }
        let delta = if magnitude != 0 && cabac.bypass()? {
            -magnitude
        } else {
            magnitude
        };
        let offset = 6 * (self.input.sps.bit_depth as i32 - 8);
        if delta < -(26 + offset / 2) || delta > 25 + offset / 2 {
            return Err(Error::InvalidData("intra_qp_delta"));
        }
        self.frame.qp.current = luma_qp(self.frame.qp.predictor, delta, offset);
        self.frame.qp.coded = true;
        self.frame.stats.qp_deltas += 1;
        self.frame.stats.nonzero_qp_deltas += u64::from(delta != 0);
        for y in (area.y..area.y + area.side()).step_by(4) {
            for x in (area.x..area.x + area.side()).step_by(4) {
                let index = self.frame.cell_index(x, y)?;
                let cell = self
                    .frame
                    .cells
                    .get_mut(index)
                    .ok_or(Error::InvalidData("intra_cell_address"))?;
                if cell.slice == self.slice {
                    cell.qp = self.frame.qp.current as i8;
                }
            }
        }
        Ok(())
    }

    fn transform_tree(&mut self, cabac: &mut Cabac<'_, '_>, cu: Cu, node: Transform) -> Result<()> {
        let Transform { area, depth, .. } = node;
        if depth > 6 || area.log2 < self.input.sps.min_tb_log2 {
            return Err(Error::InvalidData("intra_transform_tree"));
        }
        let intra = cu.chroma_mode.is_some();
        let max_depth = if intra {
            self.input.sps.max_intra_transform_depth + u32::from(cu.nxn)
        } else {
            self.input.sps.max_inter_transform_depth
        };
        let forced = area.log2 > self.input.sps.max_tb_log2
            || depth == 0 && (cu.nxn || !intra && cu.inter_split && max_depth == 0);
        let split = if !forced && area.log2 > self.input.sps.min_tb_log2 && depth < max_depth {
            decision(
                cabac,
                &mut self.frame.contexts.split_transform,
                (5 - area.log2) as usize,
            )?
        } else {
            forced
        };
        let mut chroma = node.parent_chroma;
        if area.log2 > 2 {
            for (value, parent) in chroma.iter_mut().zip(node.parent_chroma) {
                *value = (depth == 0 || parent)
                    && decision(cabac, &mut self.frame.contexts.cbf_chroma, depth as usize)?;
            }
        }
        if split {
            for (index, child) in area.children()?.into_iter().enumerate() {
                self.transform_tree(
                    cabac,
                    cu,
                    Transform {
                        area: child,
                        base: [area.x, area.y],
                        depth: depth + 1,
                        index,
                        parent_chroma: chroma,
                    },
                )?;
            }
            return Ok(());
        }
        if area.log2 > 5 {
            return Err(Error::InvalidData("intra_transform_size"));
        }
        let luma = if intra || depth != 0 || chroma.into_iter().any(|value| value) {
            decision(
                cabac,
                &mut self.frame.contexts.cbf_luma,
                usize::from(depth == 0),
            )?
        } else {
            true
        };
        if luma || chroma.into_iter().any(|value| value) {
            self.delta_qp(cabac, cu.area)?;
        }
        let mode = self.frame.cell(area.x, area.y)?.mode;
        if intra && mode.is_none() {
            return Err(Error::InvalidData("intra_luma_mode"));
        }
        self.reconstruct(
            cabac,
            ReconstructionBlock {
                area,
                component: 0,
                mode,
                coded: luma,
                bypass: cu.bypass,
            },
        )?;
        if area.log2 > 2 || node.index == 3 {
            let [x, y] = if area.log2 == 2 {
                node.base
            } else {
                [area.x, area.y]
            };
            for (component, coded) in (1..=2).zip(chroma) {
                self.reconstruct(
                    cabac,
                    ReconstructionBlock {
                        area: Area {
                            x: x / 2,
                            y: y / 2,
                            log2: area.log2.saturating_sub(1).max(2),
                        },
                        component,
                        mode: cu.chroma_mode,
                        coded,
                        bypass: cu.bypass,
                    },
                )?;
            }
        }
        Ok(())
    }

    fn references(&mut self, block: &ReconstructionBlock) -> Result<()> {
        let side = block.area.side();
        let x = block.area.x;
        let y = block.area.y;
        let scale = if block.component == 0 { 1 } else { 2 };
        let width = self.frame.info.width as usize / scale;
        for index in 0..4 * side + 1 {
            let (nx, ny) = if index < 2 * side {
                (x.checked_sub(1), Some(y + 2 * side - 1 - index))
            } else if index == 2 * side {
                (x.checked_sub(1), y.checked_sub(1))
            } else {
                (Some(x + index - 2 * side - 1), y.checked_sub(1))
            };
            let value = if let (Some(nx), Some(ny)) = (nx, ny) {
                if self
                    .neighbour(Some(nx * scale), Some(ny * scale))?
                    .is_some_and(|cell| {
                        cell.reconstructed & (1 << block.component) != 0
                            && (!self.input.pps.constrained_intra || cell.mode.is_some())
                    })
                {
                    Some(
                        *self
                            .frame
                            .planes
                            .get(block.component)
                            .and_then(|plane| plane.get(ny * width + nx))
                            .ok_or(Error::InvalidData("intra_reference_sample"))?,
                    )
                } else {
                    None
                }
            } else {
                None
            };
            *self
                .frame
                .scratch
                .references
                .get_mut(index)
                .ok_or(Error::InvalidData("intra_reference_count"))? = value;
        }
        Ok(())
    }

    fn mark(&mut self, luma_area: Area, mask: u8) -> Result<()> {
        for y in (luma_area.y..luma_area.y + luma_area.side()).step_by(4) {
            for x in (luma_area.x..luma_area.x + luma_area.side()).step_by(4) {
                let index = self.frame.cell_index(x, y)?;
                let cell = self
                    .frame
                    .cells
                    .get_mut(index)
                    .ok_or(Error::InvalidData("intra_cell_address"))?;
                if cell.reconstructed & mask != 0 || cell.slice != self.slice {
                    return Err(Error::InvalidData("intra_reconstruction_overlap"));
                }
                cell.reconstructed |= mask;
                if mask & 1 != 0 {
                    let edges = u8::from(x == luma_area.x) | (u8::from(y == luma_area.y) << 1);
                    cell.edges |= edges;
                    if mask == 1 {
                        cell.transform_edges |= edges;
                    }
                }
            }
        }
        Ok(())
    }

    fn reconstruct(&mut self, cabac: &mut Cabac<'_, '_>, block: ReconstructionBlock) -> Result<()> {
        let side = block.area.side();
        let size = BlockSize::new(side)?;
        let sample_count = side * side;
        let component = if block.component == 0 {
            Component::Luma
        } else {
            Component::Chroma
        };
        let mut transform = InverseTransform::Dct;
        if block.coded {
            let decoded = residual::decode(
                cabac,
                &mut self.frame.contexts.residual,
                residual::Block {
                    log2_size: block.area.log2 as u8,
                    component: if block.component == 0 {
                        residual::Component::Luma
                    } else {
                        residual::Component::Chroma
                    },
                    intra_mode: block.mode.map(IntraMode::number),
                    transform_skip_enabled: self.input.pps.transform_skip,
                    transquant_bypass: block.bypass,
                    sign_data_hiding_enabled: self.input.pps.sign_data_hiding,
                },
                &mut self.frame.scratch.coefficients,
            )?;
            self.frame.stats.nonzero_transform_units += 1;
            self.frame.stats.transform_skips += u64::from(decoded.transform_skip);
            self.frame.stats.sign_hidden_groups += decoded.sign_hidden_groups;
            transform = if block.bypass {
                InverseTransform::Bypass
            } else if decoded.transform_skip {
                InverseTransform::Skip
            } else if block.component == 0 && side == 4 && block.mode.is_some() {
                InverseTransform::IntraLumaDst4
            } else {
                InverseTransform::Dct
            };
            if transform == InverseTransform::IntraLumaDst4 {
                self.frame.stats.dst_blocks += 1;
            }
            if transform == InverseTransform::Dct {
                *self
                    .frame
                    .stats
                    .dct_blocks
                    .get_mut((block.area.log2 - 2) as usize)
                    .ok_or(Error::InvalidData("intra_transform_size"))? += 1;
            }
        }
        if let Some(mode) = block.mode {
            self.references(&block)?;
            kernels::predict_intra(
                IntraPrediction {
                    depth: self.frame.depth,
                    component,
                    mode,
                    strong_smoothing: self.input.sps.strong_smoothing,
                },
                &self.frame.scratch.references,
                BlockMut::new(&mut self.frame.scratch.prediction, side, size)?,
            )?;
            let mode_counts = if block.component == 0 {
                &mut self.frame.stats.luma_modes
            } else {
                &mut self.frame.stats.chroma_modes
            };
            *mode_counts
                .get_mut(usize::from(mode.number()))
                .ok_or(Error::InvalidData("intra_mode"))? += 1;
        } else {
            let scale = if block.component == 0 { 1 } else { 2 };
            let width = self.frame.info.width as usize / scale;
            let plane = self
                .frame
                .planes
                .get(block.component)
                .ok_or(Error::InvalidData("inter_plane"))?;
            for y in 0..side {
                let source = (block.area.y + y) * width + block.area.x;
                self.frame
                    .scratch
                    .prediction
                    .get_mut(y * side..(y + 1) * side)
                    .ok_or(Error::InvalidData("inter_transform_size"))?
                    .copy_from_slice(
                        plane
                            .get(source..source + side)
                            .ok_or(Error::InvalidData("inter_sample_address"))?,
                    );
            }
        }
        if block.coded {
            let lists = self.input.pps.scaling_lists.as_ref().or(self
                .input
                .sps
                .scaling_lists
                .as_ref());
            if let Some(lists) = lists {
                lists.factors(
                    block.area.log2,
                    block.component + if block.mode.is_none() { 3 } else { 0 },
                    &mut self.frame.scratch.scaling,
                )?;
                self.frame.stats.scaling_list_blocks += u64::from(!block.bypass);
            } else {
                self.frame.scratch.scaling.fill(16);
            }
            let offset = 6 * (self.input.sps.bit_depth as i32 - 8);
            let chroma_offset = if block.component == 0 {
                None
            } else {
                Some(
                    *self
                        .input
                        .syntax
                        .chroma_qp_offsets
                        .get(block.component - 1)
                        .ok_or(Error::InvalidData("intra_chroma_qp"))?,
                )
            };
            let qp = component_qp(self.frame.qp.current, offset, chroma_offset)?;
            kernels::inverse_residual(
                Reconstruction {
                    depth: self.frame.depth,
                    transform,
                    qp,
                },
                Block::new(&self.frame.scratch.coefficients, side, size)?,
                &self.frame.scratch.scaling,
                BlockMut::new(&mut self.frame.scratch.residual, side, size)?,
            )?;
        } else {
            self.frame
                .scratch
                .residual
                .get_mut(..sample_count)
                .ok_or(Error::InvalidData("intra_transform_size"))?
                .fill(0);
        }
        kernels::add_residual(
            self.frame.depth,
            Block::new(&self.frame.scratch.prediction, side, size)?,
            Block::new(&self.frame.scratch.residual, side, size)?,
            BlockMut::new(&mut self.frame.scratch.output, side, size)?,
        )?;
        let scale = if block.component == 0 { 1 } else { 2 };
        let width = self.frame.info.width as usize / scale;
        let plane = self
            .frame
            .planes
            .get_mut(block.component)
            .ok_or(Error::InvalidData("intra_plane"))?;
        for y in 0..side {
            let dest = (block.area.y + y) * width + block.area.x;
            plane
                .get_mut(dest..dest + side)
                .ok_or(Error::InvalidData("intra_sample_address"))?
                .copy_from_slice(
                    self.frame
                        .scratch
                        .output
                        .get(y * side..(y + 1) * side)
                        .ok_or(Error::InvalidData("intra_transform_size"))?,
                );
        }
        self.mark(
            Area {
                x: block.area.x * scale,
                y: block.area.y * scale,
                log2: block.area.log2 + u32::from(block.component != 0),
            },
            1 << block.component,
        )?;
        if block.component == 0 && block.coded {
            for y in (block.area.y..block.area.y + side).step_by(4) {
                for x in (block.area.x..block.area.x + side).step_by(4) {
                    let index = self.frame.cell_index(x, y)?;
                    self.frame
                        .cells
                        .get_mut(index)
                        .ok_or(Error::InvalidData("inter_cell"))?
                        .luma_residual = true;
                }
            }
        }
        self.frame.stats.transform_units += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Limits, LongTermRef, PictureInfo, Rps, SliceHeader,
        bits::Bits,
        ctu::Layout,
        params::{self, Pcm, Pps, Sps},
        parser::SliceSyntax,
    };

    enum Bin {
        Decision(bool),
        Bypass(bool),
    }

    struct Script {
        bins: std::vec::IntoIter<Bin>,
        contexts: Vec<Context>,
    }

    impl Script {
        fn new(bins: Vec<Bin>) -> Self {
            Self {
                bins: bins.into_iter(),
                contexts: Vec::new(),
            }
        }
    }

    impl Bins for Script {
        fn decision(&mut self, context: &mut Context) -> Result<bool> {
            self.contexts.push(*context);
            match self.bins.next() {
                Some(Bin::Decision(value)) => Ok(value),
                _ => Err(Error::InvalidData("test_decision")),
            }
        }

        fn bypass(&mut self) -> Result<bool> {
            match self.bins.next() {
                Some(Bin::Bypass(value)) => Ok(value),
                _ => Err(Error::InvalidData("test_bypass")),
            }
        }
    }

    #[test]
    fn complete_inter_contexts_follow_tables_9_9_through_9_23() -> Result<()> {
        for qp in -12..=63 {
            assert!(Contexts::new(qp, Initialization::I).inter.is_none());
            for (initialization, pred, merge, index, mvd) in [
                (Initialization::P, 149, 110, 122, [140, 198]),
                (Initialization::B, 134, 154, 137, [169, 198]),
            ] {
                let contexts = Contexts::new(qp, initialization);
                assert_eq!(
                    contexts.part,
                    [154, 139, 154, 154].map(|v| Context::new(v, qp))
                );
                let actual = contexts.inter.ok_or(Error::InvalidData("test_contexts"))?;
                assert_eq!(actual.skip, [197, 185, 201].map(|v| Context::new(v, qp)));
                assert_eq!(actual.pred_mode, Context::new(pred, qp));
                assert_eq!(actual.root_cbf, Context::new(79, qp));
                assert_eq!(actual.merge, Context::new(merge, qp));
                assert_eq!(actual.merge_idx, Context::new(index, qp));
                assert_eq!(
                    actual.direction,
                    [95, 79, 63, 31, 31].map(|v| Context::new(v, qp))
                );
                assert_eq!(actual.ref_idx, [Context::new(153, qp); 2]);
                assert_eq!(actual.mvp, Context::new(168, qp));
                assert_eq!(actual.mvd, mvd.map(|v| Context::new(v, qp)));
            }
        }
        Ok(())
    }

    #[test]
    fn inter_partition_strings_and_contexts_follow_tables_9_45_and_9_48() -> Result<()> {
        let mut contexts = [31, 63, 95, 127].map(|v| Context::new(v, 26));
        for minimum in 3..=6 {
            for log2 in minimum..=6 {
                for amp in [false, true] {
                    let cases: &[(usize, &str)] = if log2 > minimum && amp {
                        &[
                            (0, "1"),
                            (1, "011"),
                            (2, "001"),
                            (4, "0100"),
                            (5, "0101"),
                            (6, "0000"),
                            (7, "0001"),
                        ]
                    } else if log2 == minimum && log2 > 3 {
                        &[(0, "1"), (1, "01"), (2, "001"), (3, "000")]
                    } else {
                        &[(0, "1"), (1, "01"), (2, "00")]
                    };
                    for &(expected, string) in cases {
                        let bins = string
                            .bytes()
                            .enumerate()
                            .map(|(index, bit)| {
                                if index == 3 {
                                    Bin::Bypass(bit == b'1')
                                } else {
                                    Bin::Decision(bit == b'1')
                                }
                            })
                            .collect();
                        let mut script = Script::new(bins);
                        assert_eq!(
                            partition(&mut script, &mut contexts, log2, minimum, amp)?.index(),
                            expected
                        );
                        assert_eq!(script.bins.len(), 0);
                        let expected_contexts: Vec<_> = (0..string.len().min(3))
                            .map(|bin| {
                                contexts
                                    .get(if bin == 2 && log2 > minimum { 3 } else { bin })
                                    .copied()
                            })
                            .collect();
                        assert_eq!(
                            script
                                .contexts
                                .iter()
                                .copied()
                                .map(Some)
                                .collect::<Vec<_>>(),
                            expected_contexts
                        );
                    }
                }
            }
        }
        for (log2, minimum) in [(2, 3), (7, 3), (3, 4), (4, 2)] {
            let mut script = Script::new(Vec::new());
            assert!(partition(&mut script, &mut contexts, log2, minimum, true).is_err());
            assert!(script.contexts.is_empty());
        }
        Ok(())
    }

    #[test]
    fn merge_and_reference_indices_use_shared_context_prefixes_only() -> Result<()> {
        for maximum in [5u8, 15] {
            for count in 1..=maximum {
                for expected in 0..count {
                    let length = usize::from(expected + u8::from(expected + 1 < count));
                    let prefix = if maximum == 5 { 1 } else { 2 };
                    let mut contexts = [31, 95].map(|v| Context::new(v, 26));
                    let bins = (0..length)
                        .map(|index| {
                            let value = index < usize::from(expected);
                            if index < prefix {
                                Bin::Decision(value)
                            } else {
                                Bin::Bypass(value)
                            }
                        })
                        .collect();
                    let mut script = Script::new(bins);
                    let actual = if maximum == 5 {
                        merge_index(
                            &mut script,
                            &mut contexts
                                .first()
                                .copied()
                                .ok_or(Error::InvalidData("test_context"))?,
                            count,
                        )?
                    } else {
                        reference_index(&mut script, &mut contexts, count)?
                    };
                    assert_eq!(actual, expected);
                    assert_eq!(script.bins.len(), 0);
                    assert_eq!(
                        script.contexts,
                        contexts
                            .iter()
                            .take(length.min(prefix))
                            .copied()
                            .collect::<Vec<_>>()
                    );
                }
            }
        }
        let mut script = Script::new(Vec::new());
        assert!(merge_index(&mut script, &mut Context::new(122, 26), 0).is_err());
        assert!(merge_index(&mut script, &mut Context::new(122, 26), 6).is_err());
        assert!(reference_index(&mut script, &mut [Context::new(153, 26); 2], 0).is_err());
        assert!(reference_index(&mut script, &mut [Context::new(153, 26); 2], 16).is_err());
        assert!(script.contexts.is_empty());
        Ok(())
    }

    #[test]
    fn inter_direction_excludes_bi_for_four_by_eight_and_uses_ct_depth() -> Result<()> {
        let mut contexts = [31, 63, 95, 127, 151].map(|v| Context::new(v, 26));
        for depth in 0..=3 {
            for (width, height) in [(4, 8), (8, 4), (8, 8), (16, 32), (64, 64)] {
                for expected in 1..=3 {
                    let small = width + height == 12;
                    if small && expected == 3 {
                        continue;
                    }
                    let mut bins = Vec::new();
                    if !small {
                        bins.push(Bin::Decision(expected == 3));
                    }
                    if expected != 3 {
                        bins.push(Bin::Decision(expected == 2));
                    }
                    let mut script = Script::new(bins);
                    assert_eq!(
                        inter_direction(
                            &mut script,
                            &mut contexts,
                            depth,
                            Pu {
                                x: 0,
                                y: 0,
                                width,
                                height
                            }
                        )?,
                        expected
                    );
                    assert_eq!(script.bins.len(), 0);
                    let mut indices = Vec::new();
                    if !small {
                        indices.push(usize::from(depth));
                    }
                    if expected != 3 {
                        indices.push(4);
                    }
                    assert_eq!(
                        script.contexts,
                        indices
                            .into_iter()
                            .filter_map(|i| contexts.get(i).copied())
                            .collect::<Vec<_>>()
                    );
                }
            }
        }
        Ok(())
    }

    fn eg1(bins: &mut Vec<Bin>, mut value: u32) {
        for width in 1..=16 {
            if value >= 1 << width {
                bins.push(Bin::Bypass(true));
                value -= 1 << width;
            } else {
                bins.push(Bin::Bypass(false));
                for bit in (0..width).rev() {
                    bins.push(Bin::Bypass(value & (1 << bit) != 0));
                }
                return;
            }
        }
    }

    fn mvd_bins(values: [i32; 2]) -> Vec<Bin> {
        let mut bins = Vec::new();
        for value in values {
            bins.push(Bin::Decision(value != 0));
        }
        for value in values.into_iter().filter(|v| *v != 0) {
            bins.push(Bin::Decision(value.unsigned_abs() > 1));
        }
        for value in values.into_iter().filter(|v| *v != 0) {
            if value.unsigned_abs() > 1 {
                eg1(&mut bins, value.unsigned_abs() - 2);
            }
            bins.push(Bin::Bypass(value < 0));
        }
        bins
    }

    #[test]
    fn mvd_flags_eg1_signs_and_limits_follow_7_73_and_9_13() -> Result<()> {
        let mut contexts = [31, 95].map(|v| Context::new(v, 26));
        for x in [0, 1, -1, 2, -2, 3, -4, 31, -64, 32767, -32768] {
            for y in [0, 1, -1, 2, -2, 3, -4, 31, -64, 32767, -32768] {
                let mut script = Script::new(mvd_bins([x, y]));
                assert_eq!(
                    motion_difference(&mut script, &mut contexts)?,
                    kernels::MotionVector {
                        x: x as i16,
                        y: y as i16
                    }
                );
                assert_eq!(script.bins.len(), 0);
                let expected = [contexts.first().copied(), contexts.first().copied()]
                    .into_iter()
                    .chain(
                        [x, y]
                            .into_iter()
                            .filter(|v| *v != 0)
                            .map(|_| contexts.get(1).copied()),
                    )
                    .collect::<Vec<_>>();
                assert_eq!(
                    script.contexts.into_iter().map(Some).collect::<Vec<_>>(),
                    expected
                );
            }
        }
        for value in [32768, -32769, 65536, -65536] {
            for values in [[value, 0], [0, value]] {
                assert_eq!(
                    motion_difference(&mut Script::new(mvd_bins(values)), &mut contexts),
                    Err(Error::InvalidData("inter_mvd"))
                );
            }
        }
        assert!(motion_difference(&mut Script::new(Vec::new()), &mut contexts).is_err());
        let mut budget = 0;
        let mut cabac = Cabac::new(&[0; 4], &mut budget)?;
        assert_eq!(
            motion_difference(&mut cabac, &mut contexts),
            Err(Error::Limit("cabac_bin_budget"))
        );
        Ok(())
    }

    #[test]
    fn non_residual_initialization_follows_tables_9_5_through_9_24() {
        for qp in -12..=51 {
            for (initialization, previous, chroma, split, luma, chroma_cbf, sao) in [
                (
                    Initialization::I,
                    184,
                    63,
                    [153, 138, 138],
                    [111, 141],
                    [94, 138, 182, 154],
                    200,
                ),
                (
                    Initialization::P,
                    154,
                    152,
                    [124, 138, 94],
                    [153, 111],
                    [149, 107, 167, 154],
                    185,
                ),
                (
                    Initialization::B,
                    183,
                    152,
                    [224, 167, 122],
                    [153, 111],
                    [149, 92, 167, 154],
                    160,
                ),
            ] {
                let actual = Contexts::new(qp, initialization);
                assert_eq!(actual.previous_mode, Context::new(previous, qp));
                assert_eq!(actual.chroma_mode, Context::new(chroma, qp));
                assert_eq!(actual.split_transform, split.map(|v| Context::new(v, qp)));
                assert_eq!(actual.cbf_luma, luma.map(|v| Context::new(v, qp)));
                assert_eq!(actual.cbf_chroma, chroma_cbf.map(|v| Context::new(v, qp)));
                assert_eq!(actual.sao_type, Context::new(sao, qp));
                assert_eq!(actual.bypass, Context::new(154, qp));
                assert_eq!(actual.qp_delta, [Context::new(154, qp); 2]);
                assert_eq!(actual.sao_merge, Context::new(153, qp));
                let (split_cu, part) = if initialization == Initialization::I {
                    ([139, 141, 157], 184)
                } else {
                    ([107, 139, 126], 154)
                };
                assert_eq!(actual.split_cu, split_cu.map(|v| Context::new(v, qp)));
                assert_eq!(actual.part.first(), Some(&Context::new(part, qp)));
            }
        }
    }

    struct Fixture {
        sps: Sps,
        pps: Pps,
        layout: Layout,
        picture: PictureInfo,
    }

    struct Segment<'a> {
        rbsp: &'a [u8],
        original: &'a [u8],
        lengths: &'a [u64],
        address: u32,
        dependent: bool,
        independent: u32,
    }

    fn fixture(width: u32, height: u32, depth: u32, wpp: bool, columns: u32) -> Result<Fixture> {
        let bytes = params::tests::sequence(depth, false, false, false);
        let mut sps = params::sps(&mut Bits::new(&bytes), &Limits::default())?;
        sps.width = width;
        sps.height = height;
        sps.visible_width = width;
        sps.visible_height = height;
        sps.ctb_log2 = 4;
        sps.min_cb_log2 = 4;
        sps.min_tb_log2 = 2;
        sps.max_tb_log2 = 5;
        sps.max_intra_transform_depth = 0;
        sps.ctu_count = width.div_ceil(16) * height.div_ceil(16);
        sps.pcm = Some(Pcm {
            luma_depth: depth,
            chroma_depth: depth,
            min_log2: 4,
            max_log2: 4,
            loop_filter_disabled: false,
        });
        let mut writer = params::tests::Writer::default();
        writer.ue(0);
        writer.ue(0);
        writer.flag(true);
        writer.flag(false);
        writer.u(3, 0);
        writer.flag(false);
        writer.flag(false);
        writer.ue(0);
        writer.ue(0);
        writer.se(0);
        for _ in 0..3 {
            writer.flag(false);
        }
        writer.se(0);
        writer.se(0);
        for _ in 0..7 {
            writer.flag(false);
        }
        writer.flag(true);
        writer.flag(false);
        writer.flag(true);
        writer.flag(false);
        writer.flag(false);
        writer.ue(0);
        writer.flag(false);
        writer.flag(false);
        writer.flag(true);
        while writer.len() % 8 != 0 {
            writer.flag(false);
        }
        let mut pps = params::pps(&mut Bits::new(&writer.bytes()), &Limits::default())?;
        pps.wpp = wpp;
        pps.tiles = columns > 1;
        pps.tile_columns = columns;
        let layout = Layout::new(&sps, &pps)?;
        let picture = PictureInfo {
            width,
            height,
            visible_width: width,
            visible_height: height,
            bit_depth: depth,
            profile_idc: if depth == 8 { 1 } else { 2 },
            ctb_log2: 4,
            tiles: pps.tiles,
            wpp,
            conformance_window: [0; 4],
            vui: sps.vui,
        };
        Ok(Fixture {
            sps,
            pps,
            layout,
            picture,
        })
    }

    impl Fixture {
        fn input<'a>(&'a self, segment: Segment<'a>) -> SliceInput<'a> {
            let syntax = SliceSyntax {
                kind: 2,
                independent_address: segment.independent,
                qp_y: 26,
                chroma_qp_offsets: [0; 2],
                initialization: Initialization::I,
                sao: [false; 2],
                deblock_disabled: true,
                deblock_offsets: [0; 2],
                loop_filter_slices: true,
                inter: None,
            };
            SliceInput {
                index: 0,
                header: SliceHeader {
                    first: segment.address == 0,
                    dependent: segment.dependent,
                    pps_id: 0,
                    address: segment.address,
                    slice_type: (!segment.dependent).then_some(2),
                    poc_lsb: None,
                    output: (!segment.dependent).then_some(true),
                    slice_qp_y: (!segment.dependent).then_some(26),
                    chroma_qp_offsets: (!segment.dependent).then_some([0; 2]),
                    cabac_init_flag: (!segment.dependent).then_some(false),
                    sao: (!segment.dependent).then_some([false; 2]),
                    deblock_disabled: (!segment.dependent).then_some(true),
                    deblock_offsets: (!segment.dependent).then_some([0; 2]),
                    loop_filter_slices: (!segment.dependent).then_some(true),
                    entry_points: segment.lengths.len() as u32,
                    rbsp_header_bytes: 1,
                    no_output_of_prior_pics: false,
                    short_term_rps: Rps::default(),
                    long_term: [LongTermRef::default(); 16],
                    long_term_count: 0,
                    reference_syntax: None,
                    inter: None,
                },
                picture: self.picture,
                original: segment.original,
                rbsp: segment.rbsp,
                entry_lengths: segment.lengths,
                syntax,
                sps: &self.sps,
                pps: &self.pps,
                layout: &self.layout,
                scratch_bytes: segment.rbsp.len() + segment.lengths.len() * 8,
            }
        }
    }

    fn escaped(rbsp: &[u8]) -> Vec<u8> {
        let mut bytes = vec![38, 1];
        let mut zeros = 0;
        for &byte in rbsp {
            if zeros == 2 && byte <= 3 {
                bytes.push(3);
                zeros = 0;
            }
            bytes.push(byte);
            zeros = if byte == 0 { zeros + 1 } else { 0 };
        }
        if zeros == 2 {
            bytes.push(3);
        }
        bytes
    }

    fn pcm_body(starts: &[[u8; 2]], end: [u8; 2], values: &[u8]) -> Result<Vec<u8>> {
        if starts.len() != values.len() {
            return Err(Error::InvalidData("test_pcm_starts"));
        }
        let mut bytes = Vec::new();
        for (start, value) in starts.iter().zip(values) {
            bytes.extend(start);
            bytes.extend(core::iter::repeat_n(*value, 384));
        }
        bytes.extend(end);
        Ok(bytes)
    }

    fn segment<'a>(rbsp: &'a [u8], original: &'a [u8], lengths: &'a [u64]) -> Segment<'a> {
        Segment {
            rbsp,
            original,
            lengths,
            address: 0,
            dependent: false,
            independent: 0,
        }
    }

    fn cell_mut(frame: &mut Frame, x: usize, y: usize) -> Result<&mut Cell> {
        let index = frame.cell_index(x, y)?;
        frame
            .cells
            .get_mut(index)
            .ok_or(Error::InvalidData("test_cell"))
    }

    fn inter_input<'a>(
        fixture: &'a Fixture,
        rbsp: &'a [u8],
        original: &'a [u8],
        initialization: Initialization,
    ) -> SliceInput<'a> {
        let mut input = fixture.input(segment(rbsp, original, &[]));
        input.index = 1;
        input.syntax.kind = if initialization == Initialization::B {
            0
        } else {
            1
        };
        input.syntax.initialization = initialization;
        input.syntax.inter = Some(crate::slice::InterSlice {
            active: [1, u8::from(initialization == Initialization::B)],
            modifications: [None; 2],
            mvd_l1_zero: false,
            collocated: None,
            weights: None,
            max_merge_candidates: 1,
        });
        input.header.slice_type = Some(input.syntax.kind);
        input.header.poc_lsb = Some(1);
        input.header.inter = input.syntax.inter;
        input.header.short_term_rps = Rps {
            count: 1,
            negative_count: 1,
            ..Rps::default()
        };
        if let Some(value) = input.header.short_term_rps.delta.first_mut() {
            *value = -1;
        }
        if let Some(value) = input.header.short_term_rps.used.first_mut() {
            *value = true;
        }
        input
    }

    fn reference_dpb(input: &SliceInput<'_>) -> Result<crate::dpb::Dpb> {
        let mut dpb = crate::dpb::Dpb::new_reconstruction();
        let mut first = input.header;
        first.poc_lsb = Some(0);
        first.slice_type = Some(2);
        first.short_term_rps = Rps::default();
        first.inter = None;
        let mut nal = crate::NalHeader {
            unit_type: 19,
            layer_id: 0,
            temporal_id: 0,
        };
        dpb.picture(nal, &first, input.sps, &Limits::default(), |_| Ok(()))?;
        dpb.attach(super::super::RetainedPicture::for_test(
            0,
            input.picture,
            None,
        ))?;
        dpb.resume(|_| Ok(()))?;
        nal.unit_type = 1;
        dpb.picture(
            nal,
            &input.header,
            input.sps,
            &Limits::default(),
            |_| Ok(()),
        )?;
        Ok(dpb)
    }

    #[test]
    fn actual_p_b_skip_and_no_root_wires_finish_without_transform_bins() -> Result<()> {
        for depth in [8, 10] {
            for (initialization, skipped, body, bins) in [
                (Initialization::P, true, [0xfe, 0xe0], 2),
                (Initialization::B, true, [0xfe, 0xe0], 2),
                (Initialization::P, false, [0x5e, 0xf7], 9),
                (Initialization::B, false, [0x3c, 0x07], 11),
            ] {
                let mut fixture = fixture(8, 8, depth, false, 1)?;
                fixture.sps.min_cb_log2 = 3;
                fixture.sps.pcm = None;
                fixture.sps.conformance_window = [0; 4];
                fixture.pps.qp_delta_enabled = true;
                let rbsp = [0x80, body[0], body[1]];
                let original = escaped(&rbsp);
                let input = inter_input(&fixture, &rbsp, &original, initialization);
                let dpb = reference_dpb(&input)?;
                let mut frame =
                    Frame::new_full(&input, 1, dpb.current_references(), 0, usize::MAX)?;
                let mut budget = bins;
                let decoded = frame.decode_full(input, &dpb, &mut budget, 0, usize::MAX, 1);
                assert!(
                    decoded.is_ok(),
                    "{initialization:?} skip={skipped} depth={depth}: {decoded:?}"
                );
                assert_eq!(budget, 0);
                assert!(frame.complete());
                assert_eq!(frame.stats.transform_units, 0);
                assert!(frame.cells.iter().all(|cell| cell.transform_edges == 0));
                assert_eq!(frame.stats.qp_deltas, 0);
                assert!(frame.cells.iter().all(|cell| cell.reconstructed == 7
                    && cell.qp == 26
                    && !cell.luma_residual
                    && cell.skipped == skipped
                    && cell.motion.is_some()
                    && cell.mode.is_none()));
                for (plane, value) in frame.planes.iter().zip([42, 43, 44]) {
                    assert!(plane.iter().all(|sample| *sample == value));
                }
                assert_eq!(frame.inter_stats.skip_units, u64::from(skipped));
                assert_eq!(frame.inter_stats.prediction_units, 1);
                assert_eq!(frame.inter_stats.merge_units, u64::from(skipped));
                assert_eq!(frame.inter_stats.amvp_lists, u64::from(!skipped));
                assert_eq!(frame.inter_stats.partitions, [1, 0, 0, 0, 0, 0, 0, 0]);
            }
        }
        Ok(())
    }

    #[test]
    fn actual_p_b_intra_units_share_the_existing_reconstruction_walker() -> Result<()> {
        for depth in [8, 10] {
            for (initialization, body) in [
                (Initialization::P, [0xbb, 0x97, 0xc0]),
                (Initialization::B, [0xc0, 0x03, 0x60]),
            ] {
                let mut fixture = fixture(8, 8, depth, false, 1)?;
                fixture.sps.min_cb_log2 = 3;
                fixture.sps.pcm = None;
                fixture.sps.conformance_window = [0; 4];
                let rbsp = [0x80, body[0], body[1], body[2]];
                let original = escaped(&rbsp);
                let input = inter_input(&fixture, &rbsp, &original, initialization);
                let dpb = reference_dpb(&input)?;
                let mut frame =
                    Frame::new_full(&input, 1, dpb.current_references(), 0, usize::MAX)?;
                let mut budget = 10;
                frame.decode_full(input, &dpb, &mut budget, 0, usize::MAX, 1)?;
                assert_eq!(budget, 0);
                assert!(frame.complete());
                assert!(
                    frame
                        .planes
                        .iter()
                        .all(|plane| plane.iter().all(|sample| *sample == 1 << (depth - 1)))
                );
                assert!(
                    frame
                        .cells
                        .iter()
                        .all(|cell| cell.mode.is_some() && cell.motion.is_none())
                );
                assert_eq!(frame.stats.transform_units, 3);
                assert_eq!(frame.inter_stats.prediction_units, 0);
            }
        }
        Ok(())
    }

    #[test]
    fn actual_p_b_pcm_units_preserve_intra_classification_and_restart_budget() -> Result<()> {
        for initialization in [Initialization::P, Initialization::B] {
            let mut fixture = fixture(16, 16, 8, false, 1)?;
            fixture.sps.conformance_window = [0; 4];
            let mut rbsp = vec![0x80];
            rbsp.extend(pcm_body(&[[0xc1, 0x7c]], [0xfe, 0x80], &[37])?);
            let original = escaped(&rbsp);
            let input = inter_input(&fixture, &rbsp, &original, initialization);
            let dpb = reference_dpb(&input)?;
            let mut frame = Frame::new_full(&input, 1, dpb.current_references(), 0, usize::MAX)?;
            let mut budget = 5;
            frame.decode_full(input, &dpb, &mut budget, 0, usize::MAX, 1)?;
            assert_eq!(budget, 0);
            assert!(frame.complete());
            assert!(
                frame
                    .planes
                    .iter()
                    .all(|plane| plane.iter().all(|sample| *sample == 37))
            );
            assert!(
                frame
                    .cells
                    .iter()
                    .all(|cell| cell.mode.is_some() && cell.motion.is_none())
            );
            assert_eq!(frame.stats.pcm_units, 1);
        }
        Ok(())
    }

    #[test]
    fn inter_transform_inference_forces_partition_split_and_preserves_mc_samples() -> Result<()> {
        for depth in [8, 10] {
            let mut fixture = fixture(8, 8, depth, false, 1)?;
            fixture.sps.min_cb_log2 = 3;
            fixture.sps.max_inter_transform_depth = 0;
            let input = fixture.input(segment(&[], &[], &[]));
            let mut frame = Frame::new(&input, 0, usize::MAX)?;
            frame.contexts = Contexts::new(26, Initialization::P);
            for (plane, value) in frame.planes.iter_mut().zip([42, 43, 44]) {
                plane.fill(value);
            }
            let mut walker = Walker {
                frame: &mut frame,
                input: &input,
                location: fixture.layout.at_raster(0)?,
                slice: 1,
                dpb: None,
            };
            let area = Area {
                x: 0,
                y: 0,
                log2: 3,
            };
            walker.initialize_cells(area, 0)?;
            let mut budget = 6;
            let mut cabac = Cabac::new(&[0; 8], &mut budget)?;
            walker.transform_tree(
                &mut cabac,
                Cu {
                    area,
                    nxn: false,
                    bypass: false,
                    chroma_mode: None,
                    inter_split: true,
                },
                Transform {
                    area,
                    base: [0, 0],
                    depth: 0,
                    index: 0,
                    parent_chroma: [false; 2],
                },
            )?;
            assert_eq!(budget, 0);
            assert_eq!(walker.frame.stats.transform_units, 6);
            assert_eq!(walker.frame.stats.nonzero_transform_units, 0);
            assert!(
                walker
                    .frame
                    .cells
                    .iter()
                    .all(|cell| cell.reconstructed == 7 && !cell.luma_residual)
            );
            for (plane, value) in walker.frame.planes.iter().zip([42, 43, 44]) {
                assert!(plane.iter().all(|sample| *sample == value));
            }
        }
        Ok(())
    }

    #[test]
    fn inter_root_luma_cbf_is_inferred_and_selects_dct_not_dst() -> Result<()> {
        let fixture = fixture(8, 8, 8, false, 1)?;
        let input = fixture.input(segment(&[], &[], &[]));
        let mut frame = Frame::new(&input, 0, usize::MAX)?;
        frame.contexts = Contexts::new(26, Initialization::P);
        for plane in &mut frame.planes {
            plane.fill(42);
        }
        let mut walker = Walker {
            frame: &mut frame,
            input: &input,
            location: fixture.layout.at_raster(0)?,
            slice: 1,
            dpb: None,
        };
        let area = Area {
            x: 0,
            y: 0,
            log2: 3,
        };
        walker.begin_qp_group(area)?;
        walker.initialize_cells(area, 0)?;
        let mut budget = 6;
        let mut cabac = Cabac::new(&[0xc2, 0, 0, 0], &mut budget)?;
        walker.transform_tree(
            &mut cabac,
            Cu {
                area,
                nxn: false,
                bypass: false,
                chroma_mode: None,
                inter_split: false,
            },
            Transform {
                area,
                base: [0, 0],
                depth: 0,
                index: 0,
                parent_chroma: [false; 2],
            },
        )?;
        assert_eq!(budget, 0);
        assert_eq!(walker.frame.stats.dst_blocks, 0);
        assert_eq!(walker.frame.stats.dct_blocks, [0, 1, 0, 0]);
        assert_eq!(walker.frame.stats.nonzero_transform_units, 1);
        assert!(
            walker
                .frame
                .cells
                .iter()
                .all(|cell| cell.luma_residual && cell.reconstructed == 7)
        );
        assert!(
            walker
                .frame
                .planes
                .iter()
                .skip(1)
                .all(|plane| plane.iter().all(|sample| *sample == 42))
        );
        Ok(())
    }

    #[test]
    fn actual_bi_prediction_omits_only_list_one_mvd_when_signalled() -> Result<()> {
        let mut fixture = fixture(8, 8, 8, false, 1)?;
        fixture.sps.min_cb_log2 = 3;
        fixture.sps.pcm = None;
        fixture.sps.conformance_window = [0; 4];
        let rbsp = [0x80, 0x57, 0x63];
        let original = escaped(&rbsp);
        for zero in [true, false] {
            let mut input = inter_input(&fixture, &rbsp, &original, Initialization::B);
            if let Some(inter) = input.syntax.inter.as_mut() {
                inter.mvd_l1_zero = zero;
            }
            let dpb = reference_dpb(&input)?;
            let mut frame = Frame::new_full(&input, 1, dpb.current_references(), 0, usize::MAX)?;
            let mut budget = 11;
            let result = frame.decode_full(input, &dpb, &mut budget, 0, usize::MAX, 1);
            if !zero {
                assert!(result.is_err());
                continue;
            }
            result?;
            assert_eq!(budget, 0);
            assert!(frame.complete());
            assert_eq!(frame.inter_stats.bi_units, 1);
            assert_eq!(frame.inter_stats.amvp_lists, 2);
            assert!(frame.cells.iter().all(|cell| matches!(cell.motion,
                Some(Motion::Bi([first, second])) if first.vector == (kernels::MotionVector { x: 0, y: 0 })
                    && second.vector == first.vector)));
            for (plane, value) in frame.planes.iter().zip([42, 43, 44]) {
                assert!(plane.iter().all(|sample| *sample == value));
            }
        }
        Ok(())
    }

    #[test]
    fn intra_references_distinguish_unfinished_inter_and_constrained_intra() -> Result<()> {
        for constrained in [false, true] {
            for reconstructed in [0, 1] {
                let mut fixture = fixture(8, 8, 8, false, 1)?;
                fixture.pps.constrained_intra = constrained;
                let input = fixture.input(segment(&[], &[], &[]));
                let mut frame = Frame::new(&input, 0, usize::MAX)?;
                for plane in &mut frame.planes {
                    plane.fill(42);
                }
                let mut walker = Walker {
                    frame: &mut frame,
                    input: &input,
                    location: fixture.layout.at_raster(0)?,
                    slice: 1,
                    dpb: None,
                };
                let cell = cell_mut(walker.frame, 0, 4)?;
                cell.slice = 1;
                cell.reconstructed = reconstructed;
                assert!(walker.neighbour(Some(3), Some(4))?.is_some() == (reconstructed != 0));
                let cell = cell_mut(walker.frame, 0, 4)?;
                cell.motion = Some(Motion::L0(super::super::MotionReference {
                    reference: 0,
                    index: 0,
                    vector: kernels::MotionVector { x: 0, y: 0 },
                }));
                assert!(walker.neighbour(Some(3), Some(4))?.is_some());
                walker.references(&ReconstructionBlock {
                    area: Area {
                        x: 4,
                        y: 4,
                        log2: 2,
                    },
                    component: 0,
                    mode: Some(IntraMode::new(1)?),
                    coded: false,
                    bypass: false,
                })?;
                assert_eq!(
                    walker.frame.scratch.references.get(4),
                    Some(&if !constrained && reconstructed != 0 {
                        Some(42)
                    } else {
                        None
                    })
                );
            }
        }
        Ok(())
    }

    #[test]
    fn inter_four_sample_dct_and_chroma_only_residual_have_distinct_luma_flags() -> Result<()> {
        for component in [0, 1] {
            let mut fixture = fixture(8, 8, 8, false, 1)?;
            if component == 0 {
                fixture.sps.scaling_lists = Some(crate::scaling::ScalingLists::flat());
                let mut writer = params::tests::Writer::default();
                for size in 0..4 {
                    for matrix in (0..6).step_by(if size == 3 { 3 } else { 1 }) {
                        if size == 0 && matrix == 3 {
                            writer.flag(true);
                            writer.se(24);
                            for _ in 1..16 {
                                writer.se(0);
                            }
                        } else {
                            writer.flag(false);
                            writer.ue(0);
                        }
                    }
                }
                fixture.pps.scaling_lists = Some(crate::scaling::ScalingLists::parse(
                    &mut Bits::new(&writer.bytes()),
                )?);
            }
            let input = fixture.input(segment(&[], &[], &[]));
            let mut frame = Frame::new(&input, 0, usize::MAX)?;
            frame.contexts = Contexts::new(26, Initialization::P);
            for plane in &mut frame.planes {
                plane.fill(42);
            }
            let mut walker = Walker {
                frame: &mut frame,
                input: &input,
                location: fixture.layout.at_raster(0)?,
                slice: 1,
                dpb: None,
            };
            walker.initialize_cells(
                Area {
                    x: 0,
                    y: 0,
                    log2: 3,
                },
                0,
            )?;
            let bytes = if component == 0 {
                [0xe0, 0x80, 0, 0]
            } else {
                [0; 4]
            };
            let mut budget = 4;
            let mut cabac = Cabac::new(&bytes, &mut budget)?;
            walker.reconstruct(
                &mut cabac,
                ReconstructionBlock {
                    area: Area {
                        x: 0,
                        y: 0,
                        log2: 2,
                    },
                    component,
                    mode: None,
                    coded: true,
                    bypass: component != 0,
                },
            )?;
            assert_eq!(budget, 0);
            assert_eq!(walker.frame.stats.dst_blocks, 0);
            if component == 0 {
                assert_eq!(walker.frame.stats.dct_blocks, [1, 0, 0, 0]);
                assert!(walker.frame.cell(0, 0)?.luma_residual);
                assert!(!walker.frame.cell(4, 0)?.luma_residual);
                assert_eq!(walker.frame.stats.scaling_list_blocks, 1);
                let plane = walker
                    .frame
                    .planes
                    .first()
                    .ok_or(Error::InvalidData("test_plane"))?;
                for y in 0..8 {
                    for x in 0..8 {
                        assert_eq!(
                            plane.get(y * 8 + x),
                            Some(&if x < 4 && y < 4 { 48 } else { 42 })
                        );
                    }
                }
            } else {
                assert!(walker.frame.cells.iter().all(|cell| !cell.luma_residual));
                assert_eq!(
                    walker.frame.planes.get(1).and_then(|p| p.first()),
                    Some(&43)
                );
            }
        }
        Ok(())
    }

    #[test]
    fn luma_transform_marking_preserves_but_does_not_promote_internal_pu_edges() -> Result<()> {
        let fixture = fixture(16, 16, 8, false, 1)?;
        let input = fixture.input(segment(&[], &[], &[]));
        let mut frame = Frame::new(&input, 0, usize::MAX)?;
        let mut walker = Walker {
            frame: &mut frame,
            input: &input,
            location: fixture.layout.at_raster(0)?,
            slice: 1,
            dpb: None,
        };
        let area = Area {
            x: 0,
            y: 0,
            log2: 4,
        };
        walker.initialize_cells(area, 0)?;
        for y in (0..16).step_by(4) {
            cell_mut(walker.frame, 8, y)?.edges |= 1;
        }
        walker.mark(area, 1)?;
        assert_eq!(walker.frame.cell(8, 4)?.edges, 1);
        assert_eq!(walker.frame.cell(8, 4)?.transform_edges, 0);
        assert_eq!(walker.frame.cell(0, 4)?.transform_edges, 1);
        assert_eq!(walker.frame.cell(8, 0)?.edges, 3);
        assert_eq!(walker.frame.cell(8, 0)?.transform_edges, 2);
        assert_eq!(walker.frame.cell(0, 0)?.transform_edges, 3);
        walker.mark(area, 6)?;
        assert_eq!(walker.frame.cell(8, 4)?.transform_edges, 0);
        assert_eq!(walker.frame.cell(0, 0)?.transform_edges, 3);
        Ok(())
    }

    #[test]
    fn mpm_candidates_and_remaining_modes_follow_8_21_through_8_31() -> Result<()> {
        assert_eq!(mpm_candidates(0, 0), [0, 1, 26]);
        assert_eq!(mpm_candidates(1, 1), [0, 1, 26]);
        assert_eq!(mpm_candidates(2, 2), [2, 33, 3]);
        assert_eq!(mpm_candidates(34, 34), [34, 33, 3]);
        assert_eq!(mpm_candidates(0, 1), [0, 1, 26]);
        assert_eq!(mpm_candidates(0, 26), [0, 26, 1]);
        for left in 0..=34 {
            for above in 0..=34 {
                let candidates = mpm_candidates(left, above);
                let mut sorted = candidates;
                sorted.sort_unstable();
                assert!(sorted.windows(2).all(|pair| pair.first() < pair.get(1)));
                let expected: Vec<u8> =
                    (0..=34).filter(|mode| !candidates.contains(mode)).collect();
                assert_eq!(expected.len(), 32);
                for (remaining, expected_mode) in expected.into_iter().enumerate() {
                    assert_eq!(remaining_mode(candidates, remaining as u8), expected_mode);
                }
            }
        }
        for luma in 0..=34 {
            assert_eq!(chroma_mode(4, luma)?.number(), luma);
            for (index, base) in [0, 26, 10, 1].into_iter().enumerate() {
                assert_eq!(
                    chroma_mode(index as u32, luma)?.number(),
                    if base == luma { 34 } else { base }
                );
            }
        }
        assert!(chroma_mode(5, 0).is_err());
        Ok(())
    }

    #[test]
    fn quantization_wrap_depth_offset_and_chroma_plateaus_follow_8_283_through_8_290() -> Result<()>
    {
        for offset in [0, 12] {
            for predictor in -offset..=51 {
                for delta in -(26 + offset / 2)..=25 + offset / 2 {
                    let mut expected = predictor + delta;
                    while expected < -offset {
                        expected += 52 + offset;
                    }
                    while expected > 51 {
                        expected -= 52 + offset;
                    }
                    assert_eq!(luma_qp(predictor, delta, offset), expected);
                    assert_eq!(
                        component_qp(expected, offset, None)?,
                        (expected + offset) as u8
                    );
                }
            }
        }
        for (index, expected) in [
            (29, 29),
            (30, 29),
            (31, 30),
            (35, 33),
            (36, 34),
            (43, 37),
            (44, 38),
            (57, 51),
        ] {
            assert_eq!(component_qp(index, 0, Some(0))?, expected);
            assert_eq!(component_qp(index, 12, Some(0))?, expected + 12);
        }
        assert_eq!(component_qp(-12, 12, Some(-12))?, 0);
        assert_eq!(component_qp(51, 12, Some(12))?, 63);
        assert_eq!(component_qp(26, 0, Some(7))?, 32);
        Ok(())
    }

    #[test]
    fn actual_zero_residual_wire_reconstructs_all_components_at_both_depths() -> Result<()> {
        for depth in [8, 10] {
            let mut fixture = fixture(8, 8, depth, false, 1)?;
            fixture.sps.min_cb_log2 = 3;
            fixture.sps.pcm = None;
            let rbsp = [0x80, 0x06, 0x77];
            let original = escaped(&rbsp);
            let input = fixture.input(segment(&rbsp, &original, &[]));
            let mut frame = Frame::new(&input, 0, usize::MAX)?;
            let mut budget = 8;
            frame.decode(input, &mut budget, 0, usize::MAX)?;
            assert!(frame.complete());
            assert_eq!(budget, 0);
            assert!(
                frame
                    .planes
                    .iter()
                    .all(|plane| plane.iter().all(|value| *value == 1 << (depth - 1)))
            );
            assert_eq!(frame.stats.coding_units, 1);
            assert_eq!(frame.stats.transform_units, 3);
            assert_eq!(frame.stats.nonzero_transform_units, 0);
            assert_eq!(frame.stats.luma_modes.first(), Some(&1));
            assert_eq!(frame.stats.chroma_modes.first(), Some(&2));
        }
        Ok(())
    }

    #[test]
    fn nxn_wire_signals_all_previous_flags_before_modes_and_reconstructs_chroma_once() -> Result<()>
    {
        let mut fixture = fixture(8, 8, 8, false, 1)?;
        fixture.sps.min_cb_log2 = 3;
        fixture.sps.pcm = None;
        let rbsp = [0x80, 0x88, 0x5c, 0x4f, 0xf0];
        let original = escaped(&rbsp);
        let input = fixture.input(segment(&rbsp, &original, &[]));
        let mut frame = Frame::new(&input, 0, usize::MAX)?;
        let mut budget = 18;
        frame.decode(input, &mut budget, 0, usize::MAX)?;
        assert!(frame.complete());
        assert_eq!(budget, 0);
        assert_eq!(frame.stats.transform_units, 6);
        assert_eq!(frame.stats.luma_modes.first(), Some(&4));
        assert_eq!(frame.stats.chroma_modes.first(), Some(&2));
        assert!(
            frame
                .planes
                .iter()
                .all(|plane| plane.iter().all(|sample| *sample == 128))
        );
        Ok(())
    }

    #[test]
    fn cu64_prediction_is_reconstructed_through_forced_32_sample_transform_blocks() -> Result<()> {
        let mut fixture = fixture(64, 64, 8, false, 1)?;
        fixture.sps.ctb_log2 = 6;
        fixture.sps.min_cb_log2 = 6;
        fixture.sps.ctu_count = 1;
        fixture.sps.pcm = None;
        fixture.picture.ctb_log2 = 6;
        fixture.layout = Layout::new(&fixture.sps, &fixture.pps)?;
        let rbsp = [0x80, 0x06, 0x77, 0xf8];
        let original = escaped(&rbsp);
        let input = fixture.input(segment(&rbsp, &original, &[]));
        let mut frame = Frame::new(&input, 0, usize::MAX)?;
        let mut budget = 11;
        frame.decode(input, &mut budget, 0, usize::MAX)?;
        assert!(frame.complete());
        assert_eq!(budget, 0);
        assert_eq!(frame.stats.coding_units, 1);
        assert_eq!(frame.stats.transform_units, 12);
        assert_eq!(frame.stats.luma_modes.first(), Some(&4));
        assert_eq!(frame.stats.chroma_modes.first(), Some(&8));
        assert!(
            frame
                .planes
                .iter()
                .all(|plane| plane.iter().all(|sample| *sample == 128))
        );
        Ok(())
    }

    #[test]
    fn one_coefficient_wires_select_dst_skip_and_bypass_with_depth_offset_qp() -> Result<()> {
        let cases: [(&[u8], bool, bool, u64); 3] = [
            (&[0x88, 0x58, 0x13, 0xfe], false, false, 22),
            (&[0x88, 0x58, 0x79, 0xff], true, false, 23),
            (&[0x47, 0x88, 0x9d, 0xff, 0x80], true, true, 23),
        ];
        for depth in [8, 10] {
            for (wire, skip, bypass, expected_bins) in cases {
                let mut fixture = fixture(8, 8, depth, false, 1)?;
                fixture.sps.min_cb_log2 = 3;
                fixture.sps.pcm = None;
                fixture.pps.transform_skip = skip;
                fixture.pps.transquant_bypass = bypass;
                fixture.pps.sign_data_hiding = true;
                let mut rbsp = vec![0x80];
                rbsp.extend(wire);
                let original = escaped(&rbsp);
                let input = fixture.input(segment(&rbsp, &original, &[]));
                let mut frame = Frame::new(&input, 0, usize::MAX)?;
                let mut budget = expected_bins;
                frame.decode(input, &mut budget, 0, usize::MAX)?;
                assert!(frame.complete());
                assert_eq!(budget, 0);
                assert_eq!(frame.stats.nonzero_transform_units, 1);
                assert_eq!(frame.stats.dst_blocks, u64::from(!skip && !bypass));
                assert_eq!(frame.stats.transform_skips, u64::from(skip && !bypass));
                assert_eq!(frame.stats.transquant_bypass_units, u64::from(bypass));
                let plane = frame
                    .planes
                    .first()
                    .ok_or(Error::InvalidData("test_plane"))?;
                for (y, basis_y) in [29, 55, 74, 84i32].into_iter().enumerate() {
                    for (x, basis_x) in [29, 55, 74, 84i32].into_iter().enumerate() {
                        let shift = 20 - depth;
                        let residual = if bypass {
                            i32::from(x == 0 && y == 0)
                        } else if skip {
                            if x == 0 && y == 0 {
                                ((408 << 7) + (1 << (shift - 1))) >> shift
                            } else {
                                0
                            }
                        } else {
                            let first = (408 * basis_y + 64) >> 7;
                            (first * basis_x + (1 << (shift - 1))) >> shift
                        };
                        assert_eq!(
                            plane.get(y * 8 + x).copied(),
                            Some(((1 << (depth - 1)) + residual) as u16)
                        );
                    }
                }
            }
        }
        Ok(())
    }

    #[test]
    fn pps_scaling_list_overrides_the_sps_list_in_actual_inverse_reconstruction() -> Result<()> {
        let mut fixture = fixture(8, 8, 8, false, 1)?;
        fixture.sps.min_cb_log2 = 3;
        fixture.sps.pcm = None;
        fixture.sps.scaling_lists = Some(crate::scaling::ScalingLists::flat());
        let mut writer = params::tests::Writer::default();
        for size in 0..4 {
            for matrix in (0..6).step_by(if size == 3 { 3 } else { 1 }) {
                if size == 0 && matrix == 0 {
                    writer.flag(true);
                    writer.se(24);
                    for _ in 1..16 {
                        writer.se(0);
                    }
                } else {
                    writer.flag(false);
                    writer.ue(0);
                }
            }
        }
        fixture.pps.scaling_lists = Some(crate::scaling::ScalingLists::parse(&mut Bits::new(
            &writer.bytes(),
        ))?);
        let rbsp = [0x80, 0x88, 0x58, 0x13, 0xfe];
        let original = escaped(&rbsp);
        let input = fixture.input(segment(&rbsp, &original, &[]));
        let mut frame = Frame::new(&input, 0, usize::MAX)?;
        let mut budget = 22;
        frame.decode(input, &mut budget, 0, usize::MAX)?;
        assert!(frame.complete());
        assert_eq!(frame.stats.scaling_list_blocks, 1);
        let plane = frame
            .planes
            .first()
            .ok_or(Error::InvalidData("test_plane"))?;
        for (y, basis_y) in [29, 55, 74, 84i32].into_iter().enumerate() {
            for (x, basis_x) in [29, 55, 74, 84i32].into_iter().enumerate() {
                let first = (816 * basis_y + 64) >> 7;
                let residual = (first * basis_x + 2048) >> 12;
                assert_eq!(plane.get(y * 8 + x).copied(), Some((128 + residual) as u16));
            }
        }
        Ok(())
    }

    #[test]
    fn mpm_above_is_dc_across_a_ctb_top_even_when_the_sample_is_available() -> Result<()> {
        let fixture = fixture(16, 32, 8, false, 1)?;
        let input = fixture.input(segment(&[], &[], &[]));
        let mut frame = Frame::new(&input, 0, usize::MAX)?;
        *cell_mut(&mut frame, 0, 12)? = Cell {
            mode: Some(IntraMode::new(26)?),
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
        let mut walker = Walker {
            frame: &mut frame,
            input: &input,
            location: fixture.layout.at_raster(1)?,
            slice: 1,
            dpb: None,
        };
        assert!(walker.neighbour(Some(0), Some(15))?.is_some());
        let mut budget = 1;
        let mut cabac = Cabac::new(&[0, 0], &mut budget)?;
        assert_eq!(
            walker
                .luma_mode(
                    &mut cabac,
                    Area {
                        x: 0,
                        y: 16,
                        log2: 4
                    },
                    true
                )?
                .number(),
            0
        );
        Ok(())
    }

    #[test]
    fn qp_predictors_ignore_other_ctus_and_use_available_group_neighbours() -> Result<()> {
        let mut fixture = fixture(32, 16, 8, false, 1)?;
        fixture.pps.qp_delta_depth = 1;
        let input = fixture.input(segment(&[], &[], &[]));
        let mut frame = Frame::new(&input, 0, usize::MAX)?;
        let mut value = Cell {
            mode: Some(IntraMode::new(1)?),
            motion: None,
            skipped: false,
            luma_residual: false,
            depth: 1,
            reconstructed: 7,
            qp: 20,
            slice: 1,
            edges: 0,
            transform_edges: 0,
            protected: false,
        };
        *cell_mut(&mut frame, 4, 8)? = value;
        value.qp = 30;
        *cell_mut(&mut frame, 8, 4)? = value;
        let location = fixture.layout.at_raster(0)?;
        let mut walker = Walker {
            frame: &mut frame,
            input: &input,
            location,
            slice: 1,
            dpb: None,
        };
        walker.begin_qp_group(Area {
            x: 8,
            y: 8,
            log2: 3,
        })?;
        assert_eq!(walker.frame.qp.current, 25);
        walker.frame.qp = Qp::new(22);
        walker.location = fixture.layout.at_raster(1)?;
        value.qp = 44;
        *cell_mut(walker.frame, 12, 0)? = value;
        walker.begin_qp_group(Area {
            x: 16,
            y: 0,
            log2: 3,
        })?;
        assert_eq!(walker.frame.qp.current, 22);
        Ok(())
    }

    #[test]
    fn a_later_qp_delta_preserves_earlier_cus_and_updates_the_entire_current_cu() -> Result<()> {
        let fixture = fixture(16, 16, 8, false, 1)?;
        let input = fixture.input(segment(&[], &[], &[]));
        let mut frame = Frame::new(&input, 0, usize::MAX)?;
        frame.qp = Qp {
            current: 26,
            predictor: 26,
            origin: Some([0, 0]),
            coded: false,
        };
        let mut budget = 5;
        let mut cabac = Cabac::new(&[0x17, 0], &mut budget)?;
        let mut walker = Walker {
            frame: &mut frame,
            input: &input,
            location: fixture.layout.at_raster(0)?,
            slice: 1,
            dpb: None,
        };
        let earlier = Area {
            x: 0,
            y: 0,
            log2: 3,
        };
        let current = Area {
            x: 8,
            y: 0,
            log2: 3,
        };
        walker.initialize_cells(earlier, 1)?;
        walker.mark(earlier, 7)?;
        walker.initialize_cells(current, 1)?;
        walker.mark(
            Area {
                x: 8,
                y: 0,
                log2: 2,
            },
            7,
        )?;
        walker.delta_qp(&mut cabac, current)?;
        assert_eq!(walker.frame.qp.current, 29);
        assert_eq!(walker.frame.cell(0, 0)?.qp, 26);
        assert_eq!(walker.frame.cell(4, 4)?.qp, 26);
        assert_eq!(walker.frame.cell(8, 0)?.qp, 29);
        assert_eq!(walker.frame.cell(12, 4)?.qp, 29);
        assert_eq!(walker.frame.cell(8, 0)?.reconstructed, 7);
        assert_eq!(walker.frame.cell(0, 8)?.slice, 0);
        assert_eq!(walker.frame.stats.qp_deltas, 1);
        assert_eq!(walker.frame.stats.nonzero_qp_deltas, 1);
        walker.delta_qp(&mut cabac, current)?;
        let following = Area {
            x: 0,
            y: 8,
            log2: 3,
        };
        walker.begin_qp_group(following)?;
        walker.initialize_cells(following, 1)?;
        assert_eq!(walker.frame.cell(0, 8)?.qp, 29);
        assert_eq!(budget, 0);
        Ok(())
    }

    #[test]
    fn dependent_pcm_segments_keep_models_but_reset_arithmetic() -> Result<()> {
        let fixture = fixture(32, 16, 8, false, 1)?;
        let mut first = vec![0x80];
        first.extend(pcm_body(&[[0x86, 0x80]], [0xfe, 0x80], &[34])?);
        let first_nal = escaped(&first);
        let first_input = fixture.input(segment(&first, &first_nal, &[]));
        let mut frame = Frame::new(&first_input, 0, usize::MAX)?;
        let mut budget = 6;
        frame.decode(first_input, &mut budget, 0, usize::MAX)?;
        assert!(!frame.complete());
        assert_eq!(frame.next_ctu, 1);
        let mut second = vec![0x80];
        second.extend(pcm_body(&[[0x8c, 0x80]], [0xfe, 0x80], &[51])?);
        let second_nal = escaped(&second);
        let mut next = segment(&second, &second_nal, &[]);
        next.address = 1;
        next.dependent = true;
        frame.decode(fixture.input(next), &mut budget, 0, usize::MAX)?;
        assert!(frame.complete());
        assert_eq!(budget, 0);
        assert_eq!(frame.stats.dependent_segments, 1);
        assert_eq!(frame.stats.tile_restarts, 1);
        for (component, plane) in frame.planes.iter().enumerate() {
            let width = if component == 0 { 32 } else { 16 };
            for row in plane.chunks_exact(width) {
                assert!(
                    row.get(..width / 2)
                        .is_some_and(|part| part.iter().all(|value| *value == 34))
                );
                assert!(
                    row.get(width / 2..)
                        .is_some_and(|part| part.iter().all(|value| *value == 51))
                );
            }
        }
        Ok(())
    }

    #[test]
    fn pcm_uses_separate_component_depths_and_left_shifts_before_restart() -> Result<()> {
        let mut fixture = fixture(16, 16, 10, false, 1)?;
        fixture.sps.pcm = Some(Pcm {
            luma_depth: 8,
            chroma_depth: 6,
            min_log2: 4,
            max_log2: 4,
            loop_filter_disabled: false,
        });
        let mut writer = params::tests::Writer::default();
        for _ in 0..256 {
            writer.u(8, 33);
        }
        for _ in 0..64 {
            writer.u(6, 33);
        }
        for _ in 0..64 {
            writer.u(6, 42);
        }
        let mut rbsp = vec![0x80, 0x86, 0x80];
        rbsp.extend(writer.bytes());
        rbsp.extend([0xfe, 0x80]);
        let original = escaped(&rbsp);
        let input = fixture.input(segment(&rbsp, &original, &[]));
        let mut frame = Frame::new(&input, 0, usize::MAX)?;
        let mut budget = 3;
        frame.decode(input, &mut budget, 0, usize::MAX)?;
        assert!(frame.complete());
        assert_eq!(budget, 0);
        for (plane, expected) in frame.planes.iter().zip([132, 528, 672]) {
            assert!(plane.iter().all(|value| *value == expected));
        }
        assert_eq!(frame.stats.pcm_units, 1);
        assert_eq!(frame.stats.transform_units, 0);
        Ok(())
    }

    #[test]
    fn width_one_wpp_row_initialization_precedes_dependent_restoration() -> Result<()> {
        let fixture = fixture(16, 32, 8, true, 1)?;
        let mut rbsp = vec![0x80];
        rbsp.extend(pcm_body(&[[0x86, 0x80]], [0xfe, 0x80], &[34])?);
        let original = escaped(&rbsp);
        let input = fixture.input(segment(&rbsp, &original, &[]));
        let mut frame = Frame::new(&input, 0, usize::MAX)?;
        let mut budget = 6;
        frame.decode(input, &mut budget, 0, usize::MAX)?;
        let mut next = segment(&rbsp, &original, &[]);
        next.address = 1;
        next.dependent = true;
        frame.decode(fixture.input(next), &mut budget, 0, usize::MAX)?;
        assert!(frame.complete());
        assert!(frame.wpp.is_none());
        assert_eq!(frame.stats.wpp_restarts, 1);
        assert_eq!(budget, 0);
        Ok(())
    }

    #[test]
    fn wpp_uses_the_second_ctu_snapshot_not_the_last_ctu_or_arithmetic_state() -> Result<()> {
        let fixture = fixture(48, 32, 8, true, 1)?;
        let first = pcm_body(
            &[[0x86, 0x80], [0x8b, 0x80], [0x91, 0x80]],
            [0xfd, 0x80],
            &[34; 3],
        )?;
        let second = pcm_body(
            &[[0x92, 0x80], [0x96, 0x80], [0x9b, 0x80]],
            [0xfe, 0x80],
            &[51; 3],
        )?;
        let lengths = [first.len() as u64];
        let mut rbsp = vec![0x80];
        rbsp.extend(first);
        rbsp.extend(second);
        let original = escaped(&rbsp);
        let input = fixture.input(segment(&rbsp, &original, &lengths));
        let mut frame = Frame::new(&input, 0, usize::MAX)?;
        let mut budget = 19;
        frame.decode(input, &mut budget, 0, usize::MAX)?;
        assert!(frame.complete());
        assert_eq!(frame.stats.wpp_restarts, 1);
        assert_eq!(frame.stats.ctus, 6);
        assert_eq!(budget, 0);
        Ok(())
    }

    fn above_right_wavefront() -> Result<(Fixture, Vec<u8>, [u64; 1])> {
        let fixture = fixture(48, 32, 8, true, 1)?;
        let top = pcm_body(
            &[[0x86, 0x80], [0x8b, 0x80], [0x91, 0x80]],
            [0xfd, 0x80],
            &[34, 51, 68],
        )?;
        let lengths = [top.len() as u64];
        let mut rbsp = vec![0x80];
        rbsp.extend(top);
        rbsp.extend([0x90, 0x44, 0x2b, 0xb1, 0x80]);
        Ok((fixture, rbsp, lengths))
    }

    #[test]
    fn wavefront_above_right_prediction_waits_for_publication() -> Result<()> {
        use super::super::parallel::{TestFault, TestHook};
        let (fixture, rbsp, lengths) = above_right_wavefront()?;
        let original = escaped(&rbsp);
        let input = fixture.input(segment(&rbsp, &original, &lengths));
        let mut serial = Frame::new(&input, 0, usize::MAX)?;
        let mut budget = 10_000_000;
        serial.decode_common(input, None, &mut budget, 0, usize::MAX, 1)?;
        assert_eq!(serial.planes[0][16 * 48 + 30], 55);
        assert_eq!(serial.planes[0][16 * 48 + 31], 64);
        assert_eq!(serial.stats.pcm_units, 3);
        assert_eq!(serial.stats.coding_units, 6);
        assert_eq!(serial.cell(16, 16)?.mode, Some(IntraMode::new(34)?));
        for threads in [2, 4, 8] {
            let hook = std::sync::Arc::new(TestHook::new(TestFault::None, true));
            let mut frame = Frame::new(&input, 0, usize::MAX)?;
            frame.test_hook = Some(hook.clone());
            let mut parallel_budget = 10_000_000;
            std::thread::scope(|scope| {
                let controller =
                    scope.spawn(|| hook.release_after_wait(std::time::Duration::from_millis(20)));
                let result =
                    frame.decode_common(input, None, &mut parallel_budget, 0, usize::MAX, threads);
                assert!(controller.join().unwrap());
                result
            })?;
            assert!(frame.workers > 1);
            assert!(frame.complete());
            assert_eq!(frame.planes, serial.planes);
            assert_eq!(frame.stats, serial.stats);
            assert_eq!(frame.inter_stats, serial.inter_stats);
            assert_eq!(parallel_budget, budget);
        }
        Ok(())
    }

    #[test]
    fn wavefront_mid_row_errors_and_panics_join_every_worker() -> Result<()> {
        use super::super::parallel::{TestFault, TestHook};
        let (_, mut rbsp, prefix_lengths) = above_right_wavefront()?;
        let fixture = fixture(48, 128, 8, true, 1)?;
        let mut lengths = vec![prefix_lengths[0], 5];
        let last = rbsp.len() - 2;
        rbsp[last] = 0xb0;
        for (row, starts) in [
            [0x9c, 0xa0, 0xa5],
            [0xa6, 0xaa, 0xae],
            [0xaf, 0xb2, 0xb6],
            [0xb7, 0xb9, 0xbd],
            [0xbe, 0xc0, 0xc3],
            [0xc4, 0xc6, 0xc9],
        ]
        .into_iter()
        .enumerate()
        {
            let body = pcm_body(
                &starts.map(|byte| [byte, 0x80]),
                [if row == 5 { 0xfe } else { 0xfd }, 0x80],
                &[68; 3],
            )?;
            if row != 5 {
                lengths.push(body.len() as u64);
            }
            rbsp.extend(body);
        }
        let original = escaped(&rbsp);
        let input = fixture.input(segment(&rbsp, &original, &lengths));
        let mut control = Frame::new(&input, 0, usize::MAX)?;
        let control_result = control.decode(input, &mut 10_000_000, 0, usize::MAX);
        assert!(
            control_result.is_ok(),
            "{control_result:?} at CTU {}",
            control.next_ctu
        );
        assert!(control.complete());
        assert_eq!(control.stats.ctus, 24);
        for (fault, expected) in [
            (TestFault::Error, Error::InvalidData("injected_row_error")),
            (TestFault::Panic, Error::Internal("reconstruction_panic")),
        ] {
            let mut serial_budget = None;
            for threads in [1, 2, 4, 8] {
                let mut frame = Frame::new(&input, 0, usize::MAX)?;
                frame.test_hook = Some(std::sync::Arc::new(TestHook::new(fault, false)));
                let mut budget = 10_000_000;
                assert_eq!(
                    super::super::caught(|| frame.decode_common(
                        input,
                        None,
                        &mut budget,
                        0,
                        usize::MAX,
                        threads
                    )),
                    Err(expected)
                );
                assert_eq!(frame.workers > 1, threads > 1);
                assert_eq!(frame.workers, threads);
                assert!(!frame.complete());
                if let Some(expected_budget) = serial_budget {
                    assert_eq!(budget, expected_budget);
                } else {
                    serial_budget = Some(budget);
                }
            }
        }
        Ok(())
    }

    #[test]
    fn wavefront_deadline_expires_inside_a_parallel_dependency_wait() -> Result<()> {
        use super::super::parallel::{TestFault, TestHook};
        let (fixture, rbsp, lengths) = above_right_wavefront()?;
        let original = escaped(&rbsp);
        let input = fixture.input(segment(&rbsp, &original, &lengths));
        for threads in [2, 4, 8] {
            let hook = std::sync::Arc::new(TestHook::new(TestFault::Deadline, true));
            let mut frame = Frame::new(&input, 0, usize::MAX)?;
            frame.test_hook = Some(hook.clone());
            std::thread::scope(|scope| {
                let controller =
                    scope.spawn(|| hook.release_after_wait(std::time::Duration::from_millis(150)));
                assert_eq!(
                    frame.decode_common(input, None, &mut 10_000_000, 0, usize::MAX, threads),
                    Err(Error::Limit("decode_deadline"))
                );
                assert!(controller.join().unwrap());
            });
            assert!(frame.workers > 1);
            assert!(!frame.complete());
        }
        Ok(())
    }

    #[test]
    fn wavefront_workers_preserve_samples_work_and_failure_order() -> Result<()> {
        for width in [1, 3] {
            let fixture = fixture(width * 16, 32, 8, true, 1)?;
            let (top, bottom) = if width == 1 {
                (
                    pcm_body(&[[0x86, 0x80]], [0xfd, 0x80], &[34])?,
                    pcm_body(&[[0x86, 0x80]], [0xfe, 0x80], &[51])?,
                )
            } else {
                (
                    pcm_body(
                        &[[0x86, 0x80], [0x8b, 0x80], [0x91, 0x80]],
                        [0xfd, 0x80],
                        &[34; 3],
                    )?,
                    pcm_body(
                        &[[0x92, 0x80], [0x96, 0x80], [0x9b, 0x80]],
                        [0xfe, 0x80],
                        &[51; 3],
                    )?,
                )
            };
            let lengths = [top.len() as u64];
            for truncate in [false, true] {
                let mut rbsp = vec![0x80];
                rbsp.extend(&top);
                rbsp.extend(&bottom);
                if truncate {
                    rbsp.truncate(rbsp.len() - 7);
                }
                let original = escaped(&rbsp);
                let input = fixture.input(segment(&rbsp, &original, &lengths));
                for allowance in (0..=24).chain([10_000_000]) {
                    let mut serial = Frame::new(&input, 0, usize::MAX)?;
                    let mut budget = allowance;
                    let expected = serial.decode_common(input, None, &mut budget, 0, usize::MAX, 1);
                    for threads in [2, 4, 8] {
                        let mut frame = Frame::new(&input, 0, usize::MAX)?;
                        let mut parallel_budget = allowance;
                        let actual = frame.decode_common(
                            input,
                            None,
                            &mut parallel_budget,
                            0,
                            usize::MAX,
                            threads,
                        );
                        assert_eq!(actual.map(|_| ()), expected.map(|_| ()));
                        assert_eq!(parallel_budget, budget);
                        if allowance < 10_000_000 {
                            assert_eq!(frame.workers, 1);
                        }
                        if actual.is_ok() {
                            assert_eq!(frame.workers > 1, allowance == 10_000_000);
                            assert_eq!(frame.planes, serial.planes);
                            assert_eq!(frame.stats, serial.stats);
                            assert_eq!(frame.inter_stats, serial.inter_stats);
                            assert_eq!(frame.complete(), serial.complete());
                            assert_eq!(frame.next_ctu, serial.next_ctu);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    #[test]
    fn wavefront_memory_fallback_and_deadline_preserve_serial_admission() -> Result<()> {
        let fixture = fixture(16, 32, 8, true, 1)?;
        let top = pcm_body(&[[0x86, 0x80]], [0xfd, 0x80], &[34])?;
        let lengths = [top.len() as u64];
        let mut rbsp = vec![0x80];
        rbsp.extend(top);
        rbsp.extend(pcm_body(&[[0x86, 0x80]], [0xfe, 0x80], &[51])?);
        let original = escaped(&rbsp);
        let input = fixture.input(segment(&rbsp, &original, &lengths));
        for threads in [1, 2, 4, 8] {
            let mut frame = Frame::new(&input, 0, usize::MAX)?;
            let mut budget = 100;
            frame.decode_common(input, None, &mut budget, 0, 1 << 20, threads)?;
            assert!(frame.complete());
            assert_eq!(frame.workers, 1);
            let mut frame = Frame::new(&input, 0, usize::MAX)?;
            frame.deadline = Some(std::time::Instant::now());
            assert_eq!(
                frame.decode_common(input, None, &mut budget, 0, usize::MAX, threads),
                Err(Error::Limit("decode_deadline"))
            );
        }
        Ok(())
    }

    #[test]
    fn wavefront_eight_rows_finish_or_fail_without_stranding_workers() -> Result<()> {
        let fixture = fixture(16, 128, 8, true, 1)?;
        let mut rbsp = vec![0x80];
        let mut lengths = Vec::new();
        for row in 0..8 {
            let body = pcm_body(
                &[[0x86, 0x80]],
                [if row == 7 { 0xfe } else { 0xfd }, 0x80],
                &[34 + row],
            )?;
            if row < 7 {
                lengths.push(body.len() as u64);
            }
            rbsp.extend(body);
        }
        for mutation in 0..3 {
            let mut data = rbsp.clone();
            if mutation == 1 {
                data.truncate(data.len() - 7);
            }
            if mutation == 2 {
                data[1 + 2 * lengths[0] as usize] = 0xff;
            }
            let original = escaped(&data);
            let input = fixture.input(segment(&data, &original, &lengths));
            let mut serial = Frame::new(&input, 0, usize::MAX)?;
            let mut budget = 10_000_000;
            let expected = serial.decode_common(input, None, &mut budget, 0, usize::MAX, 1);
            for threads in [2, 4, 8] {
                let mut frame = Frame::new(&input, 0, usize::MAX)?;
                let mut parallel_budget = 10_000_000;
                let actual =
                    frame.decode_common(input, None, &mut parallel_budget, 0, usize::MAX, threads);
                assert_eq!(actual.map(|_| ()), expected.map(|_| ()));
                assert_eq!(parallel_budget, budget);
                if actual.is_ok() {
                    assert_eq!(frame.workers, threads);
                    assert_eq!(frame.planes, serial.planes);
                    assert_eq!(frame.stats, serial.stats);
                    assert!(frame.complete());
                }
            }
        }
        Ok(())
    }

    #[test]
    fn wavefront_partial_final_row_continues_as_a_dependent_segment() -> Result<()> {
        let fixture = fixture(48, 32, 8, true, 1)?;
        let top = pcm_body(
            &[[0x86, 0x80], [0x8b, 0x80], [0x91, 0x80]],
            [0xfd, 0x80],
            &[34; 3],
        )?;
        let bottom = pcm_body(&[[0x92, 0x80]], [0xfe, 0x80], &[51])?;
        let lengths = [top.len() as u64];
        let mut rbsp = vec![0x80];
        rbsp.extend(top);
        rbsp.extend(bottom);
        let original = escaped(&rbsp);
        let input = fixture.input(segment(&rbsp, &original, &lengths));
        let mut expected = None;
        for threads in [1, 2, 4, 8] {
            let mut frame = Frame::new(&input, 0, usize::MAX)?;
            let mut budget = 10_000_000;
            frame.decode_common(input, None, &mut budget, 0, usize::MAX, threads)?;
            assert_eq!(frame.next_ctu, 4);
            let mut tail = vec![0x80];
            tail.extend(pcm_body(
                &[[0x97, 0x80], [0x9b, 0x80]],
                [0xfe, 0x80],
                &[68; 2],
            )?);
            let nal = escaped(&tail);
            let mut segment = segment(&tail, &nal, &[]);
            segment.address = 4;
            segment.dependent = true;
            frame.decode_common(
                fixture.input(segment),
                None,
                &mut budget,
                0,
                usize::MAX,
                threads,
            )?;
            assert!(frame.complete());
            let actual = (frame.planes, frame.stats, budget);
            if let Some(expected) = &expected {
                assert_eq!(&actual, expected);
            } else {
                expected = Some(actual);
            }
        }
        Ok(())
    }

    #[test]
    fn wpp_slices_starting_inside_a_row_only_restore_an_available_above_right() -> Result<()> {
        for start in [1, 2] {
            let fixture = fixture(48, 32, 8, true, 1)?;
            let initial_starts: &[[u8; 2]] = if start == 1 {
                &[[0x86, 0x80]]
            } else {
                &[[0x86, 0x80], [0x8b, 0x80]]
            };
            let initial_values: &[u8] = if start == 1 { &[34] } else { &[34, 34] };
            let mut before = vec![0x80];
            before.extend(pcm_body(initial_starts, [0xfe, 0x80], initial_values)?);
            let before_nal = escaped(&before);
            let input = fixture.input(segment(&before, &before_nal, &[]));
            let mut frame = Frame::new(&input, 0, usize::MAX)?;
            let mut budget = 13;
            frame.decode(input, &mut budget, 0, usize::MAX)?;
            let top_starts: &[[u8; 2]] = if start == 1 {
                &[[0x86, 0x80], [0x8b, 0x80]]
            } else {
                &[[0x86, 0x80]]
            };
            let top_values: &[u8] = if start == 1 { &[51, 51] } else { &[51] };
            let top = pcm_body(top_starts, [0xfd, 0x80], top_values)?;
            let bottom_start = if start == 1 {
                [0x8c, 0x80]
            } else {
                [0x86, 0x80]
            };
            let bottom = pcm_body(&[bottom_start], [0xfe, 0x80], &[68])?;
            let lengths = [top.len() as u64];
            let mut rbsp = vec![0x80];
            rbsp.extend(top);
            rbsp.extend(bottom);
            let original = escaped(&rbsp);
            let mut next = segment(&rbsp, &original, &lengths);
            next.address = start;
            next.independent = start;
            frame.decode(fixture.input(next), &mut budget, 0, usize::MAX)?;
            assert_eq!(frame.next_ctu, 4);
            assert!(!frame.complete());
            assert_eq!(frame.stats.wpp_restarts, 1);
            assert_eq!(frame.stats.dependent_segments, 0);
            assert_eq!(budget, 0);
        }
        Ok(())
    }

    #[test]
    fn tile_subsets_follow_the_layout_permutation_and_reset_models() -> Result<()> {
        let fixture = fixture(32, 32, 8, false, 2)?;
        let first = pcm_body(&[[0x86, 0x80], [0x8b, 0x80]], [0xfd, 0x80], &[34, 51])?;
        let second = pcm_body(&[[0x86, 0x80], [0x8b, 0x80]], [0xfe, 0x80], &[68, 85])?;
        let lengths = [first.len() as u64];
        let mut rbsp = vec![0x80];
        rbsp.extend(first);
        rbsp.extend(second);
        let original = escaped(&rbsp);
        let input = fixture.input(segment(&rbsp, &original, &lengths));
        let mut frame = Frame::new(&input, 0, usize::MAX)?;
        let mut budget = 13;
        frame.decode(input, &mut budget, 0, usize::MAX)?;
        assert!(frame.complete());
        assert_eq!(frame.stats.tile_restarts, 2);
        assert_eq!(budget, 0);
        let y = frame
            .planes
            .first()
            .ok_or(Error::InvalidData("test_plane"))?;
        for (row, values) in y.chunks_exact(32).enumerate() {
            let [left, right] = if row < 16 { [34, 68] } else { [51, 85] };
            assert!(
                values
                    .get(..16)
                    .is_some_and(|part| part.iter().all(|value| *value == left))
            );
            assert!(
                values
                    .get(16..)
                    .is_some_and(|part| part.iter().all(|value| *value == right))
            );
        }
        Ok(())
    }

    #[test]
    fn slice_alignment_zero_words_and_range_storage_are_bounded() -> Result<()> {
        let fixture = fixture(16, 16, 8, false, 1)?;
        let mut valid = vec![0x80];
        valid.extend(pcm_body(&[[0x86, 0x80]], [0xfe, 0x80], &[34])?);
        for (suffix, accepted) in [
            (&[] as &[u8], true),
            (&[0, 0], true),
            (&[0], false),
            (&[0, 1], false),
        ] {
            let mut rbsp = valid.clone();
            rbsp.extend(suffix);
            let original = escaped(&rbsp);
            let input = fixture.input(segment(&rbsp, &original, &[]));
            let mut frame = Frame::new(&input, 0, usize::MAX)?;
            let mut budget = 3;
            assert_eq!(
                frame.decode(input, &mut budget, 0, usize::MAX).is_ok(),
                accepted
            );
        }
        let original = escaped(&valid);
        let input = fixture.input(segment(&valid, &original, &[]));
        let mut frame = Frame::new(&input, 0, usize::MAX)?;
        let mut budget = 3;
        let limit = frame.heap_bytes()? + input.scratch_bytes;
        assert_eq!(
            frame.decode(input, &mut budget, 0, limit),
            Err(Error::Limit("reconstruction_memory"))
        );
        assert_eq!(budget, 3);
        assert_eq!(frame.next_ctu, 0);
        Ok(())
    }

    #[test]
    fn missing_extra_shifted_and_unterminated_subsets_are_rejected() -> Result<()> {
        let fixture = fixture(32, 16, 8, false, 2)?;
        let first = pcm_body(&[[0x86, 0x80]], [0xfd, 0x80], &[34])?;
        let second = pcm_body(&[[0x86, 0x80]], [0xfe, 0x80], &[51])?;
        let size = first.len() as u64;
        let mut rbsp = vec![0x80];
        rbsp.extend(&first);
        rbsp.extend(&second);
        let original = escaped(&rbsp);
        let bad_lengths: [&[u64]; 4] = [&[], &[size - 1], &[size + 1], &[size, 1]];
        for lengths in bad_lengths {
            let input = fixture.input(segment(&rbsp, &original, lengths));
            let mut frame = Frame::new(&input, 0, usize::MAX)?;
            let mut budget = 20;
            assert!(frame.decode(input, &mut budget, 0, usize::MAX).is_err());
            assert!(!frame.complete());
        }
        let mut bad_subset = vec![0x80];
        bad_subset.extend(pcm_body(&[[0x86, 0x80]], [0, 0], &[34])?);
        bad_subset.extend(second);
        let original = escaped(&bad_subset);
        let lengths = [size];
        let input = fixture.input(segment(&bad_subset, &original, &lengths));
        let mut frame = Frame::new(&input, 0, usize::MAX)?;
        let mut budget = 20;
        assert_eq!(
            frame.decode(input, &mut budget, 0, usize::MAX),
            Err(Error::InvalidData("end_of_subset_one_bit"))
        );
        assert!(!frame.complete());
        let mut early_end = vec![0x80];
        early_end.extend(pcm_body(&[[0x86, 0x80]], [0xfe, 0x80], &[34])?);
        early_end.extend(first);
        let original = escaped(&early_end);
        let input = fixture.input(segment(&early_end, &original, &lengths));
        let mut frame = Frame::new(&input, 0, usize::MAX)?;
        assert_eq!(
            frame.decode(input, &mut budget, 0, usize::MAX),
            Err(Error::InvalidData("intra_unconsumed_substreams"))
        );
        assert!(!frame.complete());
        Ok(())
    }

    #[test]
    fn sao_evicts_idle_storage_at_the_source_allocation() -> Result<()> {
        struct Idle(Vec<u8>);
        impl crate::reconstruction::ReclaimMemory for Idle {
            fn retained_bytes(&mut self, mandatory: usize, limit: usize) -> Result<usize> {
                if mandatory
                    .checked_add(self.0.capacity())
                    .is_none_or(|bytes| bytes > limit)
                {
                    self.0 = Vec::new();
                }
                Ok(self.0.capacity())
            }
        }
        for component in 0..3 {
            let build = || -> Result<Frame> {
                let mut frame = filter_frame([33, 33])?;
                for ctu in &mut frame.filter_ctus {
                    ctu.sao[component] = Sao {
                        kind: 2,
                        offsets: [3, 1, -1, -3],
                        class: 0,
                        band: 0,
                    };
                }
                Ok(frame)
            };
            let mut cold = build()?;
            let mut warm = build()?;
            let live = cold.heap_bytes()? + cold.planes[component].len() * 2;
            let mut idle = Idle(vec![0; 16]);
            assert_eq!(cold.finalize(0, live)?, live);
            assert_eq!(warm.finalize_reclaiming(0, live, Some(&mut idle))?, live);
            assert_eq!(idle.0.capacity(), 0);
            assert_eq!(warm.planes, cold.planes);
            assert_eq!(warm.stats, cold.stats);
            let mut cold = build()?;
            let mut warm = build()?;
            assert_eq!(
                cold.finalize(0, live - 1),
                Err(Error::Limit("reconstruction_memory"))
            );
            assert_eq!(
                warm.finalize_reclaiming(0, live - 1, Some(&mut idle)),
                Err(Error::Limit("reconstruction_memory"))
            );
        }
        Ok(())
    }

    #[test]
    fn sao_reads_deblocked_source_once_and_charges_its_live_copy() -> Result<()> {
        let mut frame = filter_frame([33, 33])?;
        frame.planes[0][7] = 34;
        for ctu in &mut frame.filter_ctus {
            ctu.sao[0] = Sao {
                kind: 2,
                offsets: [3, 1, -1, -3],
                class: 0,
                band: 0,
            };
        }
        let live = frame.heap_bytes()? + frame.planes[0].len() * 2;
        assert_eq!(
            frame.finalize(0, live - 1),
            Err(Error::Limit("reconstruction_memory"))
        );
        assert!(!frame.filtered);
        assert_eq!(frame.planes[0][7], 34);
        assert_eq!(frame.finalize(0, live)?, live);
        assert_eq!(&frame.planes[0][6..=8], &[34, 31, 34]);
        assert_eq!(frame.stats.sao_blocks, [2, 0, 0]);
        let before = frame.planes.clone();
        let stats = frame.stats;
        assert_eq!(frame.finalize(0, live)?, 0);
        assert_eq!(frame.planes, before);
        assert_eq!(frame.stats, stats);
        Ok(())
    }

    #[test]
    fn deblocking_respects_actual_edges_and_keeps_a_protected_side() -> Result<()> {
        for protected in [false, true] {
            let mut frame = filter_frame([100, 104])?;
            for ctu in &mut frame.filter_ctus {
                ctu.deblock_disabled = false;
            }
            for y in (0..16).step_by(4) {
                for x in (0..16).step_by(4) {
                    cell_mut(&mut frame, x, y)?.protected = protected;
                }
            }
            frame.finalize(0, usize::MAX)?;
            assert_eq!(frame.stats.deblock_edges, [4, 2, 2]);
            assert_eq!(
                &frame.planes[0][13..19],
                if protected {
                    &[100, 100, 100, 103, 103, 104]
                } else {
                    &[101, 101, 102, 103, 103, 104]
                }
            );
            assert!(frame.planes[0][..13].iter().all(|v| *v == 100));
        }
        Ok(())
    }

    #[test]
    fn sao_tile_and_slice_crossings_and_protected_destinations_are_distinct() -> Result<()> {
        for tile in [false, true] {
            for across in [false, true] {
                let mut frame = filter_frame([100, 104])?;
                for ctu in &mut frame.filter_ctus {
                    ctu.sao[0] = Sao {
                        kind: 2,
                        offsets: [2, 1, -1, -2],
                        class: 0,
                        band: 0,
                    };
                    ctu.across_tiles = across;
                    ctu.across_slices = across;
                }
                if tile {
                    frame.filter_ctus[1].tile = 1;
                } else {
                    frame.filter_ctus[1].slice = 2;
                }
                frame.finalize(0, usize::MAX)?;
                assert_eq!(frame.planes[0][15], if across { 101 } else { 100 });
                assert_eq!(frame.planes[0][16], if across { 103 } else { 104 });
            }
        }
        let mut frame = filter_frame([100, 104])?;
        for ctu in &mut frame.filter_ctus {
            ctu.sao[0] = Sao {
                kind: 1,
                offsets: [2; 4],
                band: 12,
                class: 0,
            };
        }
        cell_mut(&mut frame, 0, 0)?.protected = true;
        frame.finalize(0, usize::MAX)?;
        assert_eq!(frame.planes[0][0], 100);
        assert_eq!(frame.planes[0][4], 102);
        assert_eq!(frame.planes[0][16], 106);
        Ok(())
    }

    fn filter_frame(values: [u8; 2]) -> Result<Frame> {
        let fixture = fixture(32, 16, 8, false, 1)?;
        let mut rbsp = vec![0x80];
        rbsp.extend(pcm_body(
            &[[0x86, 0x80], [0x8b, 0x80]],
            [0xfe, 0x80],
            &values,
        )?);
        let original = escaped(&rbsp);
        let input = fixture.input(segment(&rbsp, &original, &[]));
        let mut frame = Frame::new(&input, 0, usize::MAX)?;
        let mut budget = 6;
        frame.decode(input, &mut budget, 0, usize::MAX)?;
        assert!(frame.complete());
        Ok(frame)
    }

    #[test]
    fn duplicate_ctus_bad_alignment_and_truncated_pcm_never_complete() -> Result<()> {
        let fixture = fixture(16, 16, 8, false, 1)?;
        let body = pcm_body(&[[0x86, 0x80]], [0xfe, 0x80], &[34])?;
        let mut rbsp = vec![0x80];
        rbsp.extend(&body);
        let original = escaped(&rbsp);
        let input = fixture.input(segment(&rbsp, &original, &[]));
        let mut frame = Frame::new(&input, 0, usize::MAX)?;
        let mut budget = 6;
        frame.decode(input, &mut budget, 0, usize::MAX)?;
        assert!(
            frame
                .decode(
                    fixture.input(segment(&rbsp, &original, &[])),
                    &mut budget,
                    0,
                    usize::MAX
                )
                .is_err()
        );
        let mut bad = rbsp.clone();
        *bad.get_mut(2).ok_or(Error::InvalidData("test_alignment"))? |= 1;
        let bad_nal = escaped(&bad);
        let input = fixture.input(segment(&bad, &bad_nal, &[]));
        let mut frame = Frame::new(&input, 0, usize::MAX)?;
        assert_eq!(
            frame.decode(input, &mut budget, 0, usize::MAX),
            Err(Error::InvalidData("intra_alignment_zero"))
        );
        assert!(!frame.complete());
        let short = rbsp
            .get(..20)
            .ok_or(Error::InvalidData("test_truncation"))?;
        let short_nal = escaped(short);
        let input = fixture.input(segment(short, &short_nal, &[]));
        let mut frame = Frame::new(&input, 0, usize::MAX)?;
        let mut budget = 3;
        assert!(frame.decode(input, &mut budget, 0, usize::MAX).is_err());
        assert!(!frame.complete());
        Ok(())
    }
}
