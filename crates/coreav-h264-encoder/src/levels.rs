#![forbid(unsafe_code)]

use super::*;

const AC_COORDINATES: usize = 1;
const REFINE_DC: bool = true;

fn coordinates(levels: [i32; 16], separate_dc: bool) -> impl Iterator<Item = usize> {
    t::SCAN
        .into_iter()
        .rev()
        .filter(move |&p| {
            (!separate_dc || p != 0)
                && (AC_COORDINATES == 16
                    || levels
                        .get(p)
                        .is_some_and(|level| (1..=2).contains(&level.unsigned_abs())))
        })
        .take(AC_COORDINATES)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LumaPrediction {
    Inter,
    Four,
    Sixteen,
}

struct Prediction {
    luma: Tile,
    chroma: [Tile; 2],
    mode: LumaPrediction,
    available: IntraAvail,
}

impl Prediction {
    fn new(
        candidate: &Candidate,
        base: &Tile,
        cbases: &[Tile; 2],
        available: IntraAvail,
        reference: Option<&[FramePlane; 3]>,
        (mx, my): (usize, usize),
    ) -> Result<Self> {
        let mut prediction = Self {
            luma: base.clone(),
            chroma: cbases.clone(),
            mode: LumaPrediction::Four,
            available,
        };
        if let Some(inter) = &candidate.inter {
            prediction.mode = LumaPrediction::Inter;
            let reference = reference.ok_or(Error::Internal)?;
            for &part in inter.parts.iter().take(inter.len) {
                inter::predict(
                    reference.first().ok_or(Error::Internal)?,
                    &mut prediction.luma,
                    part,
                    mx,
                    my,
                    false,
                )?;
                for (c, tile) in prediction.chroma.iter_mut().enumerate() {
                    inter::predict(
                        reference.get(c + 1).ok_or(Error::Internal)?,
                        tile,
                        part,
                        mx,
                        my,
                        true,
                    )?;
                }
            }
        } else {
            if let Some(mode) = candidate.luma.mode {
                prediction.mode = LumaPrediction::Sixteen;
                prediction.luma.prediction(0, 0, 16, available, mode)?;
            }
            for tile in &mut prediction.chroma {
                tile.prediction(0, 0, 8, available, candidate.chroma.mode)?;
            }
        }
        Ok(prediction)
    }

    fn quant(&self, qp: u8, chroma: bool) -> Result<&'static t::Quant> {
        match (self.mode == LumaPrediction::Inter, chroma) {
            (true, false) => t::Quant::of(qp),
            (false, false) => t::Quant::intra(qp),
            (true, true) => t::Quant::chroma_inter(qp),
            (false, true) => t::Quant::chroma_intra(qp),
        }
    }

    fn luma(&self, candidate: &Candidate, qp: u8, blocks: usize) -> Result<Tile> {
        let mut tile = self.luma.clone();
        let quant = self.quant(qp, false)?;
        let dc = if self.mode == LumaPrediction::Sixteen {
            Some(t::luma_dc_scaled(candidate.luma.dc, qp)?)
        } else {
            None
        };
        for (k, levels) in candidate.luma.levels.iter().enumerate().take(blocks) {
            let (x, y) = xy(k);
            if self.mode == LumaPrediction::Four {
                tile.prediction(
                    x,
                    y,
                    4,
                    availability(k, self.available),
                    *candidate
                        .luma
                        .modes
                        .get(y / 4 * 4 + x / 4)
                        .ok_or(Error::Internal)?,
                )?;
            }
            let dc = dc
                .as_ref()
                .map(|values| {
                    values
                        .get(y / 4 * 4 + x / 4)
                        .copied()
                        .ok_or(Error::Internal)
                })
                .transpose()?;
            tile.reconstruct(x, y, levels, quant, dc)?;
        }
        Ok(tile)
    }

    fn chroma(&self, candidate: &Candidate, qp: u8, c: usize) -> Result<Tile> {
        let mut tile = self.chroma.get(c).ok_or(Error::Internal)?.clone();
        let dc = t::chroma_dc_scaled(*candidate.chroma.dc.get(c).ok_or(Error::Internal)?, qp)?;
        let quant = self.quant(qp, true)?;
        for (k, (levels, dc)) in candidate
            .chroma
            .levels
            .get(c)
            .ok_or(Error::Internal)?
            .iter()
            .zip(dc)
            .enumerate()
        {
            tile.reconstruct(k % 2 * 4, k / 2 * 4, levels, quant, Some(dc))?;
        }
        Ok(tile)
    }

    fn changed(&self, candidate: &Candidate, qp: u8, position: Coefficient) -> Result<Tile> {
        let (plane, k) = match position {
            Coefficient::LumaDc(_) => return self.luma(candidate, qp, 16),
            Coefficient::ChromaDc(c, _) => return self.chroma(candidate, t::chroma_qp(qp), c),
            Coefficient::Luma(k, _) => (0, k),
            Coefficient::Chroma(c, k, _) => (c + 1, k),
        };
        if plane == 0 && self.mode == LumaPrediction::Four {
            let mut tile = candidate.luma.tile.clone();
            let quant = self.quant(qp, false)?;
            for block in k..16 {
                let (x, y) = xy(block);
                let available = availability(block, self.available);
                if block != k {
                    let at = Tile::at(x, y);
                    let differs = |index| -> Result<bool> {
                        Ok(tile.0.get(index).ok_or(Error::Internal)?
                            != candidate.luma.tile.0.get(index).ok_or(Error::Internal)?)
                    };
                    let mut dirty = available.top_left && differs(at - STRIDE - 1)?;
                    for p in 0..4 {
                        dirty |= available.top && differs(at - STRIDE + p)?;
                        dirty |= available.top_right && differs(at - STRIDE + 4 + p)?;
                        dirty |= available.left && differs(at - 1 + p * STRIDE)?;
                    }
                    if !dirty {
                        continue;
                    }
                }
                tile.prediction(
                    x,
                    y,
                    4,
                    available,
                    *candidate
                        .luma
                        .modes
                        .get(y / 4 * 4 + x / 4)
                        .ok_or(Error::Internal)?,
                )?;
                tile.reconstruct(
                    x,
                    y,
                    candidate.luma.levels.get(block).ok_or(Error::Internal)?,
                    quant,
                    None,
                )?;
            }
            return Ok(tile);
        }
        let (mut tile, predicted, levels, dc, x, y, qp) = if plane == 0 {
            let (x, y) = xy(k);
            let dc = if self.mode == LumaPrediction::Sixteen {
                Some(
                    *t::luma_dc_scaled(candidate.luma.dc, qp)?
                        .get(y / 4 * 4 + x / 4)
                        .ok_or(Error::Internal)?,
                )
            } else {
                None
            };
            (
                candidate.luma.tile.clone(),
                &self.luma,
                candidate.luma.levels.get(k).ok_or(Error::Internal)?,
                dc,
                x,
                y,
                qp,
            )
        } else {
            let c = plane - 1;
            let qp = t::chroma_qp(qp);
            let dc = t::chroma_dc_scaled(*candidate.chroma.dc.get(c).ok_or(Error::Internal)?, qp)?;
            (
                candidate
                    .chroma
                    .tiles
                    .get(c)
                    .ok_or(Error::Internal)?
                    .clone(),
                self.chroma.get(c).ok_or(Error::Internal)?,
                candidate
                    .chroma
                    .levels
                    .get(c)
                    .and_then(|blocks| blocks.get(k))
                    .ok_or(Error::Internal)?,
                Some(*dc.get(k).ok_or(Error::Internal)?),
                k % 2 * 4,
                k / 2 * 4,
                qp,
            )
        };
        for row in y..y + 4 {
            let at = Tile::at(x, row);
            tile.0
                .get_mut(at..at + 4)
                .ok_or(Error::Internal)?
                .copy_from_slice(predicted.0.get(at..at + 4).ok_or(Error::Internal)?);
        }
        tile.reconstruct(x, y, levels, self.quant(qp, plane != 0)?, dc)?;
        Ok(tile)
    }
}

