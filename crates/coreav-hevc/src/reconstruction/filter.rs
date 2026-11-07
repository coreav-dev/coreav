use super::{Cell, Frame, Memory, Motion, MotionReference};
use crate::{Error, Result, dpb::References};
use coreav_kernels_hevc::{self as kernels, BoundaryStrength, SaoBand, SaoEdge};

fn transform_boundary(
    cell: Cell,
    position: [usize; 2],
    ctb_log2: u32,
    horizontal: bool,
) -> Result<bool> {
    let size = 1usize
        .checked_shl(
            ctb_log2
                .checked_sub(u32::from(cell.depth))
                .ok_or(Error::InvalidData("filter_cu_depth"))?,
        )
        .ok_or(Error::InvalidData("filter_cu_depth"))?;
    let coordinate = if horizontal { position[1] } else { position[0] };
    Ok(cell.transform_edges & (1 << usize::from(horizontal)) != 0 || coordinate % size == 0)
}

fn nearby(first: MotionReference, second: MotionReference) -> bool {
    (i32::from(first.vector.x) - i32::from(second.vector.x)).abs() < 4
        && (i32::from(first.vector.y) - i32::from(second.vector.y)).abs() < 4
}

fn strength(
    p: Cell,
    q: Cell,
    references: Option<&References>,
    transform: bool,
) -> Result<BoundaryStrength> {
    let (Some(pmotion), Some(qmotion)) = (p.motion, q.motion) else {
        return Ok(BoundaryStrength::Two);
    };
    if transform && (p.luma_residual || q.luma_residual) {
        return Ok(BoundaryStrength::One);
    }
    let references = references.ok_or(Error::InvalidData("filter_reference_catalog"))?;
    let same = |first: MotionReference, second: MotionReference| -> Result<bool> {
        let first = references
            .entries
            .get(usize::from(first.reference))
            .copied()
            .flatten()
            .ok_or(Error::InvalidData("filter_reference_catalog"))?;
        let second = references
            .entries
            .get(usize::from(second.reference))
            .copied()
            .flatten()
            .ok_or(Error::InvalidData("filter_reference_catalog"))?;
        Ok(first.index == second.index)
    };
    let equivalent = match (pmotion, qmotion) {
        (Motion::L0(p) | Motion::L1(p), Motion::L0(q) | Motion::L1(q)) => {
            same(p, q)? && nearby(p, q)
        }
        (Motion::Bi([p0, p1]), Motion::Bi([q0, q1])) => {
            same(p0, q0)? && same(p1, q1)? && nearby(p0, q0) && nearby(p1, q1)
                || same(p0, q1)? && same(p1, q0)? && nearby(p0, q1) && nearby(p1, q0)
        }
        _ => false,
    };
    Ok(if equivalent {
        BoundaryStrength::Zero
    } else {
        BoundaryStrength::One
    })
}

#[derive(Clone, Copy, Default)]
pub(super) struct Sao {
    pub kind: u8,
    pub offsets: [i16; 4],
    pub class: u8,
    pub band: u8,
}

#[derive(Clone, Copy, Default)]
pub(super) struct Ctu {
    pub seen: bool,
    pub order: u32,
    pub tile: u32,
    pub slice: u32,
    pub across_tiles: bool,
    pub across_slices: bool,
    pub deblock_disabled: bool,
    pub offsets: [i32; 2],
    pub chroma_offsets: [i32; 2],
    pub sao: [Sao; 3],
}

impl Frame {
    pub(crate) fn finalize(&mut self, fixed: usize, limit: usize) -> Result<usize> {
        if !self.complete() || self.filter_ctus.iter().any(|c| !c.seen) {
            return Err(Error::InvalidData("selected_picture_incomplete"));
        }
        if self.filtered {
            return Ok(0);
        }
        for horizontal in [false, true] {
            for component in 0..2 {
                self.deblock(component, horizontal)?;
            }
        }
        let mut peak = 0;
        for component in 0..3 {
            if !self
                .filter_ctus
                .iter()
                .any(|ctu| ctu.sao.get(component).is_some_and(|s| s.kind != 0))
            {
                continue;
            }
            let source = if self
                .filter_ctus
                .iter()
                .any(|ctu| ctu.sao.get(component).is_some_and(|s| s.kind > 1))
            {
                let mut memory = Memory::new(fixed, limit)?;
                memory.charge(self.heap_bytes()?)?;
                let plane = self
                    .planes
                    .get(component)
                    .ok_or(Error::InvalidData("filter_component"))?;
                let mut source = memory.allocate(plane.len(), 0u16)?;
                source.copy_from_slice(plane);
                peak = peak.max(memory.used);
                Some(source)
            } else {
                None
            };
            self.sao_component(component, source.as_deref())?;
        }
        self.filtered = true;
        Ok(peak)
    }

