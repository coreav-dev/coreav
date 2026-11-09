#![forbid(unsafe_code)]

use super::*;
use crate::motion::{Layout, Part};
use coreav_kernels_h264::Block;

const SPLIT_SATD: u64 = 64;
const SETTLED_SPLIT_SATD: u64 = 4 * SPLIT_SATD;
const HEXAGON_ROUNDS: usize = 16;
const NEAR_DIVISOR: u64 = 8;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Inter {
    pub(super) layout: Layout,
    pub(super) parts: [Part; 16],
    pub(super) len: usize,
    differences: [Mv; 16],
    pub(super) skip: bool,
}

impl Inter {
    fn new(layout: Layout) -> Result<Self> {
        let (parts, len) = layout.parts()?;
        Ok(Self {
            layout,
            parts,
            len,
            differences: [Mv::default(); 16],
            skip: false,
        })
    }

    fn derive(&mut self, border: Border<'_>) -> Result<()> {
        let mut grid = [None; 16];
        for (part, difference) in self.parts.iter().zip(&mut self.differences).take(self.len) {
            *difference = part
                .mv
                .difference(border.predictor(&grid, *part, self.layout)?)?;
            motion::commit(&mut grid, *part)?;
        }
        Ok(())
    }

    pub(super) fn motion(&self) -> Result<MotionMb> {
        let mut grid = [None; 16];
        for &part in self.parts.iter().take(self.len) {
            motion::commit(&mut grid, part)?;
        }
        let mut vectors = [Mv::default(); 16];
        for (v, p) in vectors.iter_mut().zip(grid) {
            *v = p.ok_or(Error::Internal)?;
        }
        Ok(MotionMb {
            intra: false,
            vectors,
        })
    }

    pub(super) fn syntax(&self, w: &mut Bits) -> Result<()> {
        w.ue(self.layout.code())?;
        if let Layout::Sub(shapes) = self.layout {
            for shape in shapes {
                w.ue(u32::from(shape))?;
            }
        }
        for &d in self.differences.iter().take(self.len) {
            d.write(w)?;
        }
        Ok(())
    }
}

pub(super) fn predict(
    reference: &FramePlane,
    tile: &mut Tile,
    part: Part,
    mx: usize,
    my: usize,
    chroma: bool,
) -> Result<()> {
    let scale = if chroma { 2 } else { 1 };
    let divisor = if chroma { 8 } else { 4 };
    let margin = if chroma { 0 } else { 2 };
    let extra = if chroma { 1 } else { 5 };
    let (x, y) = (part.x / scale, part.y / scale);
    let (width, height) = (part.width / scale, part.height / scale);
    let ix = (mx * 16 / scale + x) as i32 + part.mv.x.div_euclid(divisor) - margin;
    let iy = (my * 16 / scale + y) as i32 + part.mv.y.div_euclid(divisor) - margin;
    let mut window = [0; 21 * 21];
    let src = if ix >= 0
        && iy >= 0
        && ix as usize + width + extra <= reference.width
        && iy as usize + height + extra <= reference.height
    {
        Block::new(
            &reference.data,
            iy as usize * reference.width + ix as usize,
            reference.width,
            width + extra,
            height + extra,
        )
        .map_err(|_| Error::Internal)?
    } else {
        for j in 0..height + extra {
            reference.row(
                ix,
                iy + j as i32,
                window
                    .get_mut(j * 21..j * 21 + width + extra)
                    .ok_or(Error::Internal)?,
            )?;
        }
        Block::new(&window, 0, 21, width + extra, height + extra).map_err(|_| Error::Internal)?
    };
    let dst = BlockMut::new(&mut tile.0, Tile::at(x, y), STRIDE, width, height)
        .map_err(|_| Error::Internal)?;
    let (xf, yf) = (
        part.mv.x.rem_euclid(divisor) as u8,
        part.mv.y.rem_euclid(divisor) as u8,
    );
    if chroma {
        crate::kernels::pixels().chroma_mc(dst, src, xf, yf);
    } else {
        crate::kernels::pixels().luma_mc(dst, src, xf, yf);
    }
    Ok(())
}

pub(super) struct Search<'a> {
    pub(super) reference: &'a [FramePlane; 3],
    pub(super) decision: &'a Decision<'a>,
    pub(super) border: Border<'a>,
    pub(super) mx: usize,
    pub(super) my: usize,
    pub(super) previous: u8,
    pub(super) colocated: Option<&'a MotionMb>,
}

type Ranked = (u64, usize, Mv);

fn motion_bits(difference: Mv) -> usize {
    [difference.x, difference.y]
        .into_iter()
        .map(|value| (u32::BITS - value.unsigned_abs().leading_zeros()) as usize * 2 + 1)
        .sum()
}

fn score_rows(
    source: &[u8],
    predicted: &[u8],
    stride: usize,
    width: usize,
    motion_cost: u64,
    cutoff: Option<u64>,
) -> Result<Option<u64>> {
    let sad = crate::kernels::Sad::new(width)?;
    let mut score = motion_cost;
    for (original, predicted) in source.chunks(STRIDE).zip(predicted.chunks(stride)) {
        score += sad.row(original, predicted)?;
        if cutoff.is_some_and(|limit| score > limit) {
            return Ok(None);
        }
    }
    Ok(Some(score))
}

struct Motions {
    seen: [Option<Ranked>; 99],
    len: usize,
    ranked: [Option<Ranked>; 4],
    integer: Option<Ranked>,
}

impl Motions {
    fn new() -> Self {
        Self {
            seen: [None; 99],
            len: 0,
            ranked: [None; 4],
            integer: None,
        }
    }

    fn insert(&mut self, value: Ranked) -> Result<()> {
        *self.seen.get_mut(self.len).ok_or(Error::Internal)? = Some(value);
        self.len += 1;
        if value.2.x % 4 == 0 && value.2.y % 4 == 0 && self.integer.is_none_or(|best| value < best)
        {
            self.integer = Some(value);
        }
        let mut next = Some(value);
        for slot in &mut self.ranked {
            if next.is_some_and(|value| slot.is_none_or(|best| value < best)) {
                std::mem::swap(slot, &mut next);
            }
        }
        Ok(())
    }

    #[cfg(test)]
    fn finalists(&self, predictor: Mv) -> Result<[Option<Mv>; 6]> {
        let mut finalists = [None; 6];
        let mut len = 0;
        for mv in self
            .ranked
            .iter()
            .flatten()
            .map(|value| value.2)
            .chain([Mv::default(), predictor])
        {
            if finalists.contains(&Some(mv))
                || !self
                    .seen
                    .get(..self.len)
                    .ok_or(Error::Internal)?
                    .iter()
                    .flatten()
                    .any(|value| value.2 == mv)
            {
                continue;
            }
            *finalists.get_mut(len).ok_or(Error::Internal)? = Some(mv);
            len += 1;
        }
        Ok(finalists)
    }
}

#[cfg(test)]
struct Prepared {
    luma: Luma,
    chroma: [Tile; 2],
    sums: [[i32; 4]; 2],
    levels: [[[i32; 16]; 4]; 2],
}

#[cfg(test)]
fn overlaps(part: Part, x: usize, y: usize, size: usize) -> bool {
    x < part.x + part.width && x + size > part.x && y < part.y + part.height && y + size > part.y
}