#[derive(Clone, Copy)]
enum Coefficient {
    LumaDc(usize),
    Luma(usize, usize),
    ChromaDc(usize, usize),
    Chroma(usize, usize, usize),
}

pub(super) struct ResidualCosts {
    blocks: [cavlc::Cost; 27],
    active: u32,
}

impl ResidualCosts {
    fn new(candidate: &Candidate) -> Result<Self> {
        let mut costs = Self {
            blocks: [cavlc::Cost::default(); 27],
            active: 0,
        };
        costs.update(candidate, Coefficient::LumaDc(0))?;
        for k in 0..16 {
            costs.update(candidate, Coefficient::Luma(k, 0))?;
        }
        for c in 0..2 {
            costs.update(candidate, Coefficient::ChromaDc(c, 0))?;
            for k in 0..4 {
                costs.update(candidate, Coefficient::Chroma(c, k, 1))?;
            }
        }
        Ok(costs)
    }

    fn update(&mut self, candidate: &Candidate, position: Coefficient) -> Result<()> {
        let intra16 = candidate.inter.is_none() && candidate.luma.mode.is_some();
        let (index, cost) = match position {
            Coefficient::LumaDc(_) => (0, cavlc::Cost::new(&t::scan(&candidate.luma.dc)?, 0)?),
            Coefficient::Luma(k, _) => {
                let levels = candidate.luma.levels.get(k).ok_or(Error::Internal)?;
                let scan = t::scan(levels)?;
                (
                    1 + k,
                    cavlc::Cost::new(scan.get(usize::from(intra16)..).ok_or(Error::Internal)?, 0)?,
                )
            }
            Coefficient::ChromaDc(c, _) => (
                17 + c,
                cavlc::Cost::new(candidate.chroma.dc.get(c).ok_or(Error::Internal)?, -1)?,
            ),
            Coefficient::Chroma(c, k, _) => {
                let levels = candidate
                    .chroma
                    .levels
                    .get(c)
                    .and_then(|blocks| blocks.get(k))
                    .ok_or(Error::Internal)?;
                (
                    19 + c * 4 + k,
                    cavlc::Cost::new(t::scan(levels)?.get(1..).ok_or(Error::Internal)?, 0)?,
                )
            }
        };
        self.put(index, cost)
    }

