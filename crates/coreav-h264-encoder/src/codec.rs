#![forbid(unsafe_code)]

use crate::{
    Deblocking, Encoder, Error, Picture, Plane, Result,
    api::zeros,
    bits::Bits,
    cavlc, deblock,
    motion::{self, Border, MotionMb, Mv},
    syntax, transform as t,
    vlc::{INTER_CBP, INTRA_CBP},
};
use coreav_kernels_h264::{BlockMut, IntraAvail, IntraMut, Kernels, Scalar};

#[path = "inter.rs"]
mod inter;
#[path = "levels.rs"]
mod levels;
#[path = "rows.rs"]
mod rows;

const STRIDE: usize = 40;
const ORIGIN: usize = STRIDE + 1;
const P_INTRA_TYPE_BASE: u32 = 5;
const FINALISTS: usize = 2;
const UNRANKED: (u64, u8) = (u64::MAX, u8::MAX);
type Ranked = [(u64, u8); 4];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Trials {
    Cheap,
    Shortlist,
    Every,
}

#[derive(Clone)]
struct Tile([u8; STRIDE * 18]);
impl Tile {
    fn empty() -> Self {
        Self([128; STRIDE * 18])
    }
    fn at(x: usize, y: usize) -> usize {
        ORIGIN + y * STRIDE + x
    }
    fn prediction(
        &mut self,
        x: usize,
        y: usize,
        n: usize,
        avail: IntraAvail,
        mode: u8,
    ) -> Result<()> {
        let view = IntraMut::new(&mut self.0, Self::at(x, y), STRIDE, n, avail)
            .map_err(|_| Error::Internal)?;
        match n {
            4 => Scalar.intra4x4(view, mode),
            8 => Scalar.intra_chroma(view, mode),
            16 => Scalar.intra16x16(view, mode),
            _ => return Err(Error::Internal),
        }
        Ok(())
    }
    fn forward(&self, source: &Self, x: usize, y: usize) -> Result<t::Forward> {
        t::forward(&source.0, &self.0, Self::at(x, y), STRIDE)
    }
    fn reconstruct(
        &mut self,
        x: usize,
        y: usize,
        levels: &[i32; 16],
        quant: &t::Quant,
        dc: Option<i32>,
    ) -> Result<()> {
        quant.reconstruct(levels, dc, &mut self.0, Self::at(x, y), STRIDE)
    }
    fn code(&mut self, source: &Self, x: usize, y: usize, quant: &t::Quant) -> Result<[i32; 16]> {
        let levels = quant.levels(&self.forward(source, x, y)?, false);
        self.reconstruct(x, y, &levels, quant, None)?;
        Ok(levels)
    }
    fn error(&self, source: &Self, x: usize, y: usize, n: usize) -> Result<u64> {
        let mut sum = 0;
        for j in y..y + n {
            for i in x..x + n {
                let at = Self::at(i, j);
                let d = i32::from(*source.0.get(at).ok_or(Error::Internal)?)
                    - i32::from(*self.0.get(at).ok_or(Error::Internal)?);
                sum += (d * d) as u64;
            }
        }
        Ok(sum)
    }

    fn satd(&self, source: &Self, x: usize, y: usize, n: usize) -> Result<u64> {
        crate::kernels::satd(&source.0, &self.0, Self::at(x, y), STRIDE, n)
    }
}

#[derive(Clone, Default)]
struct Neighbour {
    counts: [[u8; 16]; 3],
    modes: Option<[u8; 16]>,
    motion: MotionMb,
}

#[derive(Clone)]
struct Luma {
    mode: Option<u8>,
    modes: [u8; 16],
    levels: [[i32; 16]; 16],
    dc: [i32; 16],
    tile: Tile,
}
#[derive(Clone)]
struct Chroma {
    mode: u8,
    levels: [[[i32; 16]; 4]; 2],
    dc: [[i32; 4]; 2],
    tiles: [Tile; 2],
}

fn xy(k: usize) -> (usize, usize) {
    ((k / 4 % 2) * 8 + (k % 2) * 4, (k / 8) * 8 + (k % 4 / 2) * 4)
}
fn order(x: usize, y: usize) -> usize {
    (y / 2) * 8 + (x / 2) * 4 + (y % 2) * 2 + x % 2
}

fn availability(k: usize, mb: IntraAvail) -> IntraAvail {
    let (x, y) = xy(k);
    let top_right = if y == 0 {
        if x < 12 { mb.top } else { mb.top_right }
    } else {
        x < 12 && order(x / 4 + 1, y / 4 - 1) < k
    };
    IntraAvail {
        top: y != 0 || mb.top,
        left: x != 0 || mb.left,
        top_left: match (x == 0, y == 0) {
            (true, true) => mb.top_left,
            (true, false) => mb.left,
            (false, true) => mb.top,
            _ => true,
        },
        top_right,
    }
}

fn intra_availability(border: Border<'_>) -> IntraAvail {
    IntraAvail {
        left: border.left.is_some_and(|n| n.intra),
        top: border.top.is_some_and(|n| n.intra),
        top_left: border.top_left.is_some_and(|n| n.intra),
        top_right: border.top_right.is_some_and(|n| n.intra),
    }
}

fn legal(mode: u8, n: usize, a: IntraAvail) -> bool {
    if n == 8 {
        match mode {
            0 => true,
            1 => a.left,
            2 => a.top,
            3 => a.top && a.left && a.top_left,
            _ => false,
        }
    } else if n == 16 {
        match mode {
            0 => a.top,
            1 => a.left,
            2 => true,
            3 => a.top && a.left && a.top_left,
            _ => false,
        }
    } else {
        match mode {
            0 | 3 | 7 => a.top,
            1 | 8 => a.left,
            2 => true,
            4..=6 => a.top && a.left && a.top_left,
            _ => false,
        }
    }
}

