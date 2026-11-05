use super::{
    Cell, Frame, Memory,
    filter::{Ctu, Sao},
};
use crate::{
    Error, Result,
    cabac::{Cabac, Context},
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
    part: Context,
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

impl Contexts {
    pub(super) fn new(slice_qp: i32) -> Self {
        Self {
            split_cu: [139, 141, 157].map(|value| Context::new(value, slice_qp)),
            bypass: Context::new(154, slice_qp),
            part: Context::new(184, slice_qp),
            previous_mode: Context::new(184, slice_qp),
            chroma_mode: Context::new(63, slice_qp),
            split_transform: [153, 138, 138].map(|value| Context::new(value, slice_qp)),
            cbf_luma: [111, 141].map(|value| Context::new(value, slice_qp)),
            cbf_chroma: [94, 138, 182, 154].map(|value| Context::new(value, slice_qp)),
            qp_delta: [Context::new(154, slice_qp); 2],
            residual: residual::Contexts::new(slice_qp),
            sao_merge: Context::new(153, slice_qp),
            sao_type: Context::new(200, slice_qp),
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

pub(super) struct Qp {
    current: i32,
    predictor: i32,
    origin: Option<[usize; 2]>,
    side: usize,
    coded: bool,
}

impl Qp {
    pub(super) fn new(slice_qp: i32) -> Self {
        Self {
            current: slice_qp,
            predictor: slice_qp,
            origin: None,
            side: 0,
            coded: false,
        }
    }
}

#[derive(Clone, Copy)]
struct Area {
    x: usize,
    y: usize,
    log2: u32,
}

impl Area {
    fn side(self) -> usize {
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
    nxn: bool,
    bypass: bool,
    chroma_mode: IntraMode,
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
    mode: IntraMode,
    coded: bool,
    bypass: bool,
}

fn decision(cabac: &mut Cabac<'_, '_>, contexts: &mut [Context], index: usize) -> Result<bool> {
    cabac.decision(
        contexts
            .get_mut(index)
            .ok_or(Error::InvalidData("intra_context"))?,
    )
}

fn bypass_bits(cabac: &mut Cabac<'_, '_>, width: u32) -> Result<u32> {
    if width > 32 {
        return Err(Error::InvalidData("intra_bin_width"));
    }
    let mut value = 0;
    for _ in 0..width {
        value = (value << 1) | u32::from(cabac.bypass()?);
    }
    Ok(value)
}

fn alignment(cabac: &mut Cabac<'_, '_>) -> Result<()> {
    while cabac.position() % 8 != 0 {
        if cabac.raw_bits(1)? != 0 {
            return Err(Error::InvalidData("intra_alignment_zero"));
        }
    }
    Ok(())
}

fn end_range(cabac: &mut Cabac<'_, '_>, bytes: usize, final_slice: bool) -> Result<()> {
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
        match index {
            30..=43 => *[29, 30, 31, 32, 33, 33, 34, 34, 35, 35, 36, 36, 37, 37]
                .get((index - 30) as usize)
                .ok_or(Error::InvalidData("intra_chroma_qp"))?,
            44.. => index - 6,
            _ => index,
        }
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

    pub(super) fn decode(
        &mut self,
        input: SliceInput<'_>,
        remaining: &mut u64,
        fixed: usize,
        limit: usize,
    ) -> Result<usize> {
        Self::admit(&input)?;
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
        if !input.header.dependent {
            self.contexts = Contexts::new(input.syntax.qp_y);
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
                let location = input.layout.at_tile_scan(self.next_ctu)?;
                self.start_ctu(&input, location, slice)?;
                self.read_sao(&input, location, slice, &mut cabac)?;
                let mut walker = Walker {
                    frame: self,
                    input: &input,
                    location,
                    slice,
                };
                walker.coding_tree(
                    &mut cabac,
                    Area {
                        x: (location.x << input.sps.ctb_log2) as usize,
                        y: (location.y << input.sps.ctb_log2) as usize,
                        log2: input.sps.ctb_log2,
                    },
                    0,
                )?;
                self.stats.ctus += 1;
                self.save_wpp(&input, location, slice);
                self.next_ctu += 1;
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

    fn cell_index(&self, x: usize, y: usize) -> Result<usize> {
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
            self.contexts = Contexts::new(input.syntax.qp_y);
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
                Contexts::new(input.syntax.qp_y)
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

struct Walker<'f, 'i, 'data> {
    frame: &'f mut Frame,
    input: &'i SliceInput<'data>,
    location: Location,
    slice: u32,
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

    fn neighbour(&self, x: Option<usize>, y: Option<usize>) -> Result<Option<Cell>> {
        let (Some(x), Some(y)) = (x, y) else {
            return Ok(None);
        };
        if x >= self.frame.info.width as usize || y >= self.frame.info.height as usize {
            return Ok(None);
        }
        let cell = *self.frame.cell(x, y)?;
        if cell.slice != self.slice || cell.mode.is_none() && cell.reconstructed == 0 {
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
            side,
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
                    depth,
                    reconstructed: 0,
                    qp: self.frame.qp.current as i8,
                    slice: self.slice,
                    edges: u8::from(x == area.x) | (u8::from(y == area.y) << 1),
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
        let nxn = area.log2 == self.input.sps.min_cb_log2
            && !cabac.decision(&mut self.frame.contexts.part)?;
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
            nxn,
            bypass,
            chroma_mode: chroma_mode(chroma_index, luma)?,
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

    fn pcm(&mut self, cabac: &mut Cabac<'_, '_>, area: Area) -> Result<()> {
        alignment(cabac)?;
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

    fn delta_qp(&mut self, cabac: &mut Cabac<'_, '_>) -> Result<()> {
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
        let [x0, y0] = self
            .frame
            .qp
            .origin
            .ok_or(Error::InvalidData("intra_qp_group"))?;
        let side = self.frame.qp.side;
        for y in (y0..(y0 + side).min(self.frame.info.height as usize)).step_by(4) {
            for x in (x0..(x0 + side).min(self.frame.info.width as usize)).step_by(4) {
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
        let forced = area.log2 > self.input.sps.max_tb_log2 || cu.nxn && depth == 0;
        let split = if !forced
            && area.log2 > self.input.sps.min_tb_log2
            && depth < self.input.sps.max_intra_transform_depth + u32::from(cu.nxn)
        {
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
        let luma = decision(
            cabac,
            &mut self.frame.contexts.cbf_luma,
            usize::from(depth == 0),
        )?;
        if luma || chroma.into_iter().any(|value| value) {
            self.delta_qp(cabac)?;
        }
        let mode = self
            .frame
            .cell(area.x, area.y)?
            .mode
            .ok_or(Error::InvalidData("intra_luma_mode"))?;
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
                    .is_some_and(|cell| cell.reconstructed & (1 << block.component) != 0)
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
                    cell.edges |= u8::from(x == luma_area.x) | (u8::from(y == luma_area.y) << 1);
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
                    intra_mode: block.mode.number(),
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
            } else if block.component == 0 && side == 4 {
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
        self.references(&block)?;
        kernels::predict_intra(
            IntraPrediction {
                depth: self.frame.depth,
                component,
                mode: block.mode,
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
            .get_mut(usize::from(block.mode.number()))
            .ok_or(Error::InvalidData("intra_mode"))? += 1;
        if block.coded {
            let lists = self.input.pps.scaling_lists.as_ref().or(self
                .input
                .sps
                .scaling_lists
                .as_ref());
            if let Some(lists) = lists {
                lists.factors(
                    block.area.log2,
                    block.component,
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
                cabac_init: false,
                sao: [false; 2],
                deblock_disabled: true,
                deblock_offsets: [0; 2],
                loop_filter_slices: true,
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
            depth: 0,
            reconstructed: 7,
            qp: 26,
            slice: 1,
            edges: 0,
            protected: false,
        };
        let mut walker = Walker {
            frame: &mut frame,
            input: &input,
            location: fixture.layout.at_raster(1)?,
            slice: 1,
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
            depth: 1,
            reconstructed: 7,
            qp: 20,
            slice: 1,
            edges: 0,
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
    fn a_later_qp_delta_updates_preceding_zero_cus_in_the_same_group() -> Result<()> {
        let fixture = fixture(16, 16, 8, false, 1)?;
        let input = fixture.input(segment(&[], &[], &[]));
        let mut frame = Frame::new(&input, 0, usize::MAX)?;
        frame.qp = Qp {
            current: 26,
            predictor: 26,
            origin: Some([0, 0]),
            side: 16,
            coded: false,
        };
        for (x, mask) in [(0, 7), (8, 0)] {
            *cell_mut(&mut frame, x, 0)? = Cell {
                mode: Some(IntraMode::new(1)?),
                depth: 1,
                reconstructed: mask,
                qp: 26,
                slice: 1,
                edges: 0,
                protected: false,
            };
        }
        let mut budget = 5;
        let mut cabac = Cabac::new(&[0x17, 0], &mut budget)?;
        let mut walker = Walker {
            frame: &mut frame,
            input: &input,
            location: fixture.layout.at_raster(0)?,
            slice: 1,
        };
        walker.delta_qp(&mut cabac)?;
        assert_eq!(walker.frame.qp.current, 29);
        assert_eq!(walker.frame.cell(0, 0)?.qp, 29);
        assert_eq!(walker.frame.cell(8, 0)?.qp, 29);
        assert_eq!(walker.frame.cell(0, 8)?.slice, 0);
        assert_eq!(walker.frame.stats.qp_deltas, 1);
        assert_eq!(walker.frame.stats.nonzero_qp_deltas, 1);
        walker.delta_qp(&mut cabac)?;
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