    fn put(&mut self, index: usize, cost: cavlc::Cost) -> Result<()> {
        *self.blocks.get_mut(index).ok_or(Error::Internal)? = cost;
        let bit = 1u32 << index;
        self.active = (self.active & !bit) | if cost.total == 0 { 0 } else { bit };
        Ok(())
    }

    pub(super) fn patterns(&self, intra16: bool) -> (u8, u8) {
        let mut y = 0;
        for group in 0..4 {
            if self.active & (15 << (1 + 4 * group)) != 0 {
                y |= 1 << group;
            }
        }
        if intra16 && y != 0 {
            y = 15;
        }
        let c = if self.active & (255 << 19) != 0 {
            2
        } else if self.active & (3 << 17) != 0 {
            1
        } else {
            0
        };
        (y, c)
    }

    pub(super) fn count(&self, w: &mut Bits, index: usize, nc: i32) -> Result<u8> {
        let cost = self.blocks.get(index).ok_or(Error::Internal)?;
        w.len = w.len.checked_add(cost.bits(nc)?).ok_or(Error::Limit)?;
        Ok(cost.total)
    }
}

impl Coefficient {
    fn index(self) -> usize {
        match self {
            Self::LumaDc(_) => 0,
            Self::Luma(k, _) => 1 + k,
            Self::ChromaDc(c, _) => 17 + c,
            Self::Chroma(c, k, _) => 19 + c * 4 + k,
        }
    }

    fn region(self, mode: LumaPrediction) -> (usize, usize, usize) {
        match self {
            Self::Luma(k, _) if mode != LumaPrediction::Four => {
                let (x, y) = xy(k);
                (x, y, 4)
            }
            Self::Luma(..) | Self::LumaDc(_) => (0, 0, 16),
            Self::Chroma(_, k, _) => (k % 2 * 4, k / 2 * 4, 4),
            Self::ChromaDc(..) => (0, 0, 8),
        }
    }

    fn plane(self) -> usize {
        match self {
            Self::LumaDc(_) | Self::Luma(_, _) => 0,
            Self::ChromaDc(c, _) | Self::Chroma(c, _, _) => c + 1,
        }
    }