    fn filter_ctu(&self, x: usize, y: usize) -> Result<Ctu> {
        let columns = self.info.width.div_ceil(1 << self.info.ctb_log2) as usize;
        self.filter_ctus
            .get((y >> self.info.ctb_log2) * columns + (x >> self.info.ctb_log2))
            .copied()
            .filter(|c| c.seen)
            .ok_or(Error::InvalidData("filter_ctu_address"))
    }

    fn across(&self, a: [usize; 2], b: [usize; 2]) -> Result<bool> {
        let a = self.filter_ctu(a[0], a[1])?;
        let b = self.filter_ctu(b[0], b[1])?;
        if a.tile != b.tile && !a.across_tiles {
            return Ok(false);
        }
        Ok(a.slice == b.slice
            || if a.order > b.order {
                a.across_slices
            } else {
                b.across_slices
            })
    }

    fn deblock(&mut self, component: usize, horizontal: bool) -> Result<()> {
        let scale = if component == 0 { 1 } else { 2 };
        let width = self.info.width as usize / scale;
        let height = self.info.height as usize / scale;
        let edge_extent = if horizontal { height } else { width };
        let along_extent = if horizontal { width } else { height };
        for edge in (8..edge_extent).step_by(8) {
            self.check_deadline()?;
            for along in (0..along_extent).step_by(4) {
                let [x, y] = if horizontal {
                    [along, edge]
                } else {
                    [edge, along]
                };
                let [lx, ly] = [x * scale, y * scale];
                let [px, py] = if horizontal {
                    [lx, ly - 1]
                } else {
                    [lx - 1, ly]
                };
                let qcell = *self.cell(lx, ly)?;
                let pcell = *self.cell(px, py)?;
                let ctu = self.filter_ctu(lx, ly)?;
                if qcell.edges & (1 << usize::from(horizontal)) == 0
                    || ctu.deblock_disabled
                    || !self.across([lx, ly], [px, py])?
                {
                    continue;
                }
                let strength = strength(
                    pcell,
                    qcell,
                    self.reference.as_ref().map(|state| &state.references),
                    transform_boundary(qcell, [lx, ly], self.info.ctb_log2, horizontal)?,
                )?;
                if component == 0 {
                    *self
                        .inter_stats
                        .boundary_strengths
                        .get_mut(usize::from(strength.value()))
                        .ok_or(Error::InvalidData("filter_boundary_strength"))? += 1;
                }
                if strength == BoundaryStrength::Zero
                    || component != 0 && strength != BoundaryStrength::Two
                {
                    continue;
                }
                let qp = (i32::from(qcell.qp) + i32::from(pcell.qp) + 1) >> 1;
                let protected = [pcell.protected, qcell.protected];
                for component in component..if component == 0 { 1 } else { 3 } {
                    if component == 0 {
                        let mut samples = [[0; 8]; 4];
                        for (row, line) in samples.iter_mut().enumerate() {
                            for (index, value) in line.iter_mut().enumerate() {
                                let address = if horizontal {
                                    (edge + index - 4) * width + along + row
                                } else {
                                    (along + row) * width + edge + index - 4
                                };
                                *value = *self
                                    .planes
                                    .get(component)
                                    .and_then(|p| p.get(address))
                                    .ok_or(Error::InvalidData("deblock_sample"))?;
                            }
                        }
                        let result = kernels::deblock_luma_with_strength(
                            self.depth,
                            samples,
                            qp,
                            ctu.offsets,
                            strength,
                            protected,
                        )?;
                        self.write_edge(component, horizontal, edge, along, strength, result)?;
                    } else {
                        let mut samples = [[0; 4]; 4];
                        for (row, line) in samples.iter_mut().enumerate() {
                            for (index, value) in line.iter_mut().enumerate() {
                                let address = if horizontal {
                                    (edge + index - 2) * width + along + row
                                } else {
                                    (along + row) * width + edge + index - 2
                                };
                                *value = *self
                                    .planes
                                    .get(component)
                                    .and_then(|p| p.get(address))
                                    .ok_or(Error::InvalidData("deblock_sample"))?;
                            }
                        }
                        let offset = *ctu
                            .chroma_offsets
                            .get(component - 1)
                            .ok_or(Error::InvalidData("deblock_component"))?;
                        let result = kernels::deblock_chroma(
                            self.depth,
                            samples,
                            qp + offset,
                            ctu.offsets[1],
                            protected,
                        )?;
                        self.write_edge(component, horizontal, edge, along, strength, result)?;
                    }
                    *self
                        .stats
                        .deblock_edges
                        .get_mut(component)
                        .ok_or(Error::InvalidData("filter_component"))? += 1;
                }
            }
        }
        Ok(())
    }