fn neighbours(
    current: &[u8; 16],
    top: Option<&[u8; 16]>,
    left: Option<&[u8; 16]>,
    index: usize,
    width: usize,
) -> (Option<u8>, Option<u8>) {
    let x = index % width;
    let y = index / width;
    let a = if x != 0 {
        current.get(index - 1)
    } else {
        left.and_then(|r| r.get(y * width + width - 1))
    };
    let b = if y != 0 {
        current.get(index - width)
    } else {
        top.and_then(|r| r.get((width - 1) * width + x))
    };
    (a.copied(), b.copied())
}

fn nc(
    current: &[u8; 16],
    top: Option<&Neighbour>,
    left: Option<&Neighbour>,
    c: usize,
    index: usize,
) -> i32 {
    let (a, b) = neighbours(
        current,
        top.and_then(|n| n.counts.get(c)),
        left.and_then(|n| n.counts.get(c)),
        index,
        if c == 0 { 4 } else { 2 },
    );
    match (a, b) {
        (Some(a), Some(b)) => (i32::from(a) + i32::from(b) + 1) / 2,
        (Some(n), _) | (_, Some(n)) => i32::from(n),
        _ => 0,
    }
}

fn predicted_mode(
    modes: &[u8; 16],
    top: Option<&Neighbour>,
    left: Option<&Neighbour>,
    index: usize,
) -> u8 {
    let top = top.filter(|n| n.motion.intra);
    let left = left.filter(|n| n.motion.intra);
    if (index % 4 == 0 && left.is_none()) || (index / 4 == 0 && top.is_none()) {
        return 2;
    }
    let dc = [2; 16];
    let (a, b) = neighbours(
        modes,
        top.map(|n| n.modes.as_ref().unwrap_or(&dc)),
        left.map(|n| n.modes.as_ref().unwrap_or(&dc)),
        index,
        4,
    );
    a.unwrap_or(2).min(b.unwrap_or(2))
}

fn mode_bits(w: &mut Bits, mode: u8, predicted: u8) -> Result<()> {
    w.flag(mode == predicted)?;
    if mode != predicted {
        w.u(3, u64::from(if mode < predicted { mode } else { mode - 1 }))?;
    }
    Ok(())
}

fn patterns(luma: &Luma, chroma: &Chroma, inter: bool) -> (u8, u8) {
    let mut y = 0;
    for (k, levels) in luma.levels.iter().enumerate() {
        if levels
            .iter()
            .skip(usize::from(!inter && luma.mode.is_some()))
            .any(|&x| x != 0)
        {
            y |= 1 << (k / 4);
        }
    }
    if !inter && luma.mode.is_some() && y != 0 {
        y = 15;
    }
    let c = if chroma
        .levels
        .iter()
        .flatten()
        .any(|l| l.iter().skip(1).any(|&x| x != 0))
    {
        2
    } else if chroma.dc.iter().flatten().any(|&x| x != 0) {
        1
    } else {
        0
    };
    (y, c)
}

#[derive(Clone, Copy)]
struct MbContext<'a> {
    top: Option<&'a Neighbour>,
    left: Option<&'a Neighbour>,
    p_slice: bool,
}

fn macroblock(
    w: &mut Bits,
    luma: &Luma,
    chroma: &Chroma,
    prediction: Option<&inter::Inter>,
    context: MbContext<'_>,
) -> Result<Neighbour> {
    macroblock_syntax(w, luma, chroma, prediction, context, None)
}

fn macroblock_syntax(
    w: &mut Bits,
    luma: &Luma,
    chroma: &Chroma,
    prediction: Option<&inter::Inter>,
    context: MbContext<'_>,
    costs: Option<&levels::ResidualCosts>,
) -> Result<Neighbour> {
    let start = w.len;
    let MbContext { top, left, p_slice } = context;
    let mut state = Neighbour::default();
    if let Some(inter) = prediction {
        state.motion = inter.motion()?;
        if inter.skip {
            return Ok(state);
        }
        inter.syntax(w)?;
    }
    let intra16 = prediction.is_none() && luma.mode.is_some();
    let intra4 = prediction.is_none() && !intra16;
    let (cbpy, cbpc) = costs.map_or_else(
        || patterns(luma, chroma, prediction.is_some()),
        |costs| costs.patterns(intra16),
    );
    if prediction.is_none() {
        w.ue(u32::from(p_slice) * P_INTRA_TYPE_BASE
            + luma.mode.map_or(0, |m| {
                1 + u32::from(m) + 4 * u32::from(cbpc) + if cbpy == 0 { 0 } else { 12 }
            }))?;
    }
    if intra4 {
        let mut modes = [2; 16];
        for k in 0..16 {
            let (x, y) = xy(k);
            let p = y / 4 * 4 + x / 4;
            let mode = *luma.modes.get(p).ok_or(Error::Internal)?;
            mode_bits(w, mode, predicted_mode(&modes, top, left, p))?;
            *modes.get_mut(p).ok_or(Error::Internal)? = mode;
        }
    }
    if prediction.is_none() {
        w.ue(u32::from(chroma.mode))?;
    }
    if !intra16 {
        let pattern = cbpy + 16 * cbpc;
        let code = (if intra4 { &INTRA_CBP } else { &INTER_CBP })
            .iter()
            .position(|&p| p == pattern)
            .ok_or(Error::Internal)?;
        w.ue(code as u32)?;
    }
    state.modes = if intra4 { Some(luma.modes) } else { None };
    if intra16 || cbpy != 0 || cbpc != 0 {
        w.se(0)?;
    }
    if intra16 {
        let nc = nc(&[0; 16], top, left, 0, 0);
        if let Some(costs) = costs {
            costs.count(w, 0, nc)?;
        } else {
            cavlc::residual(w, &t::scan(&luma.dc)?, nc)?;
        }
    }
    let counts = state.counts.first_mut().ok_or(Error::Internal)?;
    for (k, levels) in luma.levels.iter().enumerate() {
        let (x, y) = xy(k);
        let p = y / 4 * 4 + x / 4;
        if cbpy & (1 << (k / 4)) != 0 {
            let nc = nc(counts, top, left, 0, p);
            let count = if let Some(costs) = costs {
                costs.count(w, 1 + k, nc)?
            } else {
                let scan = t::scan(levels)?;
                cavlc::residual(
                    w,
                    scan.get(usize::from(intra16)..).ok_or(Error::Internal)?,
                    nc,
                )?
            };
            *counts.get_mut(p).ok_or(Error::Internal)? = count;
        }
    }
    if cbpc != 0 {
        for (c, dc) in chroma.dc.iter().enumerate() {
            if let Some(costs) = costs {
                costs.count(w, 17 + c, -1)?;
            } else {
                cavlc::residual(w, dc, -1)?;
            }
        }
    }
    if cbpc == 2 {
        for (c, blocks) in chroma.levels.iter().enumerate() {
            let counts = state.counts.get_mut(c + 1).ok_or(Error::Internal)?;
            for (k, levels) in blocks.iter().enumerate() {
                let nc = nc(counts, top, left, c + 1, k);
                let count = if let Some(costs) = costs {
                    costs.count(w, 19 + c * 4 + k, nc)?
                } else {
                    let scan = t::scan(levels)?;
                    cavlc::residual(w, scan.get(1..).ok_or(Error::Internal)?, nc)?
                };
                *counts.get_mut(k).ok_or(Error::Internal)? = count;
            }
        }
    }
    if w.len - start > 3200 {
        return Err(Error::MacroblockLimit);
    }
    Ok(state)
}