    fn at(self, candidate: &mut Candidate) -> Result<&mut i32> {
        match self {
            Self::LumaDc(p) => candidate.luma.dc.get_mut(p),
            Self::Luma(k, p) => candidate.luma.levels.get_mut(k).and_then(|b| b.get_mut(p)),
            Self::ChromaDc(c, p) => candidate.chroma.dc.get_mut(c).and_then(|b| b.get_mut(p)),
            Self::Chroma(c, k, p) => candidate
                .chroma
                .levels
                .get_mut(c)
                .and_then(|b| b.get_mut(k))
                .and_then(|b| b.get_mut(p)),
        }
        .ok_or(Error::Internal)
    }
}

struct Refinement {
    costs: ResidualCosts,
    errors: [u64; 3],
}

impl Decision<'_> {
    #[cfg(test)]
    pub(super) fn residual_bits(
        &self,
        luma: &Luma,
        chroma: &Chroma,
        inter: Option<&inter::Inter>,
    ) -> Result<usize> {
        let mut counter = Bits::count();
        if self.context.p_slice {
            counter.ue(self.pending_skip + u32::from(inter.is_some_and(|i| i.skip)))?;
        }
        macroblock(&mut counter, luma, chroma, inter, self.context)?;
        Ok(counter.len)
    }

    fn plane_error(&self, candidate: &Candidate, plane: usize) -> Result<u64> {
        if plane == 0 {
            candidate.luma.tile.error(self.source, 0, 0, 16)
        } else {
            candidate
                .chroma
                .tiles
                .get(plane - 1)
                .ok_or(Error::Internal)?
                .error(
                    self.csources.get(plane - 1).ok_or(Error::Internal)?,
                    0,
                    0,
                    8,
                )
        }
    }

    fn changed_bits(
        &self,
        candidate: &Candidate,
        costs: &ResidualCosts,
        position: Coefficient,
        old: cavlc::Cost,
    ) -> Result<(usize, [[u8; 16]; 3])> {
        let mut counts = candidate.state.counts;
        let cost = *costs.blocks.get(position.index()).ok_or(Error::Internal)?;
        let (mut added, mut removed) = match position {
            Coefficient::LumaDc(_) => {
                let nc = nc(&[0; 16], self.context.top, self.context.left, 0, 0);
                (cost.bits(nc)?, old.bits(nc)?)
            }
            Coefficient::ChromaDc(..) => (cost.bits(-1)?, old.bits(-1)?),
            Coefficient::Luma(..) | Coefficient::Chroma(..) => (0, 0),
        };
        if let Coefficient::Luma(k, _) | Coefficient::Chroma(_, k, _) = position {
            let plane = position.plane();
            let (p, width) = if plane == 0 {
                let (x, y) = xy(k);
                (y / 4 * 4 + x / 4, 4)
            } else {
                (k, 2)
            };
            let before = candidate.state.counts.get(plane).ok_or(Error::Internal)?;
            let after = counts.get_mut(plane).ok_or(Error::Internal)?;
            *after.get_mut(p).ok_or(Error::Internal)? = cost.total;
            let context =
                |values: &[u8; 16], p| nc(values, self.context.top, self.context.left, plane, p);
            added += cost.bits(context(after, p))?;
            removed += old.bits(context(before, p))?;
            let (cbpy, cbpc) =
                costs.patterns(candidate.inter.is_none() && candidate.luma.mode.is_some());
            for neighbour in [
                if p % width + 1 < width {
                    Some(p + 1)
                } else {
                    None
                },
                if p + width < width * width {
                    Some(p + width)
                } else {
                    None
                },
            ]
            .into_iter()
            .flatten()
            {
                let index = if plane == 0 {
                    1 + order(neighbour % 4, neighbour / 4)
                } else {
                    19 + (plane - 1) * 4 + neighbour
                };
                let coded = if plane == 0 {
                    cbpy & (1 << ((index - 1) / 4)) != 0
                } else {
                    cbpc == 2
                };
                if coded {
                    let cost = costs.blocks.get(index).ok_or(Error::Internal)?;
                    added += cost.bits(context(after, neighbour))?;
                    removed += cost.bits(context(before, neighbour))?;
                }
            }
        }
        let bits = candidate
            .key
            .bits
            .checked_add(added)
            .and_then(|bits| bits.checked_sub(removed))
            .ok_or(Error::Internal)?;
        let mut prefix = Bits::count();
        if self.context.p_slice {
            prefix.ue(self.pending_skip)?;
        }
        if bits.checked_sub(prefix.len).ok_or(Error::Internal)? > 3200 {
            return Err(Error::MacroblockLimit);
        }
        #[cfg(test)]
        {
            let state = macroblock(
                &mut prefix,
                &candidate.luma,
                &candidate.chroma,
                candidate.inter.as_ref(),
                self.context,
            )?;
            assert_eq!(bits, prefix.len);
            assert_eq!(counts, state.counts);
        }
        Ok((bits, counts))
    }

    fn coordinate(
        &self,
        candidate: &mut Candidate,
        prediction: &Prediction,
        position: Coefficient,
        nearest: i32,
        refinement: &mut Refinement,
    ) -> Result<()> {
        if nearest == 0 && *position.at(candidate)? == 0 {
            return Ok(());
        }
        let plane = position.plane();
        let qp = if plane == 0 {
            self.qp
        } else {
            t::chroma_qp(self.qp)
        };
        let weight = t::rate_weight(qp)?;
        let distortion_scale = if plane == 0 { 512 } else { 2048 };
        let mut best = distortion_scale * refinement.errors.get(plane).ok_or(Error::Internal)?
            + weight * candidate.key.bits as u64;
        let source = if plane == 0 {
            self.source
        } else {
            self.csources.get(plane - 1).ok_or(Error::Internal)?
        };
        let (x, y, n) = position.region(prediction.mode);
        let tile = if plane == 0 {
            &candidate.luma.tile
        } else {
            candidate
                .chroma
                .tiles
                .get(plane - 1)
                .ok_or(Error::Internal)?
        };
        let mut old_error = tile.error(source, x, y, n)?;
        let alternatives = [nearest, nearest - nearest.signum(), 0];
        for (index, &level) in alternatives.iter().enumerate() {
            if level == *position.at(candidate)?
                || alternatives.iter().take(index).any(|&seen| seen == level)
            {
                continue;
            }
            let old_level = *position.at(candidate)?;
            let old_cost = *refinement
                .costs
                .blocks
                .get(position.index())
                .ok_or(Error::Internal)?;
            let intra16 = candidate.inter.is_none() && candidate.luma.mode.is_some();
            let patterns = refinement.costs.patterns(intra16);
            *position.at(candidate)? = level;
            let attempt = (|| {
                refinement.costs.update(candidate, position)?;
                let (bits, counts) = if patterns == refinement.costs.patterns(intra16) {
                    self.changed_bits(candidate, &refinement.costs, position, old_cost)?
                } else {
                    let mut counter = Bits::count();
                    if self.context.p_slice {
                        counter.ue(self.pending_skip)?;
                    }
                    let state = macroblock_syntax(
                        &mut counter,
                        &candidate.luma,
                        &candidate.chroma,
                        candidate.inter.as_ref(),
                        self.context,
                        Some(&refinement.costs),
                    )?;
                    (counter.len, state.counts)
                };
                let tile = prediction.changed(candidate, self.qp, position)?;
                let error = tile.error(source, x, y, n)?;
                let total =
                    refinement.errors.get(plane).ok_or(Error::Internal)? - old_error + error;
                let score = distortion_scale * total + weight * bits as u64;
                Ok::<_, Error>((score, bits, tile, error, total, counts))
            })();
            match attempt {
                Ok((score, bits, tile, error, total, counts)) if score < best => {
                    best = score;
                    candidate.key.bits = bits;
                    candidate.state.counts = counts;
                    *refinement.errors.get_mut(plane).ok_or(Error::Internal)? = total;
                    old_error = error;
                    if plane == 0 {
                        candidate.luma.tile = tile;
                    } else {
                        *candidate
                            .chroma
                            .tiles
                            .get_mut(plane - 1)
                            .ok_or(Error::Internal)? = tile;
                    }
                    continue;
                }
                Ok(_) | Err(Error::Coefficient | Error::Syntax | Error::MacroblockLimit) => {}
                Err(error) => return Err(error),
            }
            *position.at(candidate)? = old_level;
            refinement.costs.put(position.index(), old_cost)?;
        }
        Ok(())
    }

    pub(super) fn refine_levels(
        &self,
        mut candidate: Candidate,
        base: &Tile,
        cbases: &[Tile; 2],
        available: IntraAvail,
        reference: Option<&[FramePlane; 3]>,
        position: (usize, usize),
    ) -> Result<Candidate> {
        if candidate.inter.as_ref().is_some_and(|i| i.skip) {
            return Ok(candidate);
        }
        if AC_COORDINATES < 16
            && candidate
                .luma
                .levels
                .iter()
                .flatten()
                .all(|&level| level == 0)
            && candidate
                .chroma
                .levels
                .iter()
                .flatten()
                .flatten()
                .all(|&level| level == 0)
            && candidate.luma.dc.iter().all(|&level| level == 0)
            && candidate
                .chroma
                .dc
                .iter()
                .flatten()
                .all(|&level| level == 0)
        {
            return Ok(candidate);
        }
        let prediction = Prediction::new(&candidate, base, cbases, available, reference, position)?;
        let mut refinement = Refinement {
            costs: ResidualCosts::new(&candidate)?,
            errors: [
                self.plane_error(&candidate, 0)?,
                self.plane_error(&candidate, 1)?,
                self.plane_error(&candidate, 2)?,
            ],
        };
        if REFINE_DC && prediction.mode == LumaPrediction::Sixteen {
            let mut sums = [0; 16];
            for k in 0..16 {
                let (x, y) = xy(k);
                *sums.get_mut(y / 4 * 4 + x / 4).ok_or(Error::Internal)? =
                    prediction.luma.forward(self.source, x, y)?.dc();
            }
            let nearest = t::luma_dc_nearest(sums, self.qp)?;
            for &p in t::SCAN.iter().rev() {
                self.coordinate(
                    &mut candidate,
                    &prediction,
                    Coefficient::LumaDc(p),
                    *nearest.get(p).ok_or(Error::Internal)?,
                    &mut refinement,
                )?;
            }
        }
        if AC_COORDINATES != 0 {
            let quant = prediction.quant(self.qp, false)?;
            for k in 0..16 {
                let (x, y) = xy(k);
                let mut tile = if prediction.mode == LumaPrediction::Four {
                    candidate.luma.tile.clone()
                } else {
                    prediction.luma.clone()
                };
                if prediction.mode == LumaPrediction::Four {
                    tile.prediction(
                        x,
                        y,
                        4,
                        availability(k, prediction.available),
                        *candidate
                            .luma
                            .modes
                            .get(y / 4 * 4 + x / 4)
                            .ok_or(Error::Internal)?,
                    )?;
                }
                let separate_dc = prediction.mode == LumaPrediction::Sixteen;
                let coefficients = tile.forward(self.source, x, y)?;
                for p in coordinates(
                    *candidate.luma.levels.get(k).ok_or(Error::Internal)?,
                    separate_dc,
                ) {
                    self.coordinate(
                        &mut candidate,
                        &prediction,
                        Coefficient::Luma(k, p),
                        quant.nearest_level(&coefficients, p)?,
                        &mut refinement,
                    )?;
                }
            }
        }
        let qp = t::chroma_qp(self.qp);
        let quant = prediction.quant(qp, true)?;
        for c in 0..2 {
            let tile = prediction.chroma.get(c).ok_or(Error::Internal)?;
            let source = self.csources.get(c).ok_or(Error::Internal)?;
            if REFINE_DC {
                let mut sums = [0; 4];
                for (k, sum) in sums.iter_mut().enumerate() {
                    *sum = tile.forward(source, k % 2 * 4, k / 2 * 4)?.dc();
                }
                let nearest = t::chroma_dc_nearest(sums, qp)?;
                for (p, &level) in nearest.iter().enumerate().rev() {
                    self.coordinate(
                        &mut candidate,
                        &prediction,
                        Coefficient::ChromaDc(c, p),
                        level,
                        &mut refinement,
                    )?;
                }
            }
            if AC_COORDINATES != 0 {
                for k in 0..4 {
                    let coefficients = tile.forward(source, k % 2 * 4, k / 2 * 4)?;
                    for p in coordinates(
                        *candidate
                            .chroma
                            .levels
                            .get(c)
                            .and_then(|blocks| blocks.get(k))
                            .ok_or(Error::Internal)?,
                        true,
                    ) {
                        self.coordinate(
                            &mut candidate,
                            &prediction,
                            Coefficient::Chroma(c, k, p),
                            quant.nearest_level(&coefficients, p)?,
                            &mut refinement,
                        )?;
                    }
                }
            }
        }
        candidate.key.cost = self.cost(refinement.errors.iter().sum(), candidate.key.bits)?;
        Ok(candidate)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn cached_macroblock_cost_tracks_pattern_and_neighbour_context_changes() {
        let mut source = Tile::empty();
        for y in 0..16 {
            for x in 0..16 {
                source.0[Tile::at(x, y)] = (x * 17 + y * 29) as u8;
            }
        }
        let base = Tile::empty();
        let cbases = [base.clone(), base.clone()];
        let available = IntraAvail {
            top: false,
            left: false,
            top_left: false,
            top_right: false,
        };
        for four in [false, true] {
            let luma = if four {
                intra4(&base, &source, available, 28, None, None, Trials::Shortlist)
                    .unwrap()
                    .1
            } else {
                intra16(&base, &source, available, 2, 28).unwrap()
            };
            let chroma = chroma(&cbases, &cbases, available, 0, 28).unwrap();
            let empty_decision = Decision {
                source: &source,
                csources: &cbases,
                context: MbContext {
                    top: None,
                    left: None,
                    p_slice: true,
                },
                qp: 28,
                pending_skip: 7,
            };
            let mut candidate = empty_decision
                .candidate(luma, chroma, None, if four { 4 } else { 2 })
                .unwrap();
            let mut costs = ResidualCosts::new(&candidate).unwrap();
            let mut positions = Vec::new();
            if !four {
                positions.push(Coefficient::LumaDc(0));
            }
            for k in 0..16 {
                positions.push(Coefficient::Luma(k, 1));
            }
            for c in 0..2 {
                positions.push(Coefficient::ChromaDc(c, 0));
                for k in 0..4 {
                    positions.push(Coefficient::Chroma(c, k, 1));
                }
            }
            for position in positions {
                for value in [0, 1, -1, 3] {
                    *position.at(&mut candidate).unwrap() = value;
                    costs.update(&candidate, position).unwrap();
                    for count in [0, 1, 2, 4, 8, 16] {
                        let neighbour = Neighbour {
                            counts: [[count; 16]; 3],
                            ..Neighbour::default()
                        };
                        let context = MbContext {
                            top: Some(&neighbour),
                            left: Some(&neighbour),
                            p_slice: true,
                        };
                        let mut plain = Bits::count();
                        let plain_state = macroblock(
                            &mut plain,
                            &candidate.luma,
                            &candidate.chroma,
                            None,
                            context,
                        )
                        .unwrap();
                        let mut cached = Bits::count();
                        let cached_state = macroblock_syntax(
                            &mut cached,
                            &candidate.luma,
                            &candidate.chroma,
                            None,
                            context,
                            Some(&costs),
                        )
                        .unwrap();
                        assert_eq!(plain.len, cached.len);
                        assert_eq!(plain_state.counts, cached_state.counts);
                    }
                }
            }
        }
    }

    #[test]
    fn replay_preserves_seed_and_level_search_keeps_modes_and_improves_exact_plane_objective() {
        let base = Tile::empty();
        let cbases = [base.clone(), base.clone()];
        let mut csources = cbases.clone();
        for (c, source) in csources.iter_mut().enumerate() {
            for y in 0..8 {
                for x in 0..8 {
                    source.0[Tile::at(x, y)] = (96 + c * 48 + (x * 5 + y * 9) % 31) as u8;
                }
            }
        }
        let available = IntraAvail {
            top: false,
            left: false,
            top_left: false,
            top_right: false,
        };
        let mut changed = false;
        let mut changed_chroma = false;
        for qp in [10, 22, 28, 40, 51] {
            let mut source = base.clone();
            for y in 0..16 {
                for x in 0..16 {
                    source.0[Tile::at(x, y)] = (128 + ((x * 5 + y * 9) % 31) as i32 - 15) as u8;
                }
            }
            let decision = Decision {
                source: &source,
                csources: &csources,
                context: MbContext {
                    top: None,
                    left: None,
                    p_slice: false,
                },
                qp,
                pending_skip: 0,
            };
            for four in [false, true] {
                let luma = if four {
                    intra4(&base, &source, available, qp, None, None, Trials::Shortlist)
                        .unwrap()
                        .1
                } else {
                    intra16(&base, &source, available, 2, qp).unwrap()
                };
                let chroma = chroma(&cbases, &csources, available, 0, t::chroma_qp(qp)).unwrap();
                let seed = decision
                    .candidate(luma, chroma, None, if four { 4 } else { 2 })
                    .unwrap();
                let prediction =
                    Prediction::new(&seed, &base, &cbases, available, None, (0, 0)).unwrap();
                assert_eq!(
                    prediction
                        .luma(&seed, qp, 16)
                        .unwrap()
                        .error(&seed.luma.tile, 0, 0, 16)
                        .unwrap(),
                    0
                );
                for c in 0..2 {
                    assert_eq!(
                        prediction
                            .chroma(&seed, t::chroma_qp(qp), c)
                            .unwrap()
                            .error(&seed.chroma.tiles[c], 0, 0, 8)
                            .unwrap(),
                        0
                    );
                }
                let mut positions = Vec::new();
                if !four {
                    positions.push(Coefficient::LumaDc(0));
                }
                for k in 0..16 {
                    positions.push(Coefficient::Luma(k, 15));
                }
                for c in 0..2 {
                    positions.push(Coefficient::ChromaDc(c, 0));
                    for k in 0..4 {
                        positions.push(Coefficient::Chroma(c, k, 15));
                    }
                }
                for position in positions {
                    let mut trial = seed.clone();
                    *position.at(&mut trial).unwrap() += 1;
                    let full = if position.plane() == 0 {
                        prediction.luma(&trial, qp, 16).unwrap()
                    } else {
                        prediction
                            .chroma(&trial, t::chroma_qp(qp), position.plane() - 1)
                            .unwrap()
                    };
                    let partial = prediction.changed(&trial, qp, position).unwrap();
                    assert_eq!(full.0, partial.0);
                }
                let objective = |candidate: &Candidate| {
                    let y = t::rate_weight(qp).unwrap();
                    let c = t::rate_weight(t::chroma_qp(qp)).unwrap();
                    512 * (decision.plane_error(candidate, 0).unwrap() * c
                        + 4 * (decision.plane_error(candidate, 1).unwrap()
                            + decision.plane_error(candidate, 2).unwrap())
                            * y)
                        + candidate.key.bits as u64 * y * c
                };
                let before = objective(&seed);
                let result = decision
                    .refine_levels(seed.clone(), &base, &cbases, available, None, (0, 0))
                    .unwrap();
                let after = objective(&result);
                assert!(after <= before, "QP={qp} four={four}");
                assert_eq!(result.luma.mode, seed.luma.mode);
                assert_eq!(result.luma.modes, seed.luma.modes);
                assert_eq!(result.chroma.mode, seed.chroma.mode);
                assert_eq!(
                    result.key.bits,
                    decision
                        .residual_bits(&result.luma, &result.chroma, None)
                        .unwrap()
                );
                let distortion: u64 = (0..3)
                    .map(|plane| decision.plane_error(&result, plane).unwrap())
                    .sum();
                assert_eq!(
                    result.key.cost,
                    decision.cost(distortion, result.key.bits).unwrap()
                );
                assert!(result.inter.is_none());
                let prediction =
                    Prediction::new(&result, &base, &cbases, available, None, (0, 0)).unwrap();
                assert_eq!(
                    prediction
                        .luma(&result, qp, 16)
                        .unwrap()
                        .error(&result.luma.tile, 0, 0, 16)
                        .unwrap(),
                    0
                );
                changed |= result.luma.levels != seed.luma.levels || result.luma.dc != seed.luma.dc;
                changed_chroma |= result.chroma.levels != seed.chroma.levels
                    || result.chroma.dc != seed.chroma.dc;
            }
        }
        assert!(changed);
        assert!(changed_chroma);
    }
}