    fn write_edge<const N: usize>(
        &mut self,
        component: usize,
        horizontal: bool,
        edge: usize,
        along: usize,
        strength: BoundaryStrength,
        result: [[u16; N]; 4],
    ) -> Result<()> {
        let width = self.info.width as usize / if component == 0 { 1 } else { 2 };
        let plane = self
            .planes
            .get_mut(component)
            .ok_or(Error::InvalidData("filter_component"))?;
        let mut changed = 0;
        for (row, line) in result.into_iter().enumerate() {
            for (index, value) in line.into_iter().enumerate() {
                let address = if horizontal {
                    (edge + index - N / 2) * width + along + row
                } else {
                    (along + row) * width + edge + index - N / 2
                };
                let target = plane
                    .get_mut(address)
                    .ok_or(Error::InvalidData("deblock_sample"))?;
                changed += u64::from(*target != value);
                *target = value;
            }
        }
        *self
            .stats
            .deblock_changed_samples
            .get_mut(component)
            .ok_or(Error::InvalidData("filter_component"))? += changed;
        *self
            .inter_stats
            .boundary_changed_samples
            .get_mut(usize::from(strength.value()))
            .and_then(|row| row.get_mut(component))
            .ok_or(Error::InvalidData("filter_component"))? += changed;
        Ok(())
    }

    fn sao_component(&mut self, component: usize, source: Option<&[u16]>) -> Result<()> {
        let mut output = core::mem::take(
            self.planes
                .get_mut(component)
                .ok_or(Error::InvalidData("sao_component"))?,
        );
        let result = self.sao_into(component, source, &mut output);
        *self
            .planes
            .get_mut(component)
            .ok_or(Error::InvalidData("sao_component"))? = output;
        result
    }