fn intra16(base: &Tile, source: &Tile, a: IntraAvail, mode: u8, qp: u8) -> Result<Luma> {
    let mut out = Luma {
        mode: Some(mode),
        modes: [2; 16],
        levels: [[0; 16]; 16],
        dc: [0; 16],
        tile: base.clone(),
    };
    out.tile.prediction(0, 0, 16, a, mode)?;
    let quant = t::Quant::intra(qp)?;
    let mut sums = [0; 16];
    for (k, levels) in out.levels.iter_mut().enumerate() {
        let (x, y) = xy(k);
        let coefficients = out.tile.forward(source, x, y)?;
        *sums.get_mut(y / 4 * 4 + x / 4).ok_or(Error::Internal)? = coefficients.dc();
        *levels = quant.levels(&coefficients, true);
    }
    let (levels, dc) = t::luma_dc(sums, qp)?;
    out.dc = levels;
    for (k, levels) in out.levels.iter().enumerate() {
        let (x, y) = xy(k);
        let dc = *dc.get(y / 4 * 4 + x / 4).ok_or(Error::Internal)?;
        out.tile.reconstruct(x, y, levels, quant, Some(dc))?;
    }
    Ok(out)
}

fn chroma(base: &[Tile; 2], source: &[Tile; 2], a: IntraAvail, mode: u8, qp: u8) -> Result<Chroma> {
    let mut predicted = base.clone();
    for tile in &mut predicted {
        tile.prediction(0, 0, 8, a, mode)?;
    }
    let mut out = chroma_code(&predicted, source, qp, t::Quant::chroma_intra(qp)?)?;
    out.mode = mode;
    Ok(out)
}

fn chroma_residual(base: &[Tile; 2], source: &[Tile; 2], qp: u8) -> Result<Chroma> {
    chroma_code(base, source, qp, t::Quant::chroma_inter(qp)?)
}

fn chroma_code(base: &[Tile; 2], source: &[Tile; 2], qp: u8, quant: &t::Quant) -> Result<Chroma> {
    let mut sums = [[0; 4]; 2];
    let mut levels = [[[0; 16]; 4]; 2];
    for (((tile, original), sums), levels) in
        base.iter().zip(source).zip(&mut sums).zip(&mut levels)
    {
        for (k, (sum, levels)) in sums.iter_mut().zip(levels).enumerate() {
            let coefficients = tile.forward(original, k % 2 * 4, k / 2 * 4)?;
            *sum = coefficients.dc();
            *levels = quant.levels(&coefficients, true);
        }
    }
    chroma_finish(base, &sums, levels, qp, quant)
}

#[cfg(test)]
fn chroma_reconstruct(
    base: &[Tile; 2],
    sums: &[[i32; 4]; 2],
    levels: [[[i32; 16]; 4]; 2],
    qp: u8,
) -> Result<Chroma> {
    chroma_finish(base, sums, levels, qp, t::Quant::chroma_inter(qp)?)
}

fn chroma_finish(
    base: &[Tile; 2],
    sums: &[[i32; 4]; 2],
    levels: [[[i32; 16]; 4]; 2],
    qp: u8,
    quant: &t::Quant,
) -> Result<Chroma> {
    let mut out = Chroma {
        mode: 0,
        levels,
        dc: [[0; 4]; 2],
        tiles: base.clone(),
    };
    for (((tile, &sums), dc_levels), blocks) in out
        .tiles
        .iter_mut()
        .zip(sums)
        .zip(&mut out.dc)
        .zip(&out.levels)
    {
        let (levels, dc) = quant.chroma_dc(sums, qp)?;
        *dc_levels = levels;
        for (k, (levels, &dc)) in blocks.iter().zip(&dc).enumerate() {
            tile.reconstruct(k % 2 * 4, k / 2 * 4, levels, quant, Some(dc))?;
        }
    }
    Ok(out)
}

