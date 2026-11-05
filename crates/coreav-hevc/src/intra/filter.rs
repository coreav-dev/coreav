use super::{Frame, Memory};
use crate::{Error, Result};
use coreav_kernels_hevc::{self as kernels, SaoClass};

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
    pub(super) fn finalize(&mut self, fixed: usize, limit: usize) -> Result<usize> {
        if !self.complete() || self.filter_ctus.iter().any(|c| !c.seen) {
            return Err(Error::InvalidData("selected_picture_incomplete"));
        }
        if self.filtered {
            return Ok(0);
        }
        for horizontal in [false, true] {
            for component in 0..3 {
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
            let mut memory = Memory::new(fixed, limit)?;
            memory.charge(self.heap_bytes()?)?;
            let plane = self
                .planes
                .get(component)
                .ok_or(Error::InvalidData("filter_component"))?;
            let mut source = memory.allocate(plane.len(), 0u16)?;
            source.copy_from_slice(plane);
            peak = peak.max(memory.used);
            self.sao_component(component, &source)?;
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
                let qp = (i32::from(qcell.qp) + i32::from(pcell.qp) + 1) >> 1;
                let protected = [pcell.protected, qcell.protected];
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
                    let result =
                        kernels::deblock_luma(self.depth, samples, qp, ctu.offsets, protected)?;
                    self.write_edge(component, horizontal, edge, along, result)?;
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
                    self.write_edge(component, horizontal, edge, along, result)?;
                }
                *self
                    .stats
                    .deblock_edges
                    .get_mut(component)
                    .ok_or(Error::InvalidData("filter_component"))? += 1;
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
        result: [[u16; N]; 4],
    ) -> Result<()> {
        let width = self.info.width as usize / if component == 0 { 1 } else { 2 };
        for (row, line) in result.into_iter().enumerate() {
            for (index, value) in line.into_iter().enumerate() {
                let address = if horizontal {
                    (edge + index - N / 2) * width + along + row
                } else {
                    (along + row) * width + edge + index - N / 2
                };
                let target = self
                    .planes
                    .get_mut(component)
                    .and_then(|p| p.get_mut(address))
                    .ok_or(Error::InvalidData("deblock_sample"))?;
                *self
                    .stats
                    .deblock_changed_samples
                    .get_mut(component)
                    .ok_or(Error::InvalidData("filter_component"))? += u64::from(*target != value);
                *target = value;
            }
        }
        Ok(())
    }

    fn sao_component(&mut self, component: usize, source: &[u16]) -> Result<()> {
        let scale = if component == 0 { 1 } else { 2 };
        let width = self.info.width as usize / scale;
        let height = self.info.height as usize / scale;
        let side = (1usize << self.info.ctb_log2) / scale;
        for cy in (0..height).step_by(side) {
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
                for y in cy..(cy + side).min(height) {
                    for x in cx..(cx + side).min(width) {
                        if self.cell(x * scale, y * scale)?.protected {
                            continue;
                        }
                        let original = *source
                            .get(y * width + x)
                            .ok_or(Error::InvalidData("sao_sample"))?;
                        let class = if sao.kind == 1 {
                            SaoClass::Band(sao.band)
                        } else {
                            let [dx, dy] = match sao.class {
                                0 => [1, 0],
                                1 => [0, 1],
                                2 => [1, 1],
                                3 => [-1, 1],
                                _ => return Err(Error::InvalidData("sao_class")),
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
                                    || !self
                                        .across([x * scale, y * scale], [nx * scale, ny * scale])?
                                {
                                    available = false;
                                    break;
                                }
                                *value = *source
                                    .get(ny * width + nx)
                                    .ok_or(Error::InvalidData("sao_neighbour"))?;
                            }
                            if !available {
                                continue;
                            }
                            SaoClass::Edge(neighbours)
                        };
                        let value = kernels::sao_sample(self.depth, original, sao.offsets, class)?;
                        *self
                            .planes
                            .get_mut(component)
                            .and_then(|p| p.get_mut(y * width + x))
                            .ok_or(Error::InvalidData("sao_sample"))? = value;
                        *self
                            .stats
                            .sao_changed_samples
                            .get_mut(component)
                            .ok_or(Error::InvalidData("sao_component"))? +=
                            u64::from(value != original);
                    }
                }
            }
        }
        Ok(())
    }
}