    fn sao_into(
        &mut self,
        component: usize,
        source: Option<&[u16]>,
        output: &mut [u16],
    ) -> Result<()> {
        let scale = if component == 0 { 1 } else { 2 };
        let width = self.info.width as usize / scale;
        let height = self.info.height as usize / scale;
        let side = (1usize << self.info.ctb_log2) / scale;
        for cy in (0..height).step_by(side) {
            self.check_deadline()?;
            for cx in (0..width).step_by(side) {
                let sao = *self
                    .filter_ctu(cx * scale, cy * scale)?
                    .sao
                    .get(component)
                    .ok_or(Error::InvalidData("sao_component"))?;
                if sao.kind == 0 {
                    continue;
                }
                *self
                    .stats
                    .sao_blocks
                    .get_mut(component)
                    .ok_or(Error::InvalidData("sao_component"))? += 1;
                let ex = (cx + side).min(width);
                let ey = (cy + side).min(height);
                let band = if sao.kind == 1 {
                    Some(SaoBand::new(self.depth, sao.offsets, sao.band)?)
                } else {
                    None
                };
                let edge = if band.is_none() {
                    Some(SaoEdge::new(self.depth, sao.offsets)?)
                } else {
                    None
                };
                let [dx, dy] = if band.is_some() {
                    [0, 0]
                } else {
                    match sao.class {
                        0 => [1, 0],
                        1 => [0, 1],
                        2 => [1, 1],
                        3 => [-1, 1],
                        _ => return Err(Error::InvalidData("sao_class")),
                    }
                };
                let xs = if band.is_some() {
                    [cx, cx, ex, ex]
                } else {
                    [cx, cx + 1, ex - 1, ex]
                };
                let ys = if band.is_some() {
                    [cy, cy, ey, ey]
                } else {
                    [cy, cy + 1, ey - 1, ey]
                };
                for yr in ys.windows(2) {
                    for xr in xs.windows(2) {
                        let (&[x0, x1], &[y0, y1]) = (xr, yr) else {
                            return Err(Error::InvalidData("sao_rectangle"));
                        };
                        if x0 >= x1 || y0 >= y1 {
                            continue;
                        }
                        let mut available = true;
                        for sign in [-1, 1] {
                            let (Some(nx), Some(ny)) = (
                                x0.checked_add_signed(sign * dx),
                                y0.checked_add_signed(sign * dy),
                            ) else {
                                available = false;
                                break;
                            };
                            if nx >= width
                                || ny >= height
                                || !self
                                    .across([x0 * scale, y0 * scale], [nx * scale, ny * scale])?
                            {
                                available = false;
                                break;
                            }
                        }
                        if !available {
                            continue;
                        }
                        for y in y0..y1 {
                            let mut x = x0;
                            while x < x1 {
                                let start = x;
                                let protected = self.cell(x * scale, y * scale)?.protected;
                                x = ((x / (4 / scale) + 1) * (4 / scale)).min(x1);
                                while x < x1
                                    && self.cell(x * scale, y * scale)?.protected == protected
                                {
                                    x = (x + 4 / scale).min(x1);
                                }
                                if protected {
                                    continue;
                                }
                                let destination = output
                                    .get_mut(y * width + start..y * width + x)
                                    .ok_or(Error::InvalidData("sao_sample"))?;
                                let changed = if let Some(band) = band {
                                    band.apply(destination)?
                                } else {
                                    let source = source.ok_or(Error::InvalidData("sao_source"))?;
                                    let samples = source
                                        .get(y * width + start..y * width + x)
                                        .ok_or(Error::InvalidData("sao_sample"))?;
                                    let neighbour = |sign| -> Result<&[u16]> {
                                        let nx = start
                                            .checked_add_signed(sign * dx)
                                            .ok_or(Error::InvalidData("sao_neighbour"))?;
                                        let ny = y
                                            .checked_add_signed(sign * dy)
                                            .ok_or(Error::InvalidData("sao_neighbour"))?;
                                        source
                                            .get(ny * width + nx..ny * width + nx + x - start)
                                            .ok_or(Error::InvalidData("sao_neighbour"))
                                    };
                                    edge.ok_or(Error::InvalidData("sao_class"))?.apply(
                                        samples,
                                        [neighbour(-1)?, neighbour(1)?],
                                        destination,
                                    )?
                                };
                                *self
                                    .stats
                                    .sao_changed_samples
                                    .get_mut(component)
                                    .ok_or(Error::InvalidData("sao_component"))? += changed;
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::{ReferenceState, RetainedPicture, Scratch, syntax};
    use super::*;
    use crate::{
        InterStats, IntraStats, PictureInfo, cabac::Initialization, dpb::ReferenceIdentity,
    };
    use coreav_kernels_hevc::MotionVector;

    fn reference(catalog: u8, index: u8, x: i16, y: i16) -> MotionReference {
        MotionReference {
            reference: catalog,
            index,
            vector: MotionVector { x, y },
        }
    }

    fn cell(motion: Option<Motion>) -> Cell {
        Cell {
            mode: None,
            motion,
            skipped: false,
            luma_residual: false,
            depth: 1,
            reconstructed: 7,
            qp: 30,
            slice: 1,
            edges: 0,
            transform_edges: 0,
            protected: false,
        }
    }

    fn references() -> References {
        let mut result = References::default();
        for (slot, (index, poc, long_term)) in
            result
                .entries
                .iter_mut()
                .zip([(11, 0, false), (12, 1, false), (11, 0, true)])
        {
            *slot = Some(ReferenceIdentity {
                index,
                poc,
                long_term,
            });
        }
        result.before = 3;
        result
    }

    fn sao_frame(bits: u32) -> Result<Frame> {
        let info = PictureInfo {
            width: 40,
            height: 40,
            visible_width: 40,
            visible_height: 40,
            bit_depth: bits,
            profile_idc: if bits == 8 { 1 } else { 2 },
            ctb_log2: 4,
            tiles: true,
            wpp: false,
            conformance_window: [0; 4],
            vui: crate::Vui::default(),
        };
        let retained = RetainedPicture::for_test(0, info, None);
        Ok(Frame {
            deadline: None,
            index: 0,
            info,
            depth: kernels::BitDepth::new(bits as u8)?,
            planes: retained.planes,
            cells: retained.cells,
            filter_ctus: vec![
                Ctu {
                    seen: true,
                    deblock_disabled: true,
                    ..Ctu::default()
                };
                9
            ],
            filtered: false,
            next_ctu: 9,
            segment_closed: true,
            contexts: syntax::Contexts::new(30, Initialization::I),
            wpp: None,
            qp: syntax::Qp::new(30),
            stats: IntraStats::default(),
            inter_stats: InterStats::default(),
            reference: None,
            hash_seen: false,
            scratch: Scratch {
                coefficients: Vec::new(),
                residual: Vec::new(),
                prediction: Vec::new(),
                output: Vec::new(),
                scaling: Vec::new(),
                references: Vec::new(),
                predictors: [Vec::new(), Vec::new()],
                inter_output: Vec::new(),
            },
        })
    }

    #[test]
    fn sao_rectangles_match_sample_decisions_at_every_block_corner() -> Result<()> {
        for bits in [8, 10] {
            for pattern in 0..32 {
                for direction in 0..4 {
                    let mut frame = sao_frame(bits)?;
                    for (i, cell) in frame.cells.iter_mut().enumerate() {
                        cell.protected = (i + pattern) % 7 == 0;
                    }
                    for (i, ctu) in frame.filter_ctus.iter_mut().enumerate() {
                        ctu.order = (8 - i) as u32;
                        ctu.tile = (i % 2) as u32;
                        ctu.slice = (i % 3) as u32;
                        ctu.across_tiles = pattern & 1 != 0;
                        ctu.across_slices = (pattern >> (i % 4)) & 2 != 0;
                        ctu.sao = [Sao {
                            kind: match (i + pattern) % 3 {
                                0 => 0,
                                1 => 1,
                                _ => 2,
                            },
                            offsets: [3, 1, -2, -3],
                            class: direction,
                            band: ((i + pattern) % 32) as u8,
                        }; 3];
                    }
                    for component in 0..3 {
                        let scale = if component == 0 { 1 } else { 2 };
                        let width = 40 / scale;
                        let height = 40 / scale;
                        let plane = frame
                            .planes
                            .get_mut(component)
                            .ok_or(Error::InvalidData("fixture_plane"))?;
                        for (i, value) in plane.iter_mut().enumerate() {
                            *value = ((i * 71 + i / width * 19 + pattern * 23) & ((1 << bits) - 1))
                                as u16;
                        }
                        let source = plane.clone();
                        let mut expected = source.clone();
                        for y in 0..height {
                            for x in 0..width {
                                let sao = frame
                                    .filter_ctu(x * scale, y * scale)?
                                    .sao
                                    .get(component)
                                    .copied()
                                    .ok_or(Error::InvalidData("fixture_sao"))?;
                                if sao.kind == 0 || frame.cell(x * scale, y * scale)?.protected {
                                    continue;
                                }
                                let class = if sao.kind == 1 {
                                    kernels::SaoClass::Band(sao.band)
                                } else {
                                    let [dx, dy] = match sao.class {
                                        0 => [1, 0],
                                        1 => [0, 1],
                                        2 => [1, 1],
                                        _ => [-1, 1],
                                    };
                                    let mut neighbours = [0; 2];
                                    let mut available = true;
                                    for (value, sign) in neighbours.iter_mut().zip([-1, 1]) {
                                        let (Some(nx), Some(ny)) = (
                                            x.checked_add_signed(sign * dx),
                                            y.checked_add_signed(sign * dy),
                                        ) else {
                                            available = false;
                                            break;
                                        };
                                        if nx >= width
                                            || ny >= height
                                            || !frame.across(
                                                [x * scale, y * scale],
                                                [nx * scale, ny * scale],
                                            )?
                                        {
                                            available = false;
                                            break;
                                        }
                                        *value = *source
                                            .get(ny * width + nx)
                                            .ok_or(Error::InvalidData("fixture_sample"))?;
                                    }
                                    if !available {
                                        continue;
                                    }
                                    kernels::SaoClass::Edge(neighbours)
                                };
                                let sample = *source
                                    .get(y * width + x)
                                    .ok_or(Error::InvalidData("fixture_sample"))?;
                                *expected
                                    .get_mut(y * width + x)
                                    .ok_or(Error::InvalidData("fixture_sample"))? =
                                    kernels::sao_sample(frame.depth, sample, sao.offsets, class)?;
                            }
                        }
                        frame.sao_component(component, Some(&source))?;
                        assert_eq!(
                            frame.planes.get(component),
                            Some(&expected),
                            "{bits}/{pattern}/{direction}/{component}"
                        );
                        assert_eq!(
                            frame.stats.sao_changed_samples.get(component).copied(),
                            Some(
                                source.iter().zip(&expected).filter(|(a, b)| a != b).count() as u64
                            )
                        );
                    }
                }
            }
        }
        Ok(())
    }

    #[test]
    fn band_only_finalize_needs_no_extra_component_allocation() -> Result<()> {
        let mut frame = sao_frame(8)?;
        for plane in &mut frame.planes {
            plane.fill(0);
        }
        for cell in &mut frame.cells {
            cell.reconstructed = 7;
        }
        for ctu in &mut frame.filter_ctus {
            ctu.sao = [Sao {
                kind: 1,
                offsets: [1; 4],
                ..Sao::default()
            }; 3];
        }
        let live = frame.heap_bytes()?;
        assert_eq!(frame.finalize(0, live)?, 0);
        assert!(frame.planes.iter().flatten().all(|&sample| sample == 1));
        Ok(())
    }

    #[test]
    fn strength_uses_picture_identity_vector_count_and_the_four_quarter_sample_threshold()
    -> Result<()> {
        let references = references();
        let p = cell(Some(Motion::L0(reference(0, 0, 0, 0))));
        for (motion, expected) in [
            (Motion::L1(reference(2, 5, 3, -3)), BoundaryStrength::Zero),
            (Motion::L0(reference(0, 0, 4, 0)), BoundaryStrength::One),
            (Motion::L0(reference(0, 0, 0, -4)), BoundaryStrength::One),
            (Motion::L0(reference(1, 0, 0, 0)), BoundaryStrength::One),
            (
                Motion::Bi([reference(0, 0, 0, 0); 2]),
                BoundaryStrength::One,
            ),
        ] {
            assert_eq!(
                strength(p, cell(Some(motion)), Some(&references), false)?,
                expected
            );
        }
        assert_eq!(
            strength(
                cell(Some(Motion::L0(reference(0, 0, i16::MIN, 0)))),
                cell(Some(Motion::L0(reference(0, 0, i16::MAX, 0)))),
                Some(&references),
                false
            )?,
            BoundaryStrength::One
        );
        assert_eq!(strength(p, cell(None), None, false)?, BoundaryStrength::Two);
        assert_eq!(strength(cell(None), p, None, false)?, BoundaryStrength::Two);
        assert_eq!(
            strength(p, p, None, false),
            Err(Error::InvalidData("filter_reference_catalog"))
        );
        Ok(())
    }

    #[test]
    fn bipred_strength_accepts_equivalent_permutations_including_a_shared_picture() -> Result<()> {
        let references = references();
        for (p, q, expected) in [
            (
                [reference(0, 0, 0, 0), reference(1, 0, 8, 0)],
                [reference(1, 4, 8, 0), reference(0, 2, 0, 0)],
                BoundaryStrength::Zero,
            ),
            (
                [reference(0, 0, 0, 0), reference(0, 0, 8, 0)],
                [reference(0, 1, 8, 0), reference(0, 3, 0, 0)],
                BoundaryStrength::Zero,
            ),
            (
                [reference(0, 0, 0, 0), reference(0, 0, 8, 0)],
                [reference(0, 0, 4, 0), reference(0, 0, 4, 0)],
                BoundaryStrength::One,
            ),
            (
                [reference(0, 0, 0, 0), reference(0, 0, 8, 0)],
                [reference(0, 0, 3, 0), reference(0, 0, 7, 0)],
                BoundaryStrength::Zero,
            ),
        ] {
            assert_eq!(
                strength(
                    cell(Some(Motion::Bi(p))),
                    cell(Some(Motion::Bi(q))),
                    Some(&references),
                    false
                )?,
                expected
            );
        }
        Ok(())
    }

    #[test]
    fn only_transform_edges_get_residual_strength_and_no_root_cu_borders_are_inferred() -> Result<()>
    {
        let references = references();
        let p = cell(Some(Motion::L0(reference(0, 0, 0, 0))));
        let mut q = p;
        q.luma_residual = true;
        assert_eq!(
            strength(p, q, Some(&references), false)?,
            BoundaryStrength::Zero
        );
        assert_eq!(
            strength(p, q, Some(&references), true)?,
            BoundaryStrength::One
        );
        assert!(!transform_boundary(q, [8, 4], 5, false)?);
        assert!(transform_boundary(q, [16, 4], 5, false)?);
        assert!(transform_boundary(q, [4, 16], 5, true)?);
        q.transform_edges = 1;
        assert!(transform_boundary(q, [8, 4], 5, false)?);
        assert!(!transform_boundary(q, [8, 4], 5, true)?);
        Ok(())
    }

    #[test]
    fn actual_inter_strength_one_filters_luma_but_never_chroma_at_both_depths() -> Result<()> {
        for bits in [8, 10] {
            let info = PictureInfo {
                width: 32,
                height: 16,
                visible_width: 32,
                visible_height: 16,
                bit_depth: bits,
                profile_idc: if bits == 8 { 1 } else { 2 },
                ctb_log2: 5,
                tiles: false,
                wpp: false,
                conformance_window: [0; 4],
                vui: crate::Vui::default(),
            };
            let retained = RetainedPicture::for_test(0, info, None);
            let mut frame = Frame {
                workers: 1,
                deadline: None,
                index: 0,
                info,
                depth: kernels::BitDepth::new(bits as u8)?,
                planes: retained.planes,
                cells: retained.cells,
                filter_ctus: vec![Ctu {
                    seen: true,
                    slice: 1,
                    across_tiles: true,
                    across_slices: true,
                    ..Ctu::default()
                }],
                filtered: false,
                next_ctu: 1,
                segment_closed: true,
                contexts: syntax::Contexts::new(30, Initialization::P),
                wpp: None,
                qp: syntax::Qp::new(30),
                stats: IntraStats::default(),
                inter_stats: InterStats::default(),
                reference: Some(ReferenceState {
                    poc: 2,
                    references: references(),
                }),
                hash_seen: false,
                scratch: Scratch {
                    coefficients: Vec::new(),
                    residual: Vec::new(),
                    prediction: Vec::new(),
                    output: Vec::new(),
                    scaling: Vec::new(),
                    references: Vec::new(),
                    predictors: [Vec::new(), Vec::new()],
                    inter_output: Vec::new(),
                },
            };
            let scale = 1u16 << (bits - 8);
            for (index, value) in frame.cells.iter_mut().enumerate() {
                let x = index % 8 * 4;
                let y = index / 8 * 4;
                *value = cell(Some(Motion::L0(reference(
                    0,
                    0,
                    if x < 16 { 0 } else { 4 },
                    0,
                ))));
                value.edges = u8::from(x % 16 == 0) | (u8::from(y == 0) << 1);
            }
            for (component, plane) in frame.planes.iter_mut().enumerate() {
                let width = if component == 0 { 32 } else { 16 };
                for (index, value) in plane.iter_mut().enumerate() {
                    *value = if index % width < width / 2 {
                        100 * scale
                    } else {
                        140 * scale
                    };
                }
            }
            frame.finalize(0, usize::MAX)?;
            assert_eq!(frame.stats.deblock_edges, [4, 0, 0]);
            assert_eq!(frame.inter_stats.boundary_strengths, [0, 4, 0]);
            let luma = frame
                .planes
                .first()
                .ok_or(Error::InvalidData("fixture_plane"))?;
            for row in luma.chunks_exact(32) {
                assert_eq!(row.get(15), Some(&(102 * scale)));
                assert_eq!(row.get(16), Some(&(138 * scale)));
            }
            for plane in frame.planes.iter().skip(1) {
                for row in plane.chunks_exact(16) {
                    assert!(row.iter().take(8).all(|v| *v == 100 * scale));
                    assert!(row.iter().skip(8).all(|v| *v == 140 * scale));
                }
            }
        }
        Ok(())
    }
}