fn intra4(
    base: &Tile,
    source: &Tile,
    a: IntraAvail,
    qp: u8,
    top: Option<&Neighbour>,
    left: Option<&Neighbour>,
    trials: Trials,
) -> Result<(u64, Luma)> {
    let mut out = Luma {
        mode: None,
        modes: [2; 16],
        levels: [[0; 16]; 16],
        dc: [0; 16],
        tile: base.clone(),
    };
    let mut counts = [0; 16];
    let quant = t::Quant::intra(qp)?;
    let lambda = 1u64 << (qp / 3);
    let mut cheap_score = 0;
    for k in 0..16 {
        let (x, y) = xy(k);
        let p = y / 4 * 4 + x / 4;
        let a = availability(k, a);
        let predicted = predicted_mode(&out.modes, top, left, p);
        let mut ranked = [UNRANKED; 9];
        let mut tile = out.tile.clone();
        for (slot, mode) in ranked.iter_mut().zip(0..9) {
            if !legal(mode, 4, a) {
                continue;
            }
            tile.prediction(x, y, 4, a, mode)?;
            let mut bits = Bits::count();
            mode_bits(&mut bits, mode, predicted)?;
            *slot = (
                tile.satd(source, x, y, 4)? + (1u64 << (qp / 6)) * bits.len as u64,
                mode,
            );
        }
        ranked.sort_unstable();
        if trials == Trials::Cheap {
            let (score, mode) = *ranked
                .first()
                .filter(|r| **r != UNRANKED)
                .ok_or(Error::Internal)?;
            cheap_score += score;
            out.tile.prediction(x, y, 4, a, mode)?;
            let levels = out.tile.code(source, x, y, quant)?;
            let count = cavlc::residual(
                &mut Bits::count(),
                &t::scan(&levels)?,
                nc(&counts, top, left, 0, p),
            )?;
            *out.modes.get_mut(p).ok_or(Error::Internal)? = mode;
            *out.levels.get_mut(k).ok_or(Error::Internal)? = levels;
            *counts.get_mut(p).ok_or(Error::Internal)? = count;
            continue;
        }
        let mut best = None;
        let mut completed = 0;
        for &(_, mode) in ranked.iter().filter(|r| **r != UNRANKED) {
            if trials == Trials::Shortlist && completed == FINALISTS {
                break;
            }
            let result = (|| {
                tile.prediction(x, y, 4, a, mode)?;
                let levels = tile.code(source, x, y, quant)?;
                let mut bits = Bits::count();
                mode_bits(&mut bits, mode, predicted)?;
                let count =
                    cavlc::residual(&mut bits, &t::scan(&levels)?, nc(&counts, top, left, 0, p))?;
                let score = tile.error(source, x, y, 4)? + lambda * bits.len as u64;
                let mut block = [0; 16];
                for (j, row) in block.chunks_exact_mut(4).enumerate() {
                    let at = Tile::at(x, y + j);
                    row.copy_from_slice(tile.0.get(at..at + 4).ok_or(Error::Internal)?);
                }
                Ok::<_, Error>(((score, bits.len, mode), block, levels, count))
            })();
            match result {
                Ok(candidate) => {
                    completed += 1;
                    if best.as_ref().is_none_or(
                        |b: &((u64, usize, u8), [u8; 16], [i32; 16], u8)| candidate.0 < b.0,
                    ) {
                        best = Some(candidate);
                    }
                }
                Err(Error::Coefficient) => {}
                Err(e) => return Err(e),
            }
        }
        let ((_, _, mode), block, levels, count) = best.ok_or(Error::Coefficient)?;
        for (j, row) in block.chunks_exact(4).enumerate() {
            let at = Tile::at(x, y + j);
            out.tile
                .0
                .get_mut(at..at + 4)
                .ok_or(Error::Internal)?
                .copy_from_slice(row);
        }
        *out.modes.get_mut(p).ok_or(Error::Internal)? = mode;
        *out.levels.get_mut(k).ok_or(Error::Internal)? = levels;
        *counts.get_mut(p).ok_or(Error::Internal)? = count;
    }
    Ok((cheap_score, out))
}