impl Search<'_> {
    fn score(
        &self,
        mut part: Part,
        mv: Mv,
        predictor: Mv,
        motions: &mut Motions,
    ) -> Result<Option<Ranked>> {
        #[cfg(test)]
        tests::PROBES.with(|count| count.set(count.get() + 1));
        if let Some(&value) = motions
            .seen
            .get(..motions.len)
            .ok_or(Error::Internal)?
            .iter()
            .flatten()
            .find(|value| value.2 == mv)
        {
            return Ok(Some(value));
        }
        if !mv.valid() {
            return Ok(None);
        }
        let difference = match mv.difference(predictor) {
            Ok(difference) => difference,
            Err(Error::Syntax) => return Ok(None),
            Err(e) => return Err(e),
        };
        let bits = motion_bits(difference);
        let motion_cost = (1u64 << (self.decision.qp / 6)) * bits as u64;
        let cutoff = motions
            .ranked
            .last()
            .copied()
            .flatten()
            .zip(motions.integer)
            .map(|(ranked, integer)| ranked.0.max(integer.0));
        if cutoff.is_some_and(|limit| motion_cost > limit) {
            return Ok(None);
        }
        let reference = self.reference.first().ok_or(Error::Internal)?;
        let at = Tile::at(part.x, part.y);
        let span = part.height.checked_sub(1).ok_or(Error::Internal)? * STRIDE + part.width;
        let source = self
            .decision
            .source
            .0
            .get(at..at + span)
            .ok_or(Error::Internal)?;
        let score = if mv.x % 4 == 0 && mv.y % 4 == 0 {
            let ix = (self.mx * 16 + part.x) as i32 + mv.x / 4;
            let iy = (self.my * 16 + part.y) as i32 + mv.y / 4;
            if ix >= 0
                && iy >= 0
                && ix as usize + part.width <= reference.width
                && iy as usize + part.height <= reference.height
            {
                let start = iy as usize * reference.width + ix as usize;
                let end = start + (part.height - 1) * reference.width + part.width;
                score_rows(
                    source,
                    reference.data.get(start..end).ok_or(Error::Internal)?,
                    reference.width,
                    part.width,
                    motion_cost,
                    cutoff,
                )?
            } else {
                let mut score = motion_cost;
                for (y, original) in source.chunks(STRIDE).enumerate() {
                    let mut row = [0; 16];
                    let row = row.get_mut(..part.width).ok_or(Error::Internal)?;
                    reference.row(ix, iy + y as i32, row)?;
                    score += crate::kernels::sad(
                        original.get(..part.width).ok_or(Error::Internal)?,
                        row,
                    )?;
                    if cutoff.is_some_and(|limit| score > limit) {
                        return Ok(None);
                    }
                }
                Some(score)
            }
        } else {
            part.mv = mv;
            let mut tile = Tile::empty();
            predict(reference, &mut tile, part, self.mx, self.my, false)?;
            score_rows(
                source,
                tile.0.get(at..at + span).ok_or(Error::Internal)?,
                STRIDE,
                part.width,
                motion_cost,
                cutoff,
            )?
        };
        let Some(score) = score else {
            return Ok(None);
        };
        let value = (score, bits, mv);
        motions.insert(value)?;
        Ok(Some(value))
    }

    #[cfg(test)]
    fn motions(&self, part: Part, predictor: Mv) -> Result<Motions> {
        let mut motions = Motions::new();
        self.score(part, Mv::default(), predictor, &mut motions)?;
        self.score(part, predictor, predictor, &mut motions)?;
        self.score(
            part,
            Mv {
                x: (predictor.x + 2).div_euclid(4) * 4,
                y: (predictor.y + 2).div_euclid(4) * 4,
            },
            predictor,
            &mut motions,
        )?;
        for step in [64, 32, 16, 8, 4] {
            for _ in 0..2 {
                let centre = motions.integer.ok_or(Error::Internal)?.2;
                for dx in [-step, 0, step] {
                    for dy in [-step, 0, step] {
                        let mv = Mv {
                            x: centre.x + dx,
                            y: centre.y + dy,
                        };
                        if (dx != 0 || dy != 0)
                            && (-64..=64).contains(&mv.x)
                            && (-64..=64).contains(&mv.y)
                        {
                            self.score(part, mv, predictor, &mut motions)?;
                        }
                    }
                }
                if motions.integer.ok_or(Error::Internal)?.2 == centre {
                    break;
                }
            }
        }
        let mut centre = motions.integer.ok_or(Error::Internal)?;
        for step in [2, 1] {
            let origin = centre.2;
            for dx in [-step, 0, step] {
                for dy in [-step, 0, step] {
                    if dx != 0 || dy != 0 {
                        if let Some(value) = self.score(
                            part,
                            Mv {
                                x: origin.x + dx,
                                y: origin.y + dy,
                            },
                            predictor,
                            &mut motions,
                        )? {
                            centre = centre.min(value);
                        }
                    }
                }
            }
        }
        Ok(motions)
    }

    #[cfg(test)]
    fn prepare(&self, base: &Inter, index: usize) -> Result<Prepared> {
        let changed = *base.parts.get(index).ok_or(Error::Internal)?;
        let mut prepared = Prepared {
            luma: Luma {
                mode: None,
                modes: [2; 16],
                levels: [[0; 16]; 16],
                dc: [0; 16],
                tile: Tile::empty(),
            },
            chroma: [Tile::empty(), Tile::empty()],
            sums: [[0; 4]; 2],
            levels: [[[0; 16]; 4]; 2],
        };
        for (i, &part) in base.parts.iter().take(base.len).enumerate() {
            if i == index {
                continue;
            }
            predict(
                self.reference.first().ok_or(Error::Internal)?,
                &mut prepared.luma.tile,
                part,
                self.mx,
                self.my,
                false,
            )?;
            for (c, tile) in prepared.chroma.iter_mut().enumerate() {
                predict(
                    self.reference.get(c + 1).ok_or(Error::Internal)?,
                    tile,
                    part,
                    self.mx,
                    self.my,
                    true,
                )?;
            }
        }
        let quant = t::Quant::of(self.decision.qp)?;
        for (k, levels) in prepared.luma.levels.iter_mut().enumerate() {
            let (x, y) = xy(k);
            if !overlaps(changed, x, y, 4) {
                *levels = prepared.luma.tile.code(self.decision.source, x, y, quant)?;
            }
        }
        let quant = t::Quant::chroma_inter(t::chroma_qp(self.decision.qp))?;
        for (((tile, source), sums), blocks) in prepared
            .chroma
            .iter()
            .zip(self.decision.csources)
            .zip(&mut prepared.sums)
            .zip(&mut prepared.levels)
        {
            for (k, (sum, levels)) in sums.iter_mut().zip(blocks).enumerate() {
                if !overlaps(changed, k % 2 * 8, k / 2 * 8, 8) {
                    let coefficients = tile.forward(source, k % 2 * 4, k / 2 * 4)?;
                    *sum = coefficients.dc();
                    *levels = quant.levels(&coefficients, true);
                }
            }
        }
        Ok(prepared)
    }

    #[cfg(test)]
    fn evaluate_part(&self, mut inter: Inter, index: usize, base: &Prepared) -> Result<Candidate> {
        if !motion::admitted(self.previous, inter.len) {
            return Err(Error::Syntax);
        }
        inter.derive(self.border)?;
        let part = *inter.parts.get(index).ok_or(Error::Internal)?;
        let mut luma = base.luma.clone();
        predict(
            self.reference.first().ok_or(Error::Internal)?,
            &mut luma.tile,
            part,
            self.mx,
            self.my,
            false,
        )?;
        let quant = t::Quant::of(self.decision.qp)?;
        for (k, levels) in luma.levels.iter_mut().enumerate() {
            let (x, y) = xy(k);
            if overlaps(part, x, y, 4) {
                *levels = luma.tile.code(self.decision.source, x, y, quant)?;
            }
        }
        let mut chroma = base.chroma.clone();
        let mut sums = base.sums;
        let mut levels = base.levels;
        let quant = t::Quant::chroma_inter(t::chroma_qp(self.decision.qp))?;
        for (c, (((tile, source), sums), blocks)) in chroma
            .iter_mut()
            .zip(self.decision.csources)
            .zip(&mut sums)
            .zip(&mut levels)
            .enumerate()
        {
            predict(
                self.reference.get(c + 1).ok_or(Error::Internal)?,
                tile,
                part,
                self.mx,
                self.my,
                true,
            )?;
            for (k, (sum, levels)) in sums.iter_mut().zip(blocks).enumerate() {
                if overlaps(part, k % 2 * 8, k / 2 * 8, 8) {
                    let coefficients = tile.forward(source, k % 2 * 4, k / 2 * 4)?;
                    *sum = coefficients.dc();
                    *levels = quant.levels(&coefficients, true);
                }
            }
        }
        let chroma = chroma_reconstruct(&chroma, &sums, levels, t::chroma_qp(self.decision.qp))?;
        self.decision.candidate(luma, chroma, Some(inter), 0)
    }

    fn evaluate(&self, mut inter: Inter) -> Result<Candidate> {
        #[cfg(test)]
        tests::EVALUATIONS.with(|count| count.set(count.get() + 1));
        if !motion::admitted(self.previous, inter.len) {
            return Err(Error::Syntax);
        }
        inter.derive(self.border)?;
        let mut luma = Luma {
            mode: None,
            modes: [2; 16],
            levels: [[0; 16]; 16],
            dc: [0; 16],
            tile: Tile::empty(),
        };
        let mut chroma_tiles = [Tile::empty(), Tile::empty()];
        for &part in inter.parts.iter().take(inter.len) {
            predict(
                self.reference.first().ok_or(Error::Internal)?,
                &mut luma.tile,
                part,
                self.mx,
                self.my,
                false,
            )?;
            for (c, tile) in chroma_tiles.iter_mut().enumerate() {
                predict(
                    self.reference.get(c + 1).ok_or(Error::Internal)?,
                    tile,
                    part,
                    self.mx,
                    self.my,
                    true,
                )?;
            }
        }
        let chroma = if inter.skip {
            Chroma {
                mode: 0,
                levels: [[[0; 16]; 4]; 2],
                dc: [[0; 4]; 2],
                tiles: chroma_tiles,
            }
        } else {
            let quant = t::Quant::of(self.decision.qp)?;
            for (k, levels) in luma.levels.iter_mut().enumerate() {
                let (x, y) = xy(k);
                *levels = luma.tile.code(self.decision.source, x, y, quant)?;
            }
            chroma_residual(
                &chroma_tiles,
                self.decision.csources,
                t::chroma_qp(self.decision.qp),
            )?
        };
        self.decision.candidate(luma, chroma, Some(inter), 0)
    }

    #[cfg(test)]
    fn trial(
        &self,
        base: &Inter,
        index: usize,
        mv: Mv,
        best: &mut Option<Candidate>,
        refusal: &mut Error,
    ) -> Result<()> {
        let prepared = self.prepare(base, index)?;
        self.trial_prepared(base, index, mv, &prepared, best, refusal)
    }

    #[cfg(test)]
    fn trial_prepared(
        &self,
        base: &Inter,
        index: usize,
        mv: Mv,
        prepared: &Prepared,
        best: &mut Option<Candidate>,
        refusal: &mut Error,
    ) -> Result<()> {
        if !mv.valid() {
            return Ok(());
        }
        let mut candidate = base.clone();
        candidate.parts.get_mut(index).ok_or(Error::Internal)?.mv = mv;
        match self.evaluate_part(candidate, index, prepared) {
            Ok(candidate) => consider(best, candidate),
            Err(Error::Coefficient | Error::Syntax) => {}
            Err(Error::MacroblockLimit) => *refusal = Error::MacroblockLimit,
            Err(e) => return Err(e),
        }
        Ok(())
    }

    #[cfg(test)]
    fn search_parts(
        &self,
        mut base: Inter,
        start: usize,
        end: usize,
        refusal: &mut Error,
    ) -> Result<Option<Candidate>> {
        #[cfg(test)]
        tests::SEARCHES.with(|count| count.set(count.get() + 1));
        let mut result = None;
        for index in start..end {
            let mut grid = [None; 16];
            for &p in base.parts.iter().take(index) {
                motion::commit(&mut grid, p)?;
            }
            let part = *base.parts.get(index).ok_or(Error::Internal)?;
            let predictor = self.border.predictor(&grid, part, base.layout)?;
            let prepared = match self.prepare(&base, index) {
                Ok(prepared) => prepared,
                Err(Error::Coefficient | Error::Syntax) => return Ok(None),
                Err(e) => return Err(e),
            };
            let motions = self.motions(part, predictor)?;
            let mut best = None;
            for mv in motions.finalists(predictor)?.into_iter().flatten() {
                self.trial_prepared(&base, index, mv, &prepared, &mut best, refusal)?;
            }
            let Some(best) = best else {
                return Ok(None);
            };
            base = best.inter.as_ref().ok_or(Error::Internal)?.clone();
            result = Some(best);
        }
        Ok(result)
    }

    fn compact(&self, part: Part, predictor: Mv, seeds: &[Option<Mv>]) -> Result<Ranked> {
        let mut motions = Motions::new();
        let rounded = |mv: Mv| Mv {
            x: (mv.x + 2).div_euclid(4) * 4,
            y: (mv.y + 2).div_euclid(4) * 4,
        };
        for mv in [Mv::default(), predictor, rounded(predictor)]
            .into_iter()
            .chain(seeds.iter().flatten().map(|&mv| rounded(mv)))
        {
            self.score(part, mv, predictor, &mut motions)?;
        }
        let quantum = (part.width * part.height) as u64 * (1u64 << (self.decision.qp / 6));
        let near = |motions: &Motions| {
            motions
                .integer
                .is_some_and(|best| best.0 * NEAR_DIVISOR <= quantum)
        };
        if !near(&motions) {
            for _ in 0..HEXAGON_ROUNDS {
                let origin = motions.integer.ok_or(Error::Internal)?.2;
                for (dx, dy) in [(8, 0), (-8, 0), (4, 8), (-4, 8), (4, -8), (-4, -8)] {
                    self.score(
                        part,
                        Mv {
                            x: origin.x + dx,
                            y: origin.y + dy,
                        },
                        predictor,
                        &mut motions,
                    )?;
                }
                if motions.integer.ok_or(Error::Internal)?.2 == origin {
                    break;
                }
            }
            let origin = motions.integer.ok_or(Error::Internal)?.2;
            for (dx, dy) in [(4, 0), (-4, 0), (0, 4), (0, -4)] {
                self.score(
                    part,
                    Mv {
                        x: origin.x + dx,
                        y: origin.y + dy,
                    },
                    predictor,
                    &mut motions,
                )?;
            }
        }
        let mut centre = motions.integer.ok_or(Error::Internal)?;
        let steps: &[i32] = if near(&motions) { &[2] } else { &[2, 1] };
        for &step in steps {
            let origin = centre.2;
            for (dx, dy) in [(step, 0), (-step, 0), (0, step), (0, -step)] {
                if let Some(value) = self.score(
                    part,
                    Mv {
                        x: origin.x + dx,
                        y: origin.y + dy,
                    },
                    predictor,
                    &mut motions,
                )? {
                    centre = centre.min(value);
                }
            }
        }
        let retained = [
            motions.ranked.first().copied().flatten(),
            motions.ranked.get(1).copied().flatten(),
            motions
                .seen
                .get(..motions.len)
                .ok_or(Error::Internal)?
                .iter()
                .flatten()
                .find(|value| value.2 == predictor)
                .copied(),
        ];
        let mut best = None;
        for (index, value) in retained.iter().enumerate() {
            let Some((_, bits, mv)) = *value else {
                continue;
            };
            if retained.iter().take(index).flatten().any(|old| old.2 == mv) {
                continue;
            }
            #[cfg(test)]
            tests::RESCORES.with(|count| count.set(count.get() + 1));
            let mut tile = Tile::empty();
            let mut proposed = part;
            proposed.mv = mv;
            predict(
                self.reference.first().ok_or(Error::Internal)?,
                &mut tile,
                proposed,
                self.mx,
                self.my,
                false,
            )?;
            let cost = tile.satd(self.decision.source, part.x, part.y, part.width)?
                + (1u64 << (self.decision.qp / 6)) * bits as u64;
            let value = (cost, bits, mv);
            if best.is_none_or(|old| value < old) {
                best = Some(value);
            }
        }
        best.ok_or(Error::Internal)
    }

    fn coalesce(&self, mut inter: Inter) -> Result<(usize, Inter)> {
        inter.derive(self.border)?;
        let mut bits = Bits::count();
        inter.syntax(&mut bits)?;
        let mut key = (bits.len, inter.len, inter.layout.code());
        if !matches!(inter.layout, Layout::Sub(_)) {
            return Ok((bits.len, inter));
        }
        let vectors = inter.motion()?.vectors;
        for layout in [Layout::Whole, Layout::Horizontal, Layout::Vertical] {
            let mut merged = Inter::new(layout)?;
            if !motion::admitted(self.previous, merged.len) {
                continue;
            }
            for part in merged.parts.iter_mut().take(merged.len) {
                part.mv = *vectors
                    .get(part.y / 4 * 4 + part.x / 4)
                    .ok_or(Error::Internal)?;
            }
            if merged.motion()?.vectors != vectors {
                continue;
            }
            merged.derive(self.border)?;
            let mut count = Bits::count();
            merged.syntax(&mut count)?;
            let next = (count.len, merged.len, merged.layout.code());
            if next < key {
                key = next;
                inter = merged;
            }
        }
        Ok((key.0, inter))
    }

    fn syntax_floor(&self, bits: usize) -> Result<u64> {
        let mut prefix = Bits::count();
        prefix.ue(self.decision.pending_skip)?;
        self.decision.cost(0, prefix.len + bits)
    }

    fn propose(
        &self,
        layout: Layout,
        ceiling: Option<u64>,
        seed: Option<Mv>,
    ) -> Result<Option<(usize, Inter, u64)>> {
        let mut inter = Inter::new(layout)?;
        if !motion::admitted(self.previous, inter.len) {
            return Ok(None);
        }
        #[cfg(test)]
        tests::SEARCHES.with(|count| count.set(count.get() + 1));
        let mut minimum = Bits::count();
        minimum.ue(Layout::Whole.code())?;
        Mv::default().write(&mut minimum)?;
        let floor = self.syntax_floor(minimum.len)?;
        if ceiling.is_some_and(|cost| cost < floor) {
            return Ok(None);
        }
        let mut grid = [None; 16];
        let mut rank = 0;
        for part in inter.parts.iter_mut().take(inter.len) {
            let predictor = self.border.predictor(&grid, *part, layout)?;
            let [a, b, c] = self.border.neighbour_vectors(&grid, *part)?;
            let colocated = self
                .colocated
                .filter(|mb| !mb.intra)
                .and_then(|mb| mb.vectors.get(part.y / 4 * 4 + part.x / 4).copied());
            let (cost, _, mv) = self.compact(*part, predictor, &[a, b, c, colocated, seed])?;
            rank += cost;
            part.mv = mv;
            motion::commit(&mut grid, *part)?;
        }
        let (bits, inter) = self.coalesce(inter)?;
        Ok(Some((bits, inter, rank)))
    }

    fn quiet(&self, skip: &Candidate) -> Result<bool> {
        let quant = t::Quant::of(self.decision.qp)?;
        for k in 0..16 {
            let (x, y) = xy(k);
            if !quant.zero(&skip.luma.tile.forward(self.decision.source, x, y)?, false) {
                return Ok(false);
            }
        }
        let qp = t::chroma_qp(self.decision.qp);
        let quant = t::Quant::chroma_inter(qp)?;
        for (tile, source) in skip.chroma.tiles.iter().zip(self.decision.csources) {
            let mut sums = [0; 4];
            for (k, sum) in sums.iter_mut().enumerate() {
                let block = tile.forward(source, k % 2 * 4, k / 2 * 4)?;
                *sum = block.dc();
                if !quant.zero(&block, true) {
                    return Ok(false);
                }
            }
            if !t::chroma_dc_quantizes_to_zero(sums, qp)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(super) fn candidates(
        &self,
        best: &mut Option<Candidate>,
        refusal: &mut Error,
    ) -> Result<IntraScope> {
        let mut settled = false;
        let mut coded = None;
        if motion::admitted(self.previous, 1) {
            let mv = self.border.skip()?;
            if mv.valid() {
                let mut skip = Inter::new(Layout::Whole)?;
                skip.skip = true;
                skip.parts.first_mut().ok_or(Error::Internal)?.mv = mv;
                match self.evaluate(skip) {
                    Ok(c) => {
                        let quiet = self.quiet(&c)?;
                        let cost = c.key.cost;
                        consider(best, c);
                        if quiet {
                            return Ok(IntraScope::None);
                        }
                        let mut inter = Inter::new(Layout::Whole)?;
                        inter.parts.first_mut().ok_or(Error::Internal)?.mv = mv;
                        coded = Some(mv);
                        match self.evaluate(inter) {
                            Ok(c) => {
                                settled = c.key.cost >= cost;
                                consider(best, c);
                            }
                            Err(Error::Coefficient | Error::Syntax) => {}
                            Err(Error::MacroblockLimit) => *refusal = Error::MacroblockLimit,
                            Err(e) => return Err(e),
                        }
                    }
                    Err(Error::Coefficient | Error::Syntax) => {}
                    Err(Error::MacroblockLimit) => *refusal = Error::MacroblockLimit,
                    Err(e) => return Err(e),
                }
            }
        }
        let ceiling = best.as_ref().map(|candidate| candidate.key.cost);
        let whole = self.propose(Layout::Whole, ceiling, None)?;
        let bound = if settled {
            SETTLED_SPLIT_SATD
        } else {
            SPLIT_SATD
        };
        let split = if whole
            .as_ref()
            .is_none_or(|plan| plan.2 > bound << (self.decision.qp / 6))
        {
            self.propose(
                Layout::Sub([0; 4]),
                ceiling,
                whole
                    .as_ref()
                    .and_then(|plan| plan.1.parts.first())
                    .map(|part| part.mv),
            )?
        } else {
            None
        };
        let rank = [whole.as_ref(), split.as_ref()]
            .into_iter()
            .flatten()
            .map(|plan| plan.2)
            .min();
        let mut plans = [whole, split];
        plans.sort_by_key(|plan| plan.as_ref().map_or(usize::MAX, |plan| plan.0));
        let [first, second] = &mut plans;
        if first
            .as_ref()
            .zip(second.as_ref())
            .is_some_and(|(a, b)| a.1 == b.1)
        {
            *second = None;
        }
        for (bits, inter, _) in plans.into_iter().flatten() {
            if inter.layout == Layout::Whole
                && coded.is_some_and(|mv| inter.parts.first().is_some_and(|part| part.mv == mv))
            {
                continue;
            }
            let floor = self.syntax_floor(bits)?;
            if best
                .as_ref()
                .is_some_and(|candidate| candidate.key.cost < floor)
            {
                continue;
            }
            match self.evaluate(inter) {
                Ok(candidate) => {
                    consider(best, candidate);
                }
                Err(Error::Coefficient | Error::Syntax) => {}
                Err(Error::MacroblockLimit) => *refusal = Error::MacroblockLimit,
                Err(e) => return Err(e),
            }
        }
        Ok(if settled {
            IntraScope::Sixteen
        } else {
            IntraScope::Quick(rank.filter(|_| best.is_some()))
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::{Colour, Gop, Limits, Settings};

    #[test]
    fn motion_lengths_match_written_syntax_over_the_complete_domain() {
        for value in -32768..=32767 {
            for difference in [
                Mv { x: value, y: 0 },
                Mv { x: 0, y: value },
                Mv { x: value, y: value },
            ] {
                let mut bits = Bits::new(16);
                difference.write(&mut bits).unwrap();
                assert_eq!(motion_bits(difference), bits.len);
            }
        }
    }

    std::thread_local! {
        pub(super) static SEARCHES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
        pub(super) static PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
        pub(super) static EVALUATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
        pub(super) static RESCORES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    #[test]
    fn retained_motion_reranking_is_deterministic_and_bounded() {
        let mut reference = [
            FramePlane::new(32, 32).unwrap(),
            FramePlane::new(16, 16).unwrap(),
            FramePlane::new(16, 16).unwrap(),
        ];
        for (p, sample) in reference[0].data.iter_mut().enumerate() {
            *sample = (p * 37 + p / 11 * 53) as u8;
        }
        let mut source = Tile::empty();
        for y in 0..16 {
            for x in 0..16 {
                source.0[Tile::at(x, y)] = (x * 23 + y * 19) as u8;
            }
        }
        let csources = [Tile::empty(), Tile::empty()];
        let decision = Decision {
            source: &source,
            csources: &csources,
            context: MbContext {
                top: None,
                left: None,
                p_slice: true,
            },
            qp: 28,
            pending_skip: 0,
        };
        let search = Search {
            reference: &reference,
            decision: &decision,
            border: Border::default(),
            mx: 0,
            my: 0,
            previous: 0,
            colocated: None,
        };
        let part = Inter::new(Layout::Whole).unwrap().parts[0];
        for predictor in [Mv::default(), Mv { x: 3, y: -7 }, Mv { x: 8191, y: 2047 }] {
            let mut selected = None;
            for _ in 0..2 {
                PROBES.with(|count| count.set(0));
                RESCORES.with(|count| count.set(0));
                let value = search.compact(part, predictor, &[]).unwrap();
                assert!(value.2.valid());
                assert!(value.2.difference(predictor).is_ok());
                assert!(PROBES.with(|count| count.get()) <= PART_PROBES);
                assert!((1..=3).contains(&RESCORES.with(|count| count.get())));
                if let Some(old) = selected {
                    assert_eq!(value, old);
                }
                selected = Some(value);
            }
        }
    }

    #[test]
    fn coalesced_motion_preserves_prediction_coefficients_and_probe_budget() {
        let mut reference = [
            FramePlane::new(64, 64).unwrap(),
            FramePlane::new(32, 32).unwrap(),
            FramePlane::new(32, 32).unwrap(),
        ];
        for plane in &mut reference {
            for (p, sample) in plane.data.iter_mut().enumerate() {
                *sample = (112 + (p / plane.width * 3 + p % plane.width * 5) % 32) as u8;
            }
        }
        let source = Tile::empty();
        let csources = [Tile::empty(), Tile::empty()];
        let decision = Decision {
            source: &source,
            csources: &csources,
            context: MbContext {
                top: None,
                left: None,
                p_slice: true,
            },
            qp: 28,
            pending_skip: 0,
        };
        for phase in 0..4 {
            let a = Mv {
                x: phase,
                y: -phase,
            };
            let b = Mv {
                x: phase + 4,
                y: 4 - phase,
            };
            let c = Mv {
                x: phase + 8,
                y: -phase,
            };
            let d = Mv {
                x: phase,
                y: 8 - phase,
            };
            for origin in 0..2 {
                let search = Search {
                    reference: &reference,
                    decision: &decision,
                    border: Border::default(),
                    mx: origin,
                    my: origin,
                    previous: 12,
                    colocated: None,
                };
                for (vectors, layout) in [
                    ([a, a, a, a], Layout::Whole),
                    ([a, a, b, b], Layout::Horizontal),
                    ([a, b, a, b], Layout::Vertical),
                    ([a, b, c, d], Layout::Sub([0; 4])),
                ] {
                    let mut original = Inter::new(Layout::Sub([0; 4])).unwrap();
                    for (part, mv) in original.parts.iter_mut().zip(vectors) {
                        part.mv = mv;
                    }
                    original.derive(search.border).unwrap();
                    let mut bits = Bits::count();
                    original.syntax(&mut bits).unwrap();
                    PROBES.with(|count| count.set(0));
                    let (merged_bits, merged) = search.coalesce(original.clone()).unwrap();
                    assert_eq!(PROBES.with(|count| count.get()), 0);
                    assert_eq!(merged.layout, layout);
                    assert!(motion::admitted(search.previous, merged.len));
                    assert_eq!(
                        merged.motion().unwrap().vectors,
                        original.motion().unwrap().vectors
                    );
                    let before = search.evaluate(original).unwrap();
                    let after = search.evaluate(merged).unwrap();
                    assert_eq!(before.luma.tile.0, after.luma.tile.0);
                    assert_eq!(before.luma.levels, after.luma.levels);
                    assert_eq!(before.chroma.levels, after.chroma.levels);
                    assert_eq!(before.chroma.dc, after.chroma.dc);
                    for (a, b) in before.chroma.tiles.iter().zip(&after.chroma.tiles) {
                        assert_eq!(a.0, b.0);
                    }
                    assert_eq!(before.key.bits - after.key.bits, bits.len - merged_bits);
                    assert_eq!(
                        before.key.cost - after.key.cost,
                        decision.cost(0, bits.len - merged_bits).unwrap()
                    );
                }
            }
        }
    }

    #[test]
    fn split_search_runs_only_when_the_whole_prediction_leaves_a_large_satd() {
        let mut reference = [
            FramePlane::new(16, 16).unwrap(),
            FramePlane::new(8, 8).unwrap(),
            FramePlane::new(8, 8).unwrap(),
        ];
        for plane in &mut reference {
            plane.data.fill(128);
        }
        let csources = [Tile::empty(), Tile::empty()];
        for (amplitude, searches) in [(4, 1), (60, 2)] {
            let mut source = Tile::empty();
            for y in 0..16 {
                for x in 0..16 {
                    source.0[Tile::at(x, y)] = if (x + y) % 2 == 0 {
                        128 + amplitude
                    } else {
                        128 - amplitude
                    };
                }
            }
            let decision = Decision {
                source: &source,
                csources: &csources,
                context: MbContext {
                    top: None,
                    left: None,
                    p_slice: true,
                },
                qp: 28,
                pending_skip: 0,
            };
            let search = Search {
                reference: &reference,
                decision: &decision,
                border: Border::default(),
                mx: 0,
                my: 0,
                previous: 0,
                colocated: None,
            };
            let whole = search.propose(Layout::Whole, None, None).unwrap().unwrap();
            assert_eq!(whole.2 > SPLIT_SATD << (28 / 6), searches == 2);
            SEARCHES.with(|count| count.set(0));
            search
                .candidates(&mut None, &mut Error::Coefficient)
                .unwrap();
            assert_eq!(SEARCHES.with(|count| count.get()), searches);
        }
    }

    #[test]
    fn settled_macroblocks_need_twice_the_satd_before_the_split_search() {
        let mut reference = [
            FramePlane::new(16, 16).unwrap(),
            FramePlane::new(8, 8).unwrap(),
            FramePlane::new(8, 8).unwrap(),
        ];
        for plane in &mut reference {
            plane.data.fill(128);
        }
        let csources = [Tile::empty(), Tile::empty()];
        let mut settled_between_bounds = false;
        for qp in [22, 28] {
            for (noise, spike) in (0..8).flat_map(|noise| [0, 4, 8, 12].map(|spike| (noise, spike)))
            {
                let mut state = 99u32;
                let mut source = Tile::empty();
                for y in 0..16 {
                    for x in 0..16 {
                        state ^= state << 13;
                        state ^= state >> 17;
                        state ^= state << 5;
                        let sample = (state % (2 * noise + 1)) as i32 - noise as i32;
                        let peak = if x < 4 && y < 4 && (x + y) % 2 == 0 {
                            spike
                        } else {
                            0
                        };
                        source.0[Tile::at(x, y)] = (128 + sample + peak) as u8;
                    }
                }
                let decision = Decision {
                    source: &source,
                    csources: &csources,
                    context: MbContext {
                        top: None,
                        left: None,
                        p_slice: true,
                    },
                    qp,
                    pending_skip: 0,
                };
                let search = Search {
                    reference: &reference,
                    decision: &decision,
                    border: Border::default(),
                    mx: 0,
                    my: 0,
                    previous: 0,
                    colocated: None,
                };
                let whole = search.propose(Layout::Whole, None, None).unwrap().unwrap();
                SEARCHES.with(|count| count.set(0));
                let scope = search
                    .candidates(&mut None, &mut Error::Coefficient)
                    .unwrap();
                let settled = scope == IntraScope::Sixteen;
                let bound = if settled { 2 * SPLIT_SATD } else { SPLIT_SATD } << (qp / 6);
                let expected = match scope {
                    IntraScope::None => 0,
                    _ => 1 + usize::from(whole.2 > bound),
                };
                assert_eq!(SEARCHES.with(|count| count.get()), expected);
                settled_between_bounds |=
                    settled && whole.2 > SPLIT_SATD << (qp / 6) && whole.2 <= bound;
            }
        }
        assert!(settled_between_bounds);
    }

    #[test]
    fn inter_syntax_floors_preserve_equal_bounds_and_skip_noncompetitive_trials() {
        let mut reference = [
            FramePlane::new(16, 16).unwrap(),
            FramePlane::new(8, 8).unwrap(),
            FramePlane::new(8, 8).unwrap(),
        ];
        for plane in &mut reference {
            plane.data.fill(128);
        }
        let csources = [Tile::empty(), Tile::empty()];
        for (pixels, value, evaluations) in [(1, 129, 1), (1, 168, 2)] {
            let mut source = Tile::empty();
            for x in 0..pixels {
                source.0[Tile::at(x, 0)] = value;
            }
            let decision = Decision {
                source: &source,
                csources: &csources,
                context: MbContext {
                    top: None,
                    left: None,
                    p_slice: true,
                },
                qp: 28,
                pending_skip: 0,
            };
            let search = Search {
                reference: &reference,
                decision: &decision,
                border: Border::default(),
                mx: 0,
                my: 0,
                previous: 0,
                colocated: None,
            };
            EVALUATIONS.with(|count| count.set(0));
            SEARCHES.with(|count| count.set(0));
            PROBES.with(|count| count.set(0));
            let mut best = None;
            let scope = search
                .candidates(&mut best, &mut Error::Coefficient)
                .unwrap();
            assert!(best.unwrap().inter.unwrap().skip);
            assert_eq!(EVALUATIONS.with(|count| count.get()), evaluations);
            let expected = if evaluations == 1 {
                IntraScope::None
            } else {
                IntraScope::Sixteen
            };
            assert_eq!(scope, expected);
            if evaluations == 1 {
                assert_eq!(SEARCHES.with(|count| count.get()), 0);
                assert_eq!(PROBES.with(|count| count.get()), 0);
            } else {
                assert_eq!(SEARCHES.with(|count| count.get()), 1);
                assert!(PROBES.with(|count| count.get()) > 0);
            }
            let floor = search.syntax_floor(3).unwrap();
            assert!(
                search
                    .propose(Layout::Whole, Some(floor - 1), None)
                    .unwrap()
                    .is_none()
            );
            assert!(
                search
                    .propose(Layout::Whole, Some(floor), None)
                    .unwrap()
                    .is_some()
            );
        }
    }

    #[test]
    fn intra_syntax_floor_skips_only_strictly_uncompetitive_construction() {
        let mut reference = [
            FramePlane::new(16, 16).unwrap(),
            FramePlane::new(8, 8).unwrap(),
            FramePlane::new(8, 8).unwrap(),
        ];
        for plane in &mut reference {
            plane.data.fill(128);
        }
        let base = Tile::empty();
        let csources = [base.clone(), base.clone()];
        for pending_skip in [0, 2, 126] {
            let mut prefix = Bits::count();
            prefix.ue(pending_skip).unwrap();
            let mut skipped = Bits::count();
            skipped.ue(pending_skip + 1).unwrap();
            let floor = 512 * 32 * (prefix.len + 6) as u64;
            for equality in [false, true] {
                let mut source = base.clone();
                if equality {
                    let count = (floor - 512 * 32 * skipped.len as u64) / (512 * 64);
                    for x in 0..count as usize {
                        source.0[Tile::at(x, 0)] = 136;
                    }
                } else {
                    source.0[Tile::at(0, 0)] = 129;
                }
                let decision = Decision {
                    source: &source,
                    csources: &csources,
                    context: MbContext {
                        top: None,
                        left: None,
                        p_slice: true,
                    },
                    qp: 28,
                    pending_skip,
                };
                let search = Search {
                    reference: &reference,
                    decision: &decision,
                    border: Border::default(),
                    mx: 0,
                    my: 0,
                    previous: 0,
                    colocated: None,
                };
                let mut skip = Inter::new(Layout::Whole).unwrap();
                skip.skip = true;
                let candidate = search.evaluate(skip.clone()).unwrap();
                assert_eq!(candidate.key.cost == floor, equality);
                let mut expected = Some(candidate);
                consider(
                    &mut expected,
                    decision
                        .quick_intra(
                            &base,
                            &csources,
                            intra_availability(search.border),
                            IntraScope::Quick(None),
                        )
                        .unwrap(),
                );
                let mut best = Some(search.evaluate(skip).unwrap());
                super::super::tests::INTRA_PAIRS.with(|count| count.set(0));
                decision
                    .intra(
                        &base,
                        &csources,
                        intra_availability(search.border),
                        IntraScope::Quick(None),
                        &mut best,
                        &mut Error::Coefficient,
                    )
                    .unwrap();
                assert!(best.unwrap().key == expected.unwrap().key);
                assert_eq!(
                    super::super::tests::INTRA_PAIRS.with(|count| count.get()),
                    usize::from(equality)
                );
            }
        }
    }

    const PART_PROBES: usize = 3 + 5 + 6 * HEXAGON_ROUNDS + 4 + 8;

    #[test]
    fn colocated_and_whole_block_seeds_reach_motion_beyond_the_pattern() {
        let (width, height) = (160, 32);
        let mut reference = [
            FramePlane::new(width, height).unwrap(),
            FramePlane::new(width / 2, height / 2).unwrap(),
            FramePlane::new(width / 2, height / 2).unwrap(),
        ];
        let mut seed = 0x2545_f491u32;
        for sample in &mut reference[0].data {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            *sample = (seed >> 24) as u8;
        }
        let far = Mv { x: 4 * 96, y: 0 };
        let mut source = Tile::empty();
        for y in 0..16 {
            for x in 0..16 {
                source.0[Tile::at(x, y)] = reference[0].data[y * width + 16 + 96 + x];
            }
        }
        let csources = [Tile::empty(), Tile::empty()];
        let decision = Decision {
            source: &source,
            csources: &csources,
            context: MbContext {
                top: None,
                left: None,
                p_slice: true,
            },
            qp: 28,
            pending_skip: 0,
        };
        let moving = MotionMb {
            intra: false,
            vectors: [far; 16],
        };
        let intra = MotionMb::default();
        let chosen = |colocated, layout, seed| {
            let search = Search {
                reference: &reference,
                decision: &decision,
                border: Border::default(),
                mx: 1,
                my: 0,
                previous: 0,
                colocated,
            };
            let (_, inter, _) = search.propose(layout, None, seed).unwrap().unwrap();
            inter.motion().unwrap().vectors
        };
        assert_eq!(chosen(Some(&moving), Layout::Whole, None), [far; 16]);
        assert_ne!(chosen(None, Layout::Whole, None), [far; 16]);
        assert_ne!(chosen(Some(&intra), Layout::Whole, None), [far; 16]);
        assert_eq!(chosen(None, Layout::Sub([0; 4]), Some(far)), [far; 16]);
        assert_ne!(chosen(None, Layout::Sub([0; 4]), None), [far; 16]);
    }

    #[test]
    fn compact_search_bounds_probes_and_complete_inter_evaluations() {
        let reference = [
            FramePlane::new(16, 16).unwrap(),
            FramePlane::new(8, 8).unwrap(),
            FramePlane::new(8, 8).unwrap(),
        ];
        let source = Tile::empty();
        let csources = [Tile::empty(), Tile::empty()];
        let decision = Decision {
            source: &source,
            csources: &csources,
            context: MbContext {
                top: None,
                left: None,
                p_slice: true,
            },
            qp: 28,
            pending_skip: 0,
        };
        for previous in [0, 12, 15, 16] {
            let search = Search {
                reference: &reference,
                decision: &decision,
                border: Border::default(),
                mx: 0,
                my: 0,
                previous,
                colocated: None,
            };
            PROBES.with(|count| count.set(0));
            EVALUATIONS.with(|count| count.set(0));
            let mut best = None;
            search
                .candidates(&mut best, &mut Error::Coefficient)
                .unwrap();
            assert!(PROBES.with(|count| count.get()) <= 5 * PART_PROBES);
            assert!(EVALUATIONS.with(|count| count.get()) <= 3);
            assert_eq!(best.is_some(), previous < 16);
            if let Some(best) = best {
                let inter = best.inter.unwrap();
                assert!(motion::admitted(previous, inter.len));
                assert!(matches!(
                    inter.layout,
                    Layout::Whole | Layout::Sub([0, 0, 0, 0])
                ));
            }
        }
        let zero = Tile([0; STRIDE * 18]);
        let zero_decision = Decision {
            source: &zero,
            ..decision
        };
        let search = Search {
            reference: &reference,
            decision: &zero_decision,
            border: Border::default(),
            mx: 0,
            my: 0,
            previous: 0,
            colocated: None,
        };
        let part = Inter::new(Layout::Whole).unwrap().parts[0];
        for predictor in [
            Mv::default(),
            Mv { x: 3, y: -7 },
            Mv { x: -8192, y: -2048 },
            Mv { x: 8191, y: 2047 },
        ] {
            PROBES.with(|count| count.set(0));
            let selected = search.compact(part, predictor, &[]).unwrap();
            assert_eq!(selected.2, predictor);
            assert!(PROBES.with(|count| count.get()) <= PART_PROBES);
            assert_eq!(search.compact(part, predictor, &[]).unwrap(), selected);
        }
    }

    #[test]
    fn quiet_routes_match_fixed_division_and_coded_zero_levels() {
        let zero = |numerator: i64, divisor: i64, offset: i64| {
            12 * numerator.abs() + divisor * offset < 12 * divisor
        };
        let ac_zero = |block: &t::Forward, qp, offset, separate_dc| {
            block
                .coefficients()
                .iter()
                .enumerate()
                .skip(usize::from(separate_dc))
                .all(|(p, &value)| {
                    let weight =
                        (if p / 4 % 2 == 0 { 4 } else { 5 }) * (if p % 2 == 0 { 4 } else { 5 });
                    zero(
                        64 * i64::from(value),
                        weight * t::norm(qp, p).unwrap() * (1i64 << (qp / 6)),
                        offset,
                    )
                })
        };
        let mut reference = [
            FramePlane::new(16, 16).unwrap(),
            FramePlane::new(8, 8).unwrap(),
            FramePlane::new(8, 8).unwrap(),
        ];
        for plane in &mut reference {
            plane.data.fill(128);
        }
        let base = Tile::empty();
        let cbases = [base.clone(), base.clone()];
        let available = IntraAvail {
            top: false,
            left: false,
            top_left: false,
            top_right: false,
        };
        let h = [[1, 1, 1, 1], [1, 1, -1, -1], [1, -1, -1, 1], [1, -1, 1, -1]];
        for qp in 0..=51 {
            for plane in 0..3 {
                for pattern in 0..6 {
                    for amplitude in 0..=32i32 {
                        let mut source = base.clone();
                        let mut csources = cbases.clone();
                        let n = if plane == 0 { 16 } else { 8 };
                        let changed = if plane == 0 {
                            &mut source
                        } else {
                            &mut csources[plane - 1]
                        };
                        for y in 0..n {
                            for x in 0..n {
                                let delta = match pattern {
                                    0 => amplitude,
                                    1 => {
                                        if (x / 4 + y / 4) % 2 == 0 {
                                            amplitude
                                        } else {
                                            -amplitude
                                        }
                                    }
                                    2 => {
                                        if x % 4 < 2 {
                                            amplitude
                                        } else {
                                            -amplitude
                                        }
                                    }
                                    3 => {
                                        if (x + y) % 2 == 0 {
                                            amplitude
                                        } else {
                                            -amplitude
                                        }
                                    }
                                    4 => {
                                        if x % 4 == 0 && y % 4 == 0 {
                                            amplitude
                                        } else {
                                            0
                                        }
                                    }
                                    _ => {
                                        if x < 4 && y < 4 {
                                            amplitude
                                        } else {
                                            0
                                        }
                                    }
                                };
                                changed.0[Tile::at(x, y)] = (128 + delta) as u8;
                            }
                        }
                        let mut inter_y = true;
                        let mut intra_four = true;
                        let mut intra_ac = true;
                        let mut sums = [0i64; 16];
                        for k in 0..16 {
                            let (x, y) = xy(k);
                            let block = base.forward(&source, x, y).unwrap();
                            sums[y / 4 * 4 + x / 4] = i64::from(block.dc());
                            inter_y &= ac_zero(&block, qp, 2, false);
                            intra_four &= ac_zero(&block, qp, 5, false);
                            intra_ac &= ac_zero(&block, qp, 5, true);
                        }
                        let step = t::norm(qp, 0).unwrap() << (qp / 6);
                        let mut intra_dc = true;
                        for row in &h {
                            for column in &h {
                                let mut dc = 0;
                                for y in 0..4 {
                                    for x in 0..4 {
                                        dc += row[y] * sums[y * 4 + x] * column[x];
                                    }
                                }
                                intra_dc &= zero(dc, step, 5);
                            }
                        }
                        let cqp = t::chroma_qp(qp);
                        let step = t::norm(cqp, 0).unwrap() << (cqp / 6);
                        let mut chroma_quiet = true;
                        for csource in &csources {
                            let mut sums = [0; 4];
                            for (k, sum) in sums.iter_mut().enumerate() {
                                let block = base.forward(csource, k % 2 * 4, k / 2 * 4).unwrap();
                                *sum = i64::from(block.dc());
                                chroma_quiet &= ac_zero(&block, cqp, 5, true);
                            }
                            let [a, b, c, d] = sums;
                            chroma_quiet &=
                                [a + b + c + d, a - b + c - d, a + b - c - d, a - b - c + d]
                                    .iter()
                                    .all(|&dc| zero(2 * dc, step, 5));
                        }
                        let decision = Decision {
                            source: &source,
                            csources: &csources,
                            context: MbContext {
                                top: None,
                                left: None,
                                p_slice: true,
                            },
                            qp,
                            pending_skip: 7,
                        };
                        let search = Search {
                            reference: &reference,
                            decision: &decision,
                            border: Border::default(),
                            mx: 0,
                            my: 0,
                            previous: 0,
                            colocated: None,
                        };
                        let mut skip = Inter::new(Layout::Whole).unwrap();
                        skip.skip = true;
                        let skip = search.evaluate(skip).unwrap();
                        let quiet = search.quiet(&skip).unwrap();
                        assert_eq!(
                            quiet,
                            inter_y && chroma_quiet,
                            "QP {qp}, plane {plane}, pattern {pattern}, amplitude {amplitude}"
                        );
                        if quiet {
                            let coded =
                                search.evaluate(Inter::new(Layout::Whole).unwrap()).unwrap();
                            assert!(coded.luma.levels.iter().flatten().all(|&v| v == 0));
                            assert!(
                                coded
                                    .chroma
                                    .levels
                                    .iter()
                                    .flatten()
                                    .flatten()
                                    .all(|&v| v == 0)
                            );
                            assert!(coded.chroma.dc.iter().flatten().all(|&v| v == 0));
                        }
                        if intra_four {
                            let coded = intra4(
                                &base,
                                &source,
                                available,
                                qp,
                                None,
                                None,
                                Trials::Shortlist,
                            )
                            .unwrap()
                            .1;
                            assert!(coded.levels.iter().flatten().all(|&v| v == 0));
                        }
                        if intra_ac && intra_dc {
                            let coded = intra16(&base, &source, available, 2, qp).unwrap();
                            assert!(coded.levels.iter().flatten().all(|&v| v == 0));
                            assert!(coded.dc.iter().all(|&v| v == 0));
                        }
                        if chroma_quiet {
                            let coded = chroma(&cbases, &csources, available, 0, cqp).unwrap();
                            assert!(coded.levels.iter().flatten().flatten().all(|&v| v == 0));
                            assert!(coded.dc.iter().flatten().all(|&v| v == 0));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn quiet_skip_avoids_search_only_when_every_component_quantizes_to_zero() {
        let reference = [
            FramePlane::new(16, 16).unwrap(),
            FramePlane::new(8, 8).unwrap(),
            FramePlane::new(8, 8).unwrap(),
        ];
        for qp in [10, 16, 28, 40, 51] {
            for changed in 0..7 {
                let mut source = Tile([0; STRIDE * 18]);
                let mut csources = [Tile([0; STRIDE * 18]), Tile([0; STRIDE * 18])];
                let (plane, value, size) = if changed < 4 {
                    (changed, 1, 1)
                } else {
                    (changed - 3, 255, 4)
                };
                for y in 0..size {
                    for x in 0..size {
                        let at = Tile::at(3 + x, 3 + y);
                        match plane {
                            0 => {}
                            1 => source.0[at] = value,
                            _ => csources[plane - 2].0[at] = value,
                        }
                    }
                }
                let decision = Decision {
                    source: &source,
                    csources: &csources,
                    context: MbContext {
                        top: None,
                        left: None,
                        p_slice: true,
                    },
                    qp,
                    pending_skip: 7,
                };
                let search = Search {
                    reference: &reference,
                    decision: &decision,
                    border: Border::default(),
                    mx: 0,
                    my: 0,
                    previous: 0,
                    colocated: None,
                };
                let mut best = None;
                SEARCHES.with(|count| count.set(0));
                let scope = search
                    .candidates(&mut best, &mut Error::Coefficient)
                    .unwrap();
                let count = SEARCHES.with(|count| count.get());
                assert_eq!(count == 0, changed < 4, "{qp} {changed}");
                assert_eq!(scope == IntraScope::None, changed < 4);
                if changed < 4 {
                    assert!(best.as_ref().unwrap().inter.as_ref().unwrap().skip);
                }
                if changed == 0 {
                    let best = best.unwrap();
                    assert_eq!(best.luma.tile.error(&source, 0, 0, 16).unwrap(), 0);
                    for (actual, expected) in best.chroma.tiles.iter().zip(&csources) {
                        assert_eq!(actual.error(expected, 0, 0, 8).unwrap(), 0);
                    }
                }
            }
        }
    }

    #[test]
    fn direct_reference_regions_match_sampled_windows_at_every_phase_and_edge() {
        let mut reference = FramePlane::new(48, 48).unwrap();
        for (p, v) in reference.data.iter_mut().enumerate() {
            *v = ((p * 197 + p / 48 * 31) % 256) as u8;
        }
        for chroma in [false, true] {
            let (scale, divisor, margin, extra) = if chroma { (2, 8, 0, 1) } else { (1, 4, 2, 5) };
            for size in [4, 8, 16] {
                for origin in [0, 1, 2] {
                    for base in [-64, -8, 0, 8, 64] {
                        for xf in 0..divisor {
                            for yf in 0..divisor {
                                let part = Part {
                                    width: size,
                                    height: size,
                                    mv: Mv {
                                        x: base + xf,
                                        y: -base + yf,
                                    },
                                    ..Part::default()
                                };
                                let mut actual = Tile::empty();
                                predict(&reference, &mut actual, part, origin, origin, chroma)
                                    .unwrap();
                                let (ix, iy) = (
                                    (origin * 16 / scale) as i32 + part.mv.x.div_euclid(divisor)
                                        - margin,
                                    (origin * 16 / scale) as i32 + part.mv.y.div_euclid(divisor)
                                        - margin,
                                );
                                let n = size / scale;
                                let mut window = [0; 21 * 21];
                                for y in 0..n + extra {
                                    for x in 0..n + extra {
                                        window[y * 21 + x] =
                                            reference.sample(ix + x as i32, iy + y as i32).unwrap();
                                    }
                                }
                                let mut expected = Tile::empty();
                                let src = Block::new(&window, 0, 21, n + extra, n + extra).unwrap();
                                let dst =
                                    BlockMut::new(&mut expected.0, Tile::at(0, 0), STRIDE, n, n)
                                        .unwrap();
                                if chroma {
                                    Scalar.chroma_mc(dst, src, xf as u8, yf as u8);
                                } else {
                                    Scalar.luma_mc(dst, src, xf as u8, yf as u8);
                                }
                                assert_eq!(actual.0, expected.0);
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn integer_scores_match_sampled_sad_for_rectangles_and_outlying_seeds() {
        let mut reference = [
            FramePlane::new(48, 48).unwrap(),
            FramePlane::new(24, 24).unwrap(),
            FramePlane::new(24, 24).unwrap(),
        ];
        for (p, v) in reference[0].data.iter_mut().enumerate() {
            *v = ((p * 197 + p / 48 * 31) % 256) as u8;
        }
        let mut source = Tile::empty();
        for (p, v) in source.0.iter_mut().enumerate() {
            *v = (p % 256) as u8;
        }
        let csources = [Tile::empty(), Tile::empty()];
        let decision = Decision {
            source: &source,
            csources: &csources,
            context: MbContext {
                top: None,
                left: None,
                p_slice: true,
            },
            qp: 28,
            pending_skip: 0,
        };
        for (width, height) in [(4, 4), (4, 8), (8, 4), (16, 8), (8, 16), (16, 16)] {
            let part = Part {
                width,
                height,
                ..Part::default()
            };
            for origin in [0, 1, 2] {
                let search = Search {
                    reference: &reference,
                    decision: &decision,
                    border: Border::default(),
                    mx: origin,
                    my: origin,
                    previous: 0,
                    colocated: None,
                };
                for x in [-8192, -64, -4, 0, 4, 64, 8188] {
                    for y in [-2048, -64, -4, 0, 4, 64, 2044] {
                        let mv = Mv { x, y };
                        let predictor = Mv { x: -3, y: 7 };
                        let mut bits = Bits::count();
                        mv.difference(predictor).unwrap().write(&mut bits).unwrap();
                        let mut sad = 0u64;
                        for j in 0..height {
                            for i in 0..width {
                                let b = reference[0]
                                    .sample(
                                        (origin * 16 + i) as i32 + x / 4,
                                        (origin * 16 + j) as i32 + y / 4,
                                    )
                                    .unwrap();
                                sad += u64::from(source.0[Tile::at(i, j)].abs_diff(b));
                            }
                        }
                        assert_eq!(
                            search
                                .score(part, mv, predictor, &mut Motions::new())
                                .unwrap(),
                            Some((sad + (1 << (28 / 6)) * bits.len as u64, bits.len, mv))
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn row_cutoffs_preserve_complete_scores_rankings_and_integer_centres() {
        let mut rejected = 0;
        for textured in [false, true] {
            let mut reference = [
                FramePlane::new(32, 32).unwrap(),
                FramePlane::new(16, 16).unwrap(),
                FramePlane::new(16, 16).unwrap(),
            ];
            let mut source = Tile::empty();
            let csources = [Tile::empty(), Tile::empty()];
            if textured {
                for (p, value) in reference[0].data.iter_mut().enumerate() {
                    *value = ((p * 71 + p / 32 * 19) % 256) as u8;
                }
                for y in 0..16 {
                    for x in 0..16 {
                        source.0[Tile::at(x, y)] = ((x * 17 + y * 43) % 256) as u8;
                    }
                }
            }
            for qp in [10, 16, 28, 40, 51] {
                let decision = Decision {
                    source: &source,
                    csources: &csources,
                    context: MbContext {
                        top: None,
                        left: None,
                        p_slice: true,
                    },
                    qp,
                    pending_skip: 0,
                };
                for origin in [0, 1] {
                    let search = Search {
                        reference: &reference,
                        decision: &decision,
                        border: Border::default(),
                        mx: origin,
                        my: origin,
                        previous: 0,
                        colocated: None,
                    };
                    for (width, height) in
                        [(16, 16), (16, 8), (8, 16), (8, 8), (8, 4), (4, 8), (4, 4)]
                    {
                        let part = Part {
                            width,
                            height,
                            ..Part::default()
                        };
                        for predictor in
                            [Mv::default(), Mv { x: 3, y: -7 }, Mv { x: 8191, y: 2047 }]
                        {
                            let mut probes = vec![Mv::default(), predictor];
                            for x in [-8, -4, 0, 4, 8] {
                                for y in [-8, -4, 0, 4, 8] {
                                    probes.push(Mv { x, y });
                                }
                            }
                            for x in -3..=3 {
                                for y in -3..=3 {
                                    probes.push(Mv { x, y });
                                }
                            }
                            probes.extend([Mv { x: -8192, y: -2048 }, Mv { x: 8191, y: 2047 }]);
                            let mut actual = Motions::new();
                            let mut expected = Motions::new();
                            for &mv in probes.iter().chain(probes.iter().rev()) {
                                let complete = search
                                    .score(part, mv, predictor, &mut Motions::new())
                                    .unwrap()
                                    .unwrap();
                                let bounded =
                                    search.score(part, mv, predictor, &mut actual).unwrap();
                                if !expected.seen.iter().flatten().any(|value| value.2 == mv) {
                                    expected.insert(complete).unwrap();
                                }
                                if let Some(value) = bounded {
                                    assert_eq!(value, complete);
                                } else {
                                    rejected += 1;
                                    assert!(complete.0 > expected.ranked[3].unwrap().0);
                                    assert!(complete.0 > expected.integer.unwrap().0);
                                }
                                assert_eq!(actual.ranked, expected.ranked);
                                assert_eq!(actual.integer, expected.integer);
                            }
                            assert_eq!(
                                actual.finalists(predictor).unwrap(),
                                expected.finalists(predictor).unwrap()
                            );
                        }
                    }
                }
            }
        }
        assert!(rejected > 0);
    }

    #[test]
    fn score_cutoffs_protect_integer_centres_and_equal_cost_ties() {
        let reference = [
            FramePlane::new(16, 16).unwrap(),
            FramePlane::new(8, 8).unwrap(),
            FramePlane::new(8, 8).unwrap(),
        ];
        let source = Tile([0; STRIDE * 18]);
        let csources = [Tile::empty(), Tile::empty()];
        let decision = Decision {
            source: &source,
            csources: &csources,
            context: MbContext {
                top: None,
                left: None,
                p_slice: true,
            },
            qp: 10,
            pending_skip: 0,
        };
        let search = Search {
            reference: &reference,
            decision: &decision,
            border: Border::default(),
            mx: 0,
            my: 0,
            previous: 0,
            colocated: None,
        };
        let part = Inter::new(Layout::Whole).unwrap().parts[0];
        let mut motions = Motions::new();
        motions.insert((200, 2, Mv::default())).unwrap();
        for x in 1..=4 {
            motions.insert((x as u64, 2, Mv { x, y: 1 })).unwrap();
        }
        let candidate = search
            .score(part, Mv { x: 4, y: 0 }, Mv::default(), &mut motions)
            .unwrap()
            .unwrap();
        assert_eq!(motions.integer, Some(candidate));
        assert!(candidate.0 > motions.ranked[3].unwrap().0);

        let mut motions = Motions::new();
        motions.insert((8, 2, Mv::default())).unwrap();
        for y in 1..=3 {
            motions.insert((16, 8, Mv { x: 3, y })).unwrap();
        }
        let mv = Mv { x: -4, y: 0 };
        assert_eq!(
            search.score(part, mv, Mv::default(), &mut motions).unwrap(),
            Some((16, 8, mv))
        );
        assert!(motions.ranked.contains(&Some((16, 8, mv))));
    }

    fn per_row_scores(
        source: &[u8],
        predicted: &[u8],
        stride: usize,
        width: usize,
        motion_cost: u64,
        cutoff: Option<u64>,
    ) -> Result<Option<u64>> {
        let mut score = motion_cost;
        for (original, predicted) in source.chunks(STRIDE).zip(predicted.chunks(stride)) {
            score += crate::kernels::sad(
                original.get(..width).ok_or(Error::Internal)?,
                predicted.get(..width).ok_or(Error::Internal)?,
            )?;
            if cutoff.is_some_and(|limit| score > limit) {
                return Ok(None);
            }
        }
        Ok(Some(score))
    }

    #[test]
    fn block_sad_dispatch_matches_per_row_scores_cutoffs_and_failures() {
        let mut seed = 0x5ad0_d15b_a7c4_e001u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut compared = 0;
        for width in 0..=20 {
            for height in 1..=16 {
                for stride in [width.max(1), 16, 21, STRIDE, 64] {
                    let extremes = next() % 4 == 0;
                    let mut sample = || {
                        let value = next();
                        if extremes {
                            [0, 255][value as usize % 2]
                        } else {
                            value as u8
                        }
                    };
                    let source: Vec<u8> = (0..(height - 1) * STRIDE + width.max(1))
                        .map(|_| sample())
                        .collect();
                    let predicted: Vec<u8> = (0..(height - 1) * stride + width.max(1) + 3)
                        .map(|_| sample())
                        .collect();
                    let motion_cost = next() % 512;
                    let mut prefixes = vec![motion_cost];
                    if width <= 16 {
                        for (row, original) in source.chunks(STRIDE).enumerate() {
                            let start = row * stride;
                            let total = original
                                .iter()
                                .take(width)
                                .zip(&predicted[start..start + width])
                                .map(|(&a, &b)| u64::from(a.abs_diff(b)))
                                .sum::<u64>();
                            prefixes.push(prefixes.last().unwrap() + total);
                        }
                        assert_eq!(
                            score_rows(&source, &predicted, stride, width, motion_cost, None),
                            Ok(Some(*prefixes.last().unwrap()))
                        );
                    }
                    let mut cutoffs = vec![None, Some(0), Some(u64::MAX)];
                    for &prefix in &prefixes {
                        cutoffs.extend([prefix.checked_sub(1), Some(prefix), Some(prefix + 1)]);
                    }
                    let short = (height - 1) * stride + width.max(1) - 1;
                    for cutoff in cutoffs {
                        for (source, predicted) in [
                            (&source[..], &predicted[..]),
                            (&source[..source.len() - 1], &predicted[..]),
                            (&source[..], &predicted[..short]),
                        ] {
                            assert_eq!(
                                score_rows(source, predicted, stride, width, motion_cost, cutoff),
                                per_row_scores(
                                    source,
                                    predicted,
                                    stride,
                                    width,
                                    motion_cost,
                                    cutoff
                                ),
                                "width {width} height {height} stride {stride} cutoff {cutoff:?}"
                            );
                            compared += 1;
                        }
                    }
                }
            }
        }
        assert!(compared > 50_000);
    }

    #[test]
    fn block_sad_keeps_row_cutoffs_equal_score_ties_and_admission() {
        let source = [10u8; STRIDE * 3 + 16];
        let mut predicted = [0u8; 16 * 4];
        let rows = |predicted: &[u8], cutoff| score_rows(&source, predicted, 16, 16, 5, cutoff);
        assert_eq!(rows(&predicted, None), Ok(Some(645)));
        assert_eq!(rows(&predicted, Some(645)), Ok(Some(645)));
        assert_eq!(rows(&predicted, Some(644)), Ok(None));
        assert_eq!(rows(&predicted, Some(165)), Ok(None));
        assert_eq!(rows(&predicted, Some(4)), Ok(None));
        assert_eq!(rows(&predicted[..15], Some(0)), Err(Error::Internal));
        predicted[16 * 3..].fill(10);
        assert_eq!(rows(&predicted, Some(485)), Ok(Some(485)));
        assert_eq!(rows(&predicted, Some(484)), Ok(None));
        assert_eq!(
            score_rows(&source, &predicted, 16, 17, 5, None),
            Err(Error::Internal)
        );
        assert_eq!(score_rows(&[], &[], 16, 16, 7, Some(0)), Ok(Some(7)));
    }

    #[test]
    fn cheap_finalists_keep_protected_seeds_and_deterministic_ties() {
        let mut motions = Motions::new();
        let predictor = Mv { x: 80, y: 0 };
        for (cost, mv) in [
            (100, Mv::default()),
            (90, predictor),
            (10, Mv { x: 4, y: 0 }),
            (10, Mv { x: 0, y: 4 }),
            (10, Mv { x: -4, y: 0 }),
            (10, Mv { x: 0, y: -4 }),
        ] {
            motions.insert((cost, 2, mv)).unwrap();
        }
        assert_eq!(
            motions.ranked.map(|value| value.unwrap().2),
            [
                Mv { x: -4, y: 0 },
                Mv { x: 0, y: -4 },
                Mv { x: 0, y: 4 },
                Mv { x: 4, y: 0 },
            ]
        );
        let finalists = motions.finalists(predictor).unwrap();
        assert_eq!(finalists.iter().flatten().count(), 6);
        assert_eq!(finalists[4], Some(Mv::default()));
        assert_eq!(finalists[5], Some(predictor));
    }

    #[test]
    fn cheap_probes_are_bounded_distinct_and_preserve_motion_endpoints() {
        let reference = [
            FramePlane::new(16, 16).unwrap(),
            FramePlane::new(8, 8).unwrap(),
            FramePlane::new(8, 8).unwrap(),
        ];
        let source = Tile::empty();
        let csources = [Tile::empty(), Tile::empty()];
        let decision = Decision {
            source: &source,
            csources: &csources,
            context: MbContext {
                top: None,
                left: None,
                p_slice: true,
            },
            qp: 28,
            pending_skip: 0,
        };
        let search = Search {
            reference: &reference,
            decision: &decision,
            border: Border::default(),
            mx: 0,
            my: 0,
            previous: 0,
            colocated: None,
        };
        let part = Inter::new(Layout::Whole).unwrap().parts[0];
        for predictor in [
            Mv::default(),
            Mv { x: 72, y: -80 },
            Mv { x: 3, y: -7 },
            Mv { x: -8192, y: -2048 },
            Mv { x: 8191, y: 2047 },
        ] {
            let motions = search.motions(part, predictor).unwrap();
            assert!(motions.len <= 99);
            let vectors: Vec<_> = motions.seen.iter().flatten().map(|value| value.2).collect();
            let distinct: std::collections::BTreeSet<_> = vectors.iter().copied().collect();
            assert_eq!(vectors.len(), distinct.len());
            assert!(vectors.iter().all(|mv| mv.valid()));
            let finalists = motions.finalists(predictor).unwrap();
            assert!(finalists.iter().flatten().count() <= 6);
            assert!(finalists.contains(&Some(Mv::default())));
            assert!(finalists.contains(&Some(predictor)));
            assert_eq!(
                motions.ranked,
                search.motions(part, predictor).unwrap().ranked
            );
        }
    }

    #[test]
    fn cached_finalists_match_complete_trials_for_all_partition_shapes() {
        let mut reference = [
            FramePlane::new(32, 32).unwrap(),
            FramePlane::new(16, 16).unwrap(),
            FramePlane::new(16, 16).unwrap(),
        ];
        for (c, plane) in reference.iter_mut().enumerate() {
            for (p, value) in plane.data.iter_mut().enumerate() {
                *value = ((p * 37 + p / 7 * 11 + c * 29) % 256) as u8;
            }
        }
        let mut source = Tile::empty();
        let mut csources = [Tile::empty(), Tile::empty()];
        for y in 0..16 {
            for x in 0..16 {
                source.0[Tile::at(x, y)] = ((x * 13 + y * 43) % 256) as u8;
            }
        }
        for (c, tile) in csources.iter_mut().enumerate() {
            for y in 0..8 {
                for x in 0..8 {
                    tile.0[Tile::at(x, y)] = ((x * 41 + y * 17 + c * 67) % 256) as u8;
                }
            }
        }
        let context = MbContext {
            top: None,
            left: None,
            p_slice: true,
        };
        for qp in [10, 16, 28, 40, 51] {
            let decision = Decision {
                source: &source,
                csources: &csources,
                context,
                qp,
                pending_skip: 7,
            };
            for corner in [0, 1] {
                let search = Search {
                    reference: &reference,
                    decision: &decision,
                    border: Border::default(),
                    mx: corner,
                    my: corner,
                    previous: 0,
                    colocated: None,
                };
                for layout in [
                    Layout::Whole,
                    Layout::Horizontal,
                    Layout::Vertical,
                    Layout::Sub([0; 4]),
                    Layout::Sub([1; 4]),
                    Layout::Sub([2; 4]),
                    Layout::Sub([3; 4]),
                    Layout::Sub([3, 2, 1, 0]),
                ] {
                    let mut base = Inter::new(layout).unwrap();
                    for (i, part) in base.parts.iter_mut().take(base.len).enumerate() {
                        part.mv = Mv {
                            x: i as i32 % 7 - 3,
                            y: i as i32 % 5 - 2,
                        };
                    }
                    for index in 0..base.len {
                        let prepared = search.prepare(&base, index).unwrap();
                        for mv in [Mv::default(), Mv { x: -7, y: 5 }, Mv { x: 8191, y: -2048 }] {
                            let mut trial = base.clone();
                            trial.parts[index].mv = mv;
                            let complete = search.evaluate(trial.clone());
                            let cached = search.evaluate_part(trial, index, &prepared);
                            match (complete, cached) {
                                (Ok(complete), Ok(cached)) => {
                                    assert!(complete.key == cached.key);
                                    assert_eq!(complete.luma.tile.0, cached.luma.tile.0);
                                    assert_eq!(complete.luma.levels, cached.luma.levels);
                                    assert_eq!(complete.chroma.dc, cached.chroma.dc);
                                    assert_eq!(complete.chroma.levels, cached.chroma.levels);
                                    for (a, b) in
                                        complete.chroma.tiles.iter().zip(&cached.chroma.tiles)
                                    {
                                        assert_eq!(a.0, b.0);
                                    }
                                    let mut a = Bits::new(4096);
                                    let mut b = Bits::new(4096);
                                    for (bits, candidate) in
                                        [(&mut a, &complete), (&mut b, &cached)]
                                    {
                                        macroblock(
                                            bits,
                                            &candidate.luma,
                                            &candidate.chroma,
                                            candidate.inter.as_ref(),
                                            context,
                                        )
                                        .unwrap();
                                    }
                                    assert_eq!(a.nal(0x41).unwrap(), b.nal(0x41).unwrap());
                                }
                                (Err(a), Err(b)) => assert_eq!(a, b),
                                _ => panic!("cached trial differs from complete evaluation"),
                            }
                        }
                    }
                }
            }
        }
    }

    fn settings(deblocking: Deblocking) -> Settings {
        Settings {
            frame_rate: Default::default(),
            sample_aspect_ratio: Default::default(),
            width: 16,
            height: 16,
            rate_mode: crate::RateMode::FixedQp(28),
            deblocking,
            gop: Gop::Fixed { idr_interval: 32 },
            colour: Colour {
                full_range: false,
                primaries: 1,
                transfer: 1,
                matrix: 1,
                chroma_location: 0,
            },
        }
    }

    struct Reader {
        bytes: Vec<u8>,
        bit: usize,
    }
    impl Reader {
        fn new(nal: &[u8]) -> Self {
            let mut bytes = Vec::new();
            let mut zeros = 0;
            for &b in &nal[1..] {
                if zeros == 2 && b == 3 {
                    zeros = 0;
                    continue;
                }
                bytes.push(b);
                zeros = if b == 0 { zeros + 1 } else { 0 };
            }
            Self { bytes, bit: 0 }
        }
        fn u(&mut self, n: usize) -> u32 {
            let mut value = 0;
            for _ in 0..n {
                value = value * 2 + u32::from((self.bytes[self.bit / 8] >> (7 - self.bit % 8)) & 1);
                self.bit += 1;
            }
            value
        }
        fn ue(&mut self) -> u32 {
            let mut n = 0;
            while self.u(1) == 0 {
                n += 1;
                assert!(n < 32);
            }
            (1 << n) - 1 + self.u(n)
        }
        fn se(&mut self) -> i32 {
            let v = self.ue();
            if v % 2 == 0 {
                -(v as i32 / 2)
            } else {
                (v as i32 + 1) / 2
            }
        }
    }

    #[test]
    fn every_luma_and_chroma_phase_uses_euclidean_negative_coordinates() {
        let mut reference = FramePlane::new(16, 16).unwrap();
        for (p, v) in reference.data.iter_mut().enumerate() {
            *v = (32 + p % 16 * 4 + p / 16 * 8) as u8;
        }
        for xf in 0..4 {
            for yf in 0..4 {
                let mut tile = Tile::empty();
                let part = Part {
                    x: 4,
                    y: 4,
                    width: 4,
                    height: 4,
                    mv: Mv {
                        x: -4 + xf,
                        y: -4 + yf,
                    },
                };
                predict(&reference, &mut tile, part, 0, 0, false).unwrap();
                for j in 0..4 {
                    for i in 0..4 {
                        assert_eq!(
                            tile.0[Tile::at(4 + i, 4 + j)],
                            (32 + (3 + i) * 4 + (3 + j) * 8 + xf as usize + yf as usize * 2) as u8
                        );
                    }
                }
            }
        }
        for (p, v) in reference.data.iter_mut().enumerate() {
            *v = (p % 16 * 8 + p / 16 * 8) as u8;
        }
        for xf in 0..8 {
            for yf in 0..8 {
                let mut tile = Tile::empty();
                let part = Part {
                    x: 4,
                    y: 4,
                    width: 4,
                    height: 4,
                    mv: Mv {
                        x: -8 + xf,
                        y: -8 + yf,
                    },
                };
                predict(&reference, &mut tile, part, 0, 0, true).unwrap();
                for j in 0..2 {
                    for i in 0..2 {
                        assert_eq!(
                            tile.0[Tile::at(2 + i, 2 + j)],
                            ((1 + i) * 8 + (1 + j) * 8 + xf as usize + yf as usize) as u8
                        );
                    }
                }
            }
        }
        for chroma in [false, true] {
            for mv in [Mv { x: -8192, y: -2048 }, Mv { x: 8191, y: 2047 }] {
                let mut tile = Tile::empty();
                predict(
                    &reference,
                    &mut tile,
                    Part {
                        width: 16,
                        height: 16,
                        mv,
                        ..Part::default()
                    },
                    0,
                    0,
                    chroma,
                )
                .unwrap();
                let n = if chroma { 8 } else { 16 };
                let corner = if mv.x < 0 {
                    reference.data[0]
                } else {
                    reference.data[255]
                };
                for y in 0..n {
                    for x in 0..n {
                        assert_eq!(tile.0[Tile::at(x, y)], corner);
                    }
                }
            }
        }
    }

    #[test]
    fn explicit_syntax_fixtures_decode_every_partition_and_subpartition() {
        let export = std::env::var_os("H264_P_FIXTURE_DIR").map(std::path::PathBuf::from);
        if let Some(dir) = &export {
            std::fs::create_dir_all(dir).unwrap();
        }
        let mut manifest = String::from("layout\tqp\tdeblocking\n");
        let mut exported = 0;
        let layouts = [
            Layout::Whole,
            Layout::Horizontal,
            Layout::Vertical,
            Layout::Sub([0; 4]),
            Layout::Sub([1; 4]),
            Layout::Sub([2; 4]),
            Layout::Sub([3; 4]),
            Layout::Sub([0, 1, 2, 3]),
        ];
        for qp in [10, 16, 28, 40, 51] {
            for deblocking in [Deblocking::Disabled, Deblocking::Enabled] {
                for (layout_index, layout) in layouts.into_iter().enumerate() {
                    let mut s = settings(deblocking);
                    s.rate_mode = crate::RateMode::FixedQp(qp);
                    let mut encoder = Encoder::new(s, Limits::default()).unwrap();
                    let y: Vec<_> = (0..256)
                        .map(|p| (32 + p % 16 * 5 + p / 16 * 3) as u8)
                        .collect();
                    let cb: Vec<_> = (0..64)
                        .map(|p| (64 + p % 8 * 9 + p / 8 * 5) as u8)
                        .collect();
                    let cr: Vec<_> = (0..64)
                        .map(|p| (192 - p % 8 * 7 - p / 8 * 3) as u8)
                        .collect();
                    let picture = Picture {
                        y: Plane::new(&y, 16, 16, 16).unwrap(),
                        cb: Plane::new(&cb, 8, 8, 8).unwrap(),
                        cr: Plane::new(&cr, 8, 8, 8).unwrap(),
                    };
                    let first = encoder.encode(picture).unwrap();
                    let reference = encoder.reference.as_ref().unwrap();
                    let (_, source) = reference[0].tiles(picture.y, 0, 0, 16).unwrap();
                    let csources = [
                        reference[1].tiles(picture.cb, 0, 0, 8).unwrap().1,
                        reference[2].tiles(picture.cr, 0, 0, 8).unwrap().1,
                    ];
                    let context = MbContext {
                        top: None,
                        left: None,
                        p_slice: true,
                    };
                    let decision = Decision {
                        source: &source,
                        csources: &csources,
                        context,
                        qp,
                        pending_skip: 0,
                    };
                    let search = Search {
                        reference,
                        decision: &decision,
                        border: Border::default(),
                        mx: 0,
                        my: 0,
                        previous: 0,
                        colocated: None,
                    };
                    let mut inter = Inter::new(layout).unwrap();
                    for (i, p) in inter.parts.iter_mut().take(inter.len).enumerate() {
                        p.mv = Mv {
                            x: (i % 3) as i32 * 4 - 3,
                            y: (i % 5) as i32 * 2 - 3,
                        };
                    }
                    let candidate = search.evaluate(inter).unwrap();
                    let mut writer = Bits::new(100_000);
                    syntax::slice(
                        &mut writer,
                        &encoder,
                        syntax::Slice {
                            idr: false,
                            id: 0,
                            frame_num: 1,
                            qp,
                        },
                    )
                    .unwrap();
                    let header = writer.len;
                    writer.ue(0).unwrap();
                    let state = macroblock(
                        &mut writer,
                        &candidate.luma,
                        &candidate.chroma,
                        candidate.inter.as_ref(),
                        context,
                    )
                    .unwrap();
                    assert_eq!(writer.len - header, candidate.key.bits);
                    let nal = writer.nal(0x61).unwrap();
                    let mut reader = Reader::new(&nal);
                    assert_eq!(reader.ue(), 0);
                    assert_eq!(reader.ue(), 5);
                    assert_eq!(reader.ue(), 0);
                    assert_eq!(reader.u(4), 1);
                    assert_eq!(reader.u(3), 0);
                    assert_eq!(reader.se(), i32::from(qp) - 26);
                    let filter = reader.ue();
                    assert_eq!(filter, u32::from(deblocking == Deblocking::Disabled));
                    if filter == 0 {
                        assert_eq!(reader.se(), 0);
                        assert_eq!(reader.se(), 0);
                    }
                    assert_eq!(reader.ue(), 0);
                    let code = reader.ue();
                    assert_eq!(code, layout.code());
                    let count = if code == 3 {
                        let mut count = 0;
                        let Layout::Sub(shapes) = layout else {
                            unreachable!()
                        };
                        for expected in shapes {
                            let shape = reader.ue();
                            assert_eq!(shape, u32::from(expected));
                            count += match shape {
                                0 => 1,
                                1 | 2 => 2,
                                3 => 4,
                                _ => unreachable!(),
                            };
                        }
                        count
                    } else if code == 0 {
                        1
                    } else {
                        2
                    };
                    let inter = candidate.inter.as_ref().unwrap();
                    assert_eq!(count, inter.len);
                    for d in inter.differences.iter().take(count) {
                        assert_eq!(reader.se(), d.x);
                        assert_eq!(reader.se(), d.y);
                    }
                    let mut planes = [
                        FramePlane::new(16, 16).unwrap(),
                        FramePlane::new(8, 8).unwrap(),
                        FramePlane::new(8, 8).unwrap(),
                    ];
                    planes[0].commit(&candidate.luma.tile, 0, 0, 16).unwrap();
                    for (c, tile) in candidate.chroma.tiles.iter().enumerate() {
                        planes[c + 1].commit(tile, 0, 0, 8).unwrap();
                    }
                    let nonzero = state.counts[0]
                        .iter()
                        .enumerate()
                        .fold(0, |v, (p, &n)| v | if n != 0 { 1 << p } else { 0 });
                    if deblocking == Deblocking::Enabled {
                        for (c, plane) in planes.iter_mut().enumerate() {
                            deblock::picture(
                                &mut plane.data,
                                plane.width,
                                plane.height,
                                if c == 0 { qp } else { t::chroma_qp(qp) },
                                c != 0,
                                |_, _| {
                                    Ok(deblock::Record {
                                        motion: state.motion,
                                        nonzero,
                                    })
                                },
                            )
                            .unwrap();
                        }
                    }
                    let mut expected = vec![
                        first.reconstruction.clone(),
                        planes.iter().flat_map(|p| p.data.iter().copied()).collect(),
                    ];
                    let mut decoder = coreav_h264::Decoder::with_kernels(
                        coreav_h264::Scalar,
                        coreav_h264::Limits::default(),
                    )
                    .with_mode(coreav_h264::Mode::EveryPicture);
                    for n in &first.nals {
                        decoder.decode_nal(n).unwrap();
                    }
                    decoder.end_access_unit().unwrap();
                    for n in &first.nals[..2] {
                        decoder.decode_nal(n).unwrap();
                    }
                    decoder.decode_nal(&nal).unwrap();
                    decoder.end_access_unit().unwrap();
                    decoder.flush().unwrap();
                    let pictures = decoder.take_pictures();
                    assert_eq!(pictures.len(), 2);
                    for (picture, expected) in pictures.iter().zip(&expected) {
                        let (cb, cr) = picture.chroma();
                        let actual: Vec<_> = picture
                            .luma()
                            .rows()
                            .chain(cb.rows())
                            .chain(cr.rows())
                            .flatten()
                            .copied()
                            .collect();
                        assert_eq!(
                            &actual, expected,
                            "layout={layout:?} qp={qp} deblocking={deblocking:?}"
                        );
                    }
                    assert!(decoder.warnings().is_empty());
                    assert_eq!(decoder.warnings_dropped(), 0);
                    let enabled = u8::from(deblocking == Deblocking::Enabled);
                    if let Some(dir) = &export {
                        let name = format!("forced-{qp}-{enabled}-{layout_index}");
                        let mut stream = Vec::new();
                        for n in first
                            .nals
                            .iter()
                            .chain(first.nals.iter().take(2))
                            .chain(std::iter::once(&nal))
                        {
                            stream.extend([0, 0, 0, 1]);
                            stream.extend(n);
                        }
                        std::fs::write(dir.join(format!("{name}.h264")), stream).unwrap();
                        let samples: Vec<u8> = expected.drain(..).flatten().collect();
                        std::fs::write(dir.join(format!("{name}.yuv")), samples).unwrap();
                    }
                    manifest.push_str(&format!("{layout_index}\t{qp}\t{enabled}\n"));
                    exported += 1;
                }
            }
        }
        assert_eq!(exported, 80);
        if let Some(dir) = &export {
            std::fs::write(dir.join("manifest.tsv"), manifest).unwrap();
        }
    }

    #[test]
    fn a_forbidden_cheaper_skip_cannot_defeat_the_zero_count_intra_fallback() {
        let mut encoder = Encoder::new(settings(Deblocking::Disabled), Limits::default()).unwrap();
        let y = [104; 256];
        let cb = [92; 64];
        let cr = [156; 64];
        let picture = Picture {
            y: Plane::new(&y, 16, 16, 16).unwrap(),
            cb: Plane::new(&cb, 8, 8, 8).unwrap(),
            cr: Plane::new(&cr, 8, 8, 8).unwrap(),
        };
        encoder.encode(picture).unwrap();
        let reference = encoder.reference.as_ref().unwrap();
        let (base, source) = reference[0].tiles(picture.y, 0, 0, 16).unwrap();
        let (cb, scb) = reference[1].tiles(picture.cb, 0, 0, 8).unwrap();
        let (cr, scr) = reference[2].tiles(picture.cr, 0, 0, 8).unwrap();
        let context = MbContext {
            top: None,
            left: None,
            p_slice: true,
        };
        let csources = [scb, scr];
        let decision = Decision {
            source: &source,
            csources: &csources,
            context,
            qp: 28,
            pending_skip: 0,
        };
        let search = Search {
            reference,
            decision: &decision,
            border: Border::default(),
            mx: 0,
            my: 0,
            previous: 0,
            colocated: None,
        };
        let a = IntraAvail::default();
        let intra = decision
            .candidate(
                intra16(&base, &source, a, 2, 28).unwrap(),
                chroma(&[cb, cr], &csources, a, 0, 28).unwrap(),
                None,
                2,
            )
            .unwrap();
        let mut skip = Inter::new(Layout::Whole).unwrap();
        skip.skip = true;
        let cheap = search.evaluate(skip.clone()).unwrap();
        assert!(cheap.key < intra.key);
        let forbidden = Search {
            previous: 16,
            colocated: None,
            ..search
        };
        assert!(matches!(forbidden.evaluate(skip), Err(Error::Syntax)));
        let mut best = Some(intra);
        forbidden
            .candidates(&mut best, &mut Error::Coefficient)
            .unwrap();
        assert!(best.unwrap().inter.is_none());
        let fine = Inter::new(Layout::Sub([3; 4])).unwrap();
        assert_eq!(fine.len, 16);
        assert!(search.evaluate(fine.clone()).is_ok());
        assert!(matches!(
            Search {
                previous: 1,
                colocated: None,
                ..search
            }
            .evaluate(fine),
            Err(Error::Syntax)
        ));
    }

    #[test]
    fn predictor_seeds_outside_the_integer_square_and_fractional_limits_are_admitted_exactly() {
        let mut encoder = Encoder::new(settings(Deblocking::Disabled), Limits::default()).unwrap();
        let y = [128; 256];
        let c = [128; 64];
        let picture = Picture {
            y: Plane::new(&y, 16, 16, 16).unwrap(),
            cb: Plane::new(&c, 8, 8, 8).unwrap(),
            cr: Plane::new(&c, 8, 8, 8).unwrap(),
        };
        encoder.encode(picture).unwrap();
        let reference = encoder.reference.as_ref().unwrap();
        let source = reference[0].tiles(picture.y, 0, 0, 16).unwrap().1;
        let csources = [
            reference[1].tiles(picture.cb, 0, 0, 8).unwrap().1,
            reference[2].tiles(picture.cr, 0, 0, 8).unwrap().1,
        ];
        let decision = Decision {
            source: &source,
            csources: &csources,
            context: MbContext {
                top: None,
                left: None,
                p_slice: true,
            },
            qp: 28,
            pending_skip: 0,
        };
        for mv in [Mv { x: -8192, y: -2048 }, Mv { x: 8191, y: 2047 }] {
            let mb = MotionMb {
                intra: false,
                vectors: [mv; 16],
            };
            let search = Search {
                reference,
                decision: &decision,
                border: Border {
                    left: Some(&mb),
                    top: Some(&mb),
                    top_right: Some(&mb),
                    ..Border::default()
                },
                mx: 0,
                my: 0,
                previous: 0,
                colocated: None,
            };
            let base = Inter::new(Layout::Whole).unwrap();
            let best = search
                .search_parts(base.clone(), 0, 1, &mut Error::Coefficient)
                .unwrap()
                .unwrap();
            assert_eq!(best.inter.unwrap().parts[0].mv, mv);
            for dx in [-2, -1, 0, 1, 2] {
                for dy in [-2, -1, 0, 1, 2] {
                    let trial = Mv {
                        x: mv.x + dx,
                        y: mv.y + dy,
                    };
                    let mut best = None;
                    search
                        .trial(&base, 0, trial, &mut best, &mut Error::Coefficient)
                        .unwrap();
                    assert_eq!(best.is_some(), trial.valid());
                }
            }
            let skip = Inter {
                skip: true,
                parts: {
                    let mut p = base.parts;
                    p[0].mv = mv;
                    p
                },
                ..base
            };
            assert!(search.evaluate(skip).is_ok());
        }
    }

    #[test]
    fn skip_prefix_costs_and_zero_residual_neighbours_match_the_emitted_syntax() {
        let source = Tile::empty();
        let csources = [Tile::empty(), Tile::empty()];
        let dirty = Neighbour {
            counts: [[16; 16]; 3],
            ..Neighbour::default()
        };
        let context = MbContext {
            top: Some(&dirty),
            left: Some(&dirty),
            p_slice: true,
        };
        let luma = Luma {
            mode: None,
            modes: [2; 16],
            levels: [[0; 16]; 16],
            dc: [0; 16],
            tile: source.clone(),
        };
        let chroma = Chroma {
            mode: 0,
            levels: [[[0; 16]; 4]; 2],
            dc: [[0; 4]; 2],
            tiles: csources.clone(),
        };
        for pending_skip in [0, 1, 2, 3, 7, 8, 20] {
            let decision = Decision {
                source: &source,
                csources: &csources,
                context,
                qp: 28,
                pending_skip,
            };
            for skip in [false, true] {
                let inter = Inter {
                    skip,
                    ..Inter::new(Layout::Whole).unwrap()
                };
                let candidate = decision
                    .candidate(luma.clone(), chroma.clone(), Some(inter), 0)
                    .unwrap();
                let mut writer = Bits::new(100);
                let run = pending_skip + u32::from(skip);
                writer.ue(run).unwrap();
                let prefix_bits = writer.len;
                assert_eq!(
                    prefix_bits,
                    (2 * (32 - (run + 1).leading_zeros()) - 1) as usize
                );
                let state = macroblock(
                    &mut writer,
                    &candidate.luma,
                    &candidate.chroma,
                    candidate.inter.as_ref(),
                    context,
                )
                .unwrap();
                assert_eq!(state.counts, [[0; 16]; 3]);
                assert!(!state.motion.intra);
                assert_eq!(writer.len, candidate.key.bits);
                if skip {
                    assert_eq!(writer.len, prefix_bits);
                }
            }
        }
    }

    #[test]
    fn oversized_macroblock_layers_are_rejected_before_cost_comparison_without_counting_skip_runs()
    {
        let source = Tile::empty();
        let csources = [Tile::empty(), Tile::empty()];
        let context = MbContext {
            top: None,
            left: None,
            p_slice: true,
        };
        let decision = Decision {
            source: &source,
            csources: &csources,
            context,
            qp: 10,
            pending_skip: 0,
        };
        let prediction = Inter::new(Layout::Whole).unwrap();
        let mut luma = Luma {
            mode: None,
            modes: [2; 16],
            levels: [[2047; 16]; 16],
            dc: [0; 16],
            tile: source.clone(),
        };
        let mut chroma = Chroma {
            mode: 0,
            levels: [[[2047; 16]; 4]; 2],
            dc: [[2047; 4]; 2],
            tiles: csources.clone(),
        };
        let layer_bits = |luma: &Luma, chroma: &Chroma| {
            let mut counter = Bits::count();
            let result = macroblock(&mut counter, luma, chroma, Some(&prediction), context);
            if counter.len > 3200 {
                assert!(matches!(result, Err(Error::MacroblockLimit)));
            } else {
                result.unwrap();
            }
            counter.len
        };
        let oversized = layer_bits(&luma, &chroma);
        assert!(oversized > 3200);
        let mut alternative_luma = luma.clone();
        alternative_luma.levels = [[0; 16]; 16];
        alternative_luma.tile.0.fill(0);
        let mut alternative_chroma = chroma.clone();
        alternative_chroma.levels = [[[0; 16]; 4]; 2];
        alternative_chroma.dc = [[0; 4]; 2];
        for tile in &mut alternative_chroma.tiles {
            tile.0.fill(0);
        }
        let alternative = decision
            .candidate(
                alternative_luma,
                alternative_chroma,
                Some(prediction.clone()),
                0,
            )
            .unwrap();
        assert!(decision.cost(0, oversized).unwrap() < alternative.key.cost);
        let mut best = Some(alternative);
        match decision.candidate(luma.clone(), chroma.clone(), Some(prediction.clone()), 0) {
            Ok(candidate) => consider(&mut best, candidate),
            Err(e) => assert_eq!(e, Error::MacroblockLimit),
        }
        assert!(best.unwrap().key.cost > decision.cost(0, oversized).unwrap());
        for index in 0..392 {
            if index < 256 {
                luma.levels[index / 16][index % 16] = 0;
            } else if index < 384 {
                let c = index - 256;
                chroma.levels[c / 64][c % 64 / 16][c % 16] = 0;
            } else {
                chroma.dc[(index - 384) / 4][(index - 384) % 4] = 0;
            }
            if layer_bits(&luma, &chroma) <= 3200 {
                break;
            }
        }
        let payload = layer_bits(&luma, &chroma);
        assert!((3138..=3200).contains(&payload));
        let with_run = Decision {
            pending_skip: u32::MAX - 1,
            ..decision
        };
        let accepted = with_run
            .candidate(luma, chroma, Some(prediction), 0)
            .unwrap();
        assert_eq!(accepted.key.bits, payload + 63);
        assert!(accepted.key.bits > 3200);
    }
}