pub(crate) struct FramePlane {
    data: Vec<u8>,
    width: usize,
    height: usize,
}
impl FramePlane {
    fn new(width: usize, height: usize) -> Result<Self> {
        Ok(Self {
            data: zeros(width * height)?,
            width,
            height,
        })
    }
    #[cfg(test)]
    fn sample(&self, x: i32, y: i32) -> Result<u8> {
        self.data
            .get(
                y.clamp(0, self.height as i32 - 1) as usize * self.width
                    + x.clamp(0, self.width as i32 - 1) as usize,
            )
            .copied()
            .ok_or(Error::Internal)
    }
    fn row(&self, x: i32, y: i32, out: &mut [u8]) -> Result<()> {
        let start = y.clamp(0, self.height as i32 - 1) as usize * self.width;
        let row = self
            .data
            .get(start..start + self.width)
            .ok_or(Error::Internal)?;
        if x >= 0 && x as usize + out.len() <= self.width {
            out.copy_from_slice(
                row.get(x as usize..x as usize + out.len())
                    .ok_or(Error::Internal)?,
            );
            return Ok(());
        }
        for (i, value) in out.iter_mut().enumerate() {
            *value = *row
                .get((x + i as i32).clamp(0, self.width as i32 - 1) as usize)
                .ok_or(Error::Internal)?;
        }
        Ok(())
    }
    #[cfg(test)]
    fn tiles(&self, source: Plane<'_>, x: usize, y: usize, n: usize) -> Result<(Tile, Tile)> {
        let mut base = Tile::empty();
        let mut input = Tile::empty();
        if y != 0 {
            for i in 0..(n * 2).min(self.width - x) {
                *base.0.get_mut(ORIGIN - STRIDE + i).ok_or(Error::Internal)? = *self
                    .data
                    .get((y - 1) * self.width + x + i)
                    .ok_or(Error::Internal)?;
            }
        }
        if x != 0 {
            for j in 0..n {
                *base
                    .0
                    .get_mut(ORIGIN - 1 + j * STRIDE)
                    .ok_or(Error::Internal)? = *self
                    .data
                    .get((y + j) * self.width + x - 1)
                    .ok_or(Error::Internal)?;
            }
        }
        if x != 0 && y != 0 {
            *base.0.get_mut(ORIGIN - STRIDE - 1).ok_or(Error::Internal)? = *self
                .data
                .get((y - 1) * self.width + x - 1)
                .ok_or(Error::Internal)?;
        }
        for j in 0..n {
            for i in 0..n {
                *input.0.get_mut(Tile::at(i, j)).ok_or(Error::Internal)? =
                    source.sample(x + i, y + j)?;
            }
        }
        Ok((base, input))
    }
    fn commit(&mut self, tile: &Tile, x: usize, y: usize, n: usize) -> Result<()> {
        for j in 0..n {
            let start = (y + j) * self.width + x;
            let at = Tile::at(0, j);
            self.data
                .get_mut(start..start + n)
                .ok_or(Error::Internal)?
                .copy_from_slice(tile.0.get(at..at + n).ok_or(Error::Internal)?);
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CandidateKey {
    cost: u64,
    bits: usize,
    not_skip: bool,
    partitions: usize,
    motion: [Mv; 16],
    mode: u32,
    subshapes: [u8; 4],
    intra_mode: u8,
    chroma_mode: u8,
}

#[derive(Clone)]
struct Candidate {
    key: CandidateKey,
    luma: Luma,
    chroma: Chroma,
    inter: Option<inter::Inter>,
    state: Neighbour,
}

struct Decision<'a> {
    source: &'a Tile,
    csources: &'a [Tile; 2],
    context: MbContext<'a>,
    qp: u8,
    pending_skip: u32,
}

impl Decision<'_> {
    fn cost(&self, distortion: u64, bits: usize) -> Result<u64> {
        let weight = if self.context.p_slice {
            t::rate_weight(self.qp)?
        } else {
            512 * (1u64 << (self.qp / 3))
        };
        Ok(512 * distortion + weight * bits as u64)
    }

    fn candidate(
        &self,
        luma: Luma,
        chroma: Chroma,
        inter: Option<inter::Inter>,
        choice: u8,
    ) -> Result<Candidate> {
        #[cfg(test)]
        if inter.is_none() {
            tests::INTRA_PAIRS.with(|count| count.set(count.get() + 1));
        }
        let mut counter = Bits::count();
        let skip = inter.as_ref().is_some_and(|i| i.skip);
        if self.context.p_slice {
            counter.ue(self.pending_skip + u32::from(skip))?;
        }
        let state = macroblock(&mut counter, &luma, &chroma, inter.as_ref(), self.context)?;
        let mut distortion = luma.tile.error(self.source, 0, 0, 16)?;
        for (tile, source) in chroma.tiles.iter().zip(self.csources) {
            distortion += tile.error(source, 0, 0, 8)?;
        }
        let (count, vectors, code, shapes) = if let Some(i) = &inter {
            (
                i.len,
                i.motion()?.vectors,
                i.layout.code(),
                match i.layout {
                    motion::Layout::Sub(s) => s,
                    _ => [0; 4],
                },
            )
        } else {
            (
                if choice == 4 { 16 } else { 1 },
                [Mv::default(); 16],
                5,
                [0; 4],
            )
        };
        Ok(Candidate {
            key: CandidateKey {
                cost: self.cost(distortion, counter.len)?,
                bits: counter.len,
                not_skip: !skip,
                partitions: count,
                motion: vectors,
                mode: code,
                subshapes: shapes,
                intra_mode: choice,
                chroma_mode: chroma.mode,
            },
            luma,
            chroma,
            inter,
            state,
        })
    }

    fn ranked_modes(
        &self,
        base: &Tile,
        cbases: &[Tile; 2],
        a: IntraAvail,
    ) -> Result<(Ranked, Ranked)> {
        let mut lumas = [UNRANKED; 4];
        let mut chromas = [UNRANKED; 4];
        for (mode, (luma, chroma)) in (0..4).zip(lumas.iter_mut().zip(chromas.iter_mut())) {
            if legal(mode, 16, a) {
                let mut tile = base.clone();
                tile.prediction(0, 0, 16, a, mode)?;
                *luma = (tile.satd(self.source, 0, 0, 16)?, mode);
            }
            if legal(mode, 8, a) {
                let mut bits = Bits::count();
                bits.ue(u32::from(mode))?;
                let mut score = (1u64 << (self.qp / 6)) * bits.len as u64;
                for (base, source) in cbases.iter().zip(self.csources) {
                    let mut tile = base.clone();
                    tile.prediction(0, 0, 8, a, mode)?;
                    score += tile.satd(source, 0, 0, 8)?;
                }
                *chroma = (score, mode);
            }
        }
        lumas.sort_unstable();
        chromas.sort_unstable();
        Ok((lumas, chromas))
    }

    fn quick_intra(
        &self,
        base: &Tile,
        cbases: &[Tile; 2],
        a: IntraAvail,
        four: bool,
    ) -> Result<Candidate> {
        let (lumas, chromas) = self.ranked_modes(base, cbases, a)?;
        let luma = *lumas
            .first()
            .filter(|r| **r != UNRANKED)
            .ok_or(Error::Internal)?;
        let luma_mode = luma.1;
        let chroma_mode = chromas
            .first()
            .filter(|r| **r != UNRANKED)
            .ok_or(Error::Internal)?
            .1;
        let four = if four {
            Some(intra4(
                base,
                self.source,
                a,
                self.qp,
                self.context.top,
                self.context.left,
                Trials::Cheap,
            )?)
        } else {
            None
        };
        let (luma, choice) = match four {
            Some((score, four)) if score < luma.0 => (four, 4),
            _ => (
                intra16(base, self.source, a, luma_mode, self.qp)?,
                luma_mode,
            ),
        };
        self.candidate(
            luma,
            chroma(cbases, self.csources, a, chroma_mode, t::chroma_qp(self.qp))?,
            None,
            choice,
        )
    }

    fn intra(
        &self,
        base: &Tile,
        cbases: &[Tile; 2],
        a: IntraAvail,
        scope: IntraScope,
        best: &mut Option<Candidate>,
        refusal: &mut Error,
    ) -> Result<()> {
        if scope == IntraScope::None {
            return Ok(());
        }
        let exhaustive = scope == IntraScope::Exhaustive;
        if !exhaustive && self.context.p_slice && best.is_some() {
            let mut floor = Bits::count();
            floor.ue(self.pending_skip)?;
            floor.ue(P_INTRA_TYPE_BASE)?;
            floor.ue(0)?;
            let cost = self.cost(0, floor.len)?;
            if best
                .as_ref()
                .is_some_and(|candidate| candidate.key.cost < cost)
            {
                return Ok(());
            }
        }
        if !exhaustive {
            match self.quick_intra(base, cbases, a, scope == IntraScope::Quick) {
                Ok(candidate) => {
                    consider(best, candidate);
                    return Ok(());
                }
                Err(Error::Coefficient | Error::Syntax) => {}
                Err(Error::MacroblockLimit) => *refusal = Error::MacroblockLimit,
                Err(err) => return Err(err),
            }
        }
        if exhaustive {
            self.complete(base, cbases, a, true, best, refusal)?;
            if best.is_some() {
                return Ok(());
            }
        }
        self.complete(base, cbases, a, false, best, refusal)
    }

    fn complete(
        &self,
        base: &Tile,
        cbases: &[Tile; 2],
        a: IntraAvail,
        shortlist: bool,
        best: &mut Option<Candidate>,
        refusal: &mut Error,
    ) -> Result<()> {
        let (limit, (lumas, chroma_ranks)) = if shortlist {
            (FINALISTS, self.ranked_modes(base, cbases, a)?)
        } else {
            let order = |n| {
                [0, 1, 2, 3].map(|mode| {
                    if legal(mode, n, a) {
                        (0, mode)
                    } else {
                        UNRANKED
                    }
                })
            };
            (usize::MAX, (order(16), order(8)))
        };
        let mut chromas: [Option<Chroma>; 4] = [None, None, None, None];
        let mut filled = 0;
        for &(_, mode) in chroma_ranks.iter().filter(|r| **r != UNRANKED) {
            if filled == limit {
                break;
            }
            let Some(slot) = chromas.get_mut(filled) else {
                break;
            };
            match chroma(cbases, self.csources, a, mode, t::chroma_qp(self.qp)) {
                Ok(c) => {
                    *slot = Some(c);
                    filled += 1;
                }
                Err(Error::Coefficient) => {}
                Err(err) => return Err(err),
            }
        }
        let mut sixteens = 0;
        let choices = lumas
            .iter()
            .filter(|r| **r != UNRANKED)
            .map(|r| r.1)
            .chain([4]);
        for choice in choices {
            let luma = if choice == 4 {
                intra4(
                    base,
                    self.source,
                    a,
                    self.qp,
                    self.context.top,
                    self.context.left,
                    if shortlist {
                        Trials::Shortlist
                    } else {
                        Trials::Every
                    },
                )
                .map(|(_, luma)| luma)
            } else {
                if sixteens == limit {
                    continue;
                }
                intra16(base, self.source, a, choice, self.qp)
            };
            let luma = match luma {
                Ok(y) => y,
                Err(Error::Coefficient) => continue,
                Err(err) => return Err(err),
            };
            if choice != 4 {
                sixteens += 1;
            }
            for chroma in chromas.iter().flatten() {
                match self.candidate(luma.clone(), chroma.clone(), None, choice) {
                    Ok(candidate) => consider(best, candidate),
                    Err(Error::Coefficient | Error::Syntax) => continue,
                    Err(Error::MacroblockLimit) => {
                        *refusal = Error::MacroblockLimit;
                        continue;
                    }
                    Err(err) => return Err(err),
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IntraScope {
    None,
    Sixteen,
    Quick,
    Exhaustive,
}

fn consider(best: &mut Option<Candidate>, candidate: Candidate) {
    if best.as_ref().is_none_or(|b| candidate.key < b.key) {
        *best = Some(candidate);
    }
}

pub(crate) struct EncodedPicture {
    pub(crate) nal: Vec<u8>,
    pub(crate) reconstruction: Vec<u8>,
    pub(crate) planes: [FramePlane; 3],
    pub(crate) previous_mv_count: u8,
}

pub(crate) fn picture(
    e: &Encoder,
    input: Picture<'_>,
    slice: syntax::Slice,
) -> Result<EncodedPicture> {
    let mut planes = [
        FramePlane::new(e.coded_width, e.coded_height)?,
        FramePlane::new(e.coded_width / 2, e.coded_height / 2)?,
        FramePlane::new(e.coded_width / 2, e.coded_height / 2)?,
    ];
    let mb_width = e.coded_width / 16;
    let mut writer = Bits::new(e.limits.access_unit_bytes);
    syntax::slice(&mut writer, e, slice)?;
    let candidates = rows::decide(e, input, slice)?;
    let mut pending_skip = 0;
    let mut previous_mv_count = e.previous_mv_count;
    for (address, slot) in candidates.iter().enumerate() {
        let candidate = slot.get().ok_or(Error::Internal)?;
        let Candidate {
            luma,
            chroma,
            inter,
            ..
        } = candidate;
        let (mx, my) = (address % mb_width, address / mb_width);
        let context = rows::context(&candidates, address, mb_width, !slice.idr)?;
        let skip = inter.as_ref().is_some_and(|i| i.skip);
        if !slice.idr {
            if skip {
                pending_skip += 1;
            } else {
                writer.ue(pending_skip)?;
                pending_skip = 0;
            }
        }
        macroblock(&mut writer, luma, chroma, inter.as_ref(), context)?;
        previous_mv_count = inter.as_ref().map_or(0, |i| i.len as u8);
        planes
            .first_mut()
            .ok_or(Error::Internal)?
            .commit(&luma.tile, mx * 16, my * 16, 16)?;
        for (c, tile) in chroma.tiles.iter().enumerate() {
            planes
                .get_mut(c + 1)
                .ok_or(Error::Internal)?
                .commit(tile, mx * 8, my * 8, 8)?;
        }
    }
    if pending_skip != 0 {
        writer.ue(pending_skip)?;
    }
    if e.settings.deblocking == Deblocking::Enabled {
        for (c, plane) in planes.iter_mut().enumerate() {
            let qp = if c == 0 {
                e.settings.qp
            } else {
                t::chroma_qp(e.settings.qp)
            };
            deblock::picture(
                &mut plane.data,
                plane.width,
                plane.height,
                qp,
                c != 0,
                |mx, my| {
                    let n = &candidates
                        .get(my * mb_width + mx)
                        .and_then(std::sync::OnceLock::get)
                        .ok_or(Error::Internal)?
                        .state;
                    let mut nonzero = 0;
                    for (p, &count) in n.counts.first().ok_or(Error::Internal)?.iter().enumerate() {
                        if count != 0 {
                            nonzero |= 1 << p;
                        }
                    }
                    Ok(deblock::Record {
                        motion: n.motion,
                        nonzero,
                    })
                },
            )?;
        }
    }
    let visible = e.settings.width * e.settings.height * 3 / 2;
    let mut reconstruction = zeros(visible)?;
    let mut offset = 0;
    for (c, plane) in planes.iter().enumerate() {
        let scale = if c == 0 { 1 } else { 2 };
        let width = e.settings.width / scale;
        let height = e.settings.height / scale;
        if height > plane.height {
            return Err(Error::Internal);
        }
        for y in 0..height {
            reconstruction
                .get_mut(offset..offset + width)
                .ok_or(Error::Internal)?
                .copy_from_slice(
                    plane
                        .data
                        .get(y * plane.width..y * plane.width + width)
                        .ok_or(Error::Internal)?,
                );
            offset += width;
        }
    }
    Ok(EncodedPicture {
        nal: writer.nal(if slice.idr { 0x65 } else { 0x61 })?,
        reconstruction,
        planes,
        previous_mv_count,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    std::thread_local! {
        pub(super) static INTRA_PAIRS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    #[test]
    fn complete_cost_uses_quantizer_energy_and_keeps_idr_ordering() {
        let source = Tile::empty();
        let csources = [Tile::empty(), Tile::empty()];
        for qp in 10..=51 {
            for p_slice in [false, true] {
                let decision = Decision {
                    source: &source,
                    csources: &csources,
                    context: MbContext {
                        top: None,
                        left: None,
                        p_slice,
                    },
                    qp,
                    pending_skip: 0,
                };
                let v = [10u64, 11, 13, 14, 16, 18][usize::from(qp % 6)];
                let weight = if p_slice {
                    (v * v * (1 << (2 * (qp / 6)))) / 4
                } else {
                    512 * (1 << (qp / 3))
                };
                for bits in [1, 9, 53, 3233] {
                    let floor = decision.cost(0, bits).unwrap();
                    assert_eq!(floor, weight * bits as u64);
                    assert_eq!(decision.cost(1, bits).unwrap(), floor + 512);
                    assert_eq!(
                        decision.cost(384 * 255 * 255, bits).unwrap(),
                        floor + 512 * 384 * 255 * 255
                    );
                    if p_slice && qp <= 45 {
                        let higher = Decision {
                            qp: qp + 6,
                            ..decision
                        };
                        assert_eq!(higher.cost(0, bits).unwrap(), floor * 4);
                    }
                }
            }
        }
    }

    #[test]
    fn cheap_intra_four_uses_only_the_chosen_reconstructed_borders() {
        let base = Tile::empty();
        let csources = [Tile::empty(), Tile::empty()];
        let a = intra_availability(Border::default());
        let mut selected_four = false;
        for pattern in 0..8 {
            let mut source = Tile::empty();
            for y in 0..16 {
                for x in 0..16 {
                    source.0[Tile::at(x, y)] =
                        ((x * (17 + pattern) + y * (7 + pattern * 3)) % 256) as u8;
                }
            }
            let (score, luma) = intra4(&base, &source, a, 28, None, None, Trials::Cheap).unwrap();
            let mut replay = base.clone();
            let mut modes = [2; 16];
            let mut expected_score = 0;
            for k in 0..16 {
                let (x, y) = xy(k);
                let p = y / 4 * 4 + x / 4;
                let available = availability(k, a);
                let predicted = predicted_mode(&modes, None, None, p);
                let mut best = None;
                for mode in 0..9 {
                    if !legal(mode, 4, available) {
                        continue;
                    }
                    let mut tile = replay.clone();
                    tile.prediction(x, y, 4, available, mode).unwrap();
                    let mut bits = Bits::count();
                    mode_bits(&mut bits, mode, predicted).unwrap();
                    let key = (
                        tile.satd(&source, x, y, 4).unwrap() + 16 * bits.len as u64,
                        mode,
                    );
                    if best.is_none_or(|old| key < old) {
                        best = Some(key);
                    }
                }
                let (cost, mode) = best.unwrap();
                assert_eq!(luma.modes[p], mode);
                expected_score += cost;
                replay.prediction(x, y, 4, available, mode).unwrap();
                replay
                    .reconstruct(x, y, &luma.levels[k], t::Quant::of(28).unwrap(), None)
                    .unwrap();
                modes[p] = mode;
            }
            assert_eq!(expected_score, score);
            assert_eq!(replay.0, luma.tile.0);
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
            INTRA_PAIRS.with(|count| count.set(0));
            let candidate = decision.quick_intra(&base, &csources, a, true).unwrap();
            selected_four |= candidate.luma.mode.is_none();
            assert_eq!(INTRA_PAIRS.with(|count| count.get()), 1);
            let sixteen = decision.quick_intra(&base, &csources, a, false).unwrap();
            assert!(sixteen.luma.mode.is_some_and(|mode| legal(mode, 16, a)));
            if candidate.luma.mode.is_some() {
                assert!(sixteen.key == candidate.key);
            }
        }
        assert!(selected_four);
    }

    #[test]
    fn cheap_intra_completes_one_legal_pair_for_every_availability_and_qp_boundary() {
        let base = Tile::empty();
        let csources = [Tile::empty(), Tile::empty()];
        for qp in [10, 16, 28, 40, 51] {
            let decision = Decision {
                source: &base,
                csources: &csources,
                context: MbContext {
                    top: None,
                    left: None,
                    p_slice: true,
                },
                qp,
                pending_skip: 0,
            };
            for mask in 0..16 {
                let a = IntraAvail {
                    left: mask & 1 != 0,
                    top: mask & 2 != 0,
                    top_left: mask & 4 != 0,
                    top_right: mask & 8 != 0,
                };
                INTRA_PAIRS.with(|count| count.set(0));
                let mut best = None;
                decision
                    .intra(
                        &base,
                        &csources,
                        a,
                        IntraScope::Quick,
                        &mut best,
                        &mut Error::Coefficient,
                    )
                    .unwrap();
                let best = best.unwrap();
                assert!(legal(best.luma.mode.unwrap(), 16, a));
                assert!(legal(best.chroma.mode, 8, a));
                assert_eq!(INTRA_PAIRS.with(|count| count.get()), 1);
                assert_eq!(best.luma.tile.0, base.0);
            }
        }
    }

    #[test]
    fn rejected_cheap_intra_runs_the_complete_admission_fallback() {
        let base = Tile::empty();
        let mut source = Tile::empty();
        let mut csources = [Tile::empty(), Tile::empty()];
        let mut state = 17u32;
        for (n, tile) in
            std::iter::once((16, &mut source)).chain(csources.iter_mut().map(|tile| (8, tile)))
        {
            for y in 0..n {
                for x in 0..n {
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    *tile.0.get_mut(Tile::at(x, y)).unwrap() = state as u8;
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
            qp: 10,
            pending_skip: 0,
        };
        let a = intra_availability(Border::default());
        assert!(matches!(
            decision.quick_intra(&base, &[base.clone(), base.clone()], a, true),
            Err(Error::MacroblockLimit)
        ));
        INTRA_PAIRS.with(|count| count.set(0));
        let mut best = None;
        let mut refusal = Error::Coefficient;
        decision
            .intra(
                &base,
                &[base.clone(), base.clone()],
                a,
                IntraScope::Quick,
                &mut best,
                &mut refusal,
            )
            .unwrap();
        assert!(best.is_none());
        assert_eq!(refusal, Error::MacroblockLimit);
        assert!(INTRA_PAIRS.with(|count| count.get()) > 1);
    }

    #[test]
    fn constrained_intra_availability_is_independent_of_residual_neighbours() {
        let intra = MotionMb::default();
        let inter = MotionMb {
            intra: false,
            ..MotionMb::default()
        };
        for mask in 0..16 {
            let mb = |bit| if mask & bit != 0 { &intra } else { &inter };
            let a = intra_availability(Border {
                left: Some(mb(1)),
                top: Some(mb(2)),
                top_left: Some(mb(4)),
                top_right: Some(mb(8)),
            });
            assert_eq!(
                [a.left, a.top, a.top_left, a.top_right],
                [mask & 1 != 0, mask & 2 != 0, mask & 4 != 0, mask & 8 != 0]
            );
        }
        let unavailable = intra_availability(Border::default());
        assert!(
            !unavailable.top
                && !unavailable.left
                && !unavailable.top_left
                && !unavailable.top_right
        );
        for k in 0..16 {
            let (x, y) = xy(k);
            let a = availability(k, unavailable);
            assert_eq!(a.left, x != 0);
            assert_eq!(a.top, y != 0);
            assert_eq!(a.top_left, x != 0 && y != 0);
            assert_eq!(
                a.top_right,
                y != 0 && x < 12 && order(x / 4 + 1, y / 4 - 1) < k
            );
        }
        let top = Neighbour {
            counts: [[7; 16]; 3],
            modes: Some([0; 16]),
            motion: inter,
        };
        let left = Neighbour {
            counts: [[11; 16]; 3],
            modes: Some([1; 16]),
            motion: intra,
        };
        assert_eq!(predicted_mode(&[1; 16], Some(&top), Some(&left), 0), 2);
        assert_eq!(predicted_mode(&[1; 16], Some(&top), Some(&left), 1), 2);
        assert_eq!(predicted_mode(&[1; 16], Some(&top), Some(&left), 4), 1);
        assert_eq!(nc(&[0; 16], Some(&top), Some(&left), 0, 0), 9);
    }
}
