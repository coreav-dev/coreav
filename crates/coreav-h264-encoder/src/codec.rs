use crate::{
    Deblocking, Encoder, Error, Picture, Plane, Result, api::zeros, bits::Bits, cavlc, deblock,
    syntax, transform as t, vlc::INTRA_CBP,
};
use coreav_kernels_h264::{BlockMut, IntraAvail, IntraMut, Kernels, Scalar};

const STRIDE: usize = 40;
const ORIGIN: usize = STRIDE + 1;

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
    fn residual(&self, source: &Self, x: usize, y: usize) -> Result<[i32; 16]> {
        let mut out = [0; 16];
        for (p, v) in out.iter_mut().enumerate() {
            let at = Self::at(x + p % 4, y + p / 4);
            *v = i32::from(*source.0.get(at).ok_or(Error::Internal)?)
                - i32::from(*self.0.get(at).ok_or(Error::Internal)?);
        }
        Ok(out)
    }
    fn add(&mut self, x: usize, y: usize, scaled: &[i32; 16]) -> Result<()> {
        let view = BlockMut::new(&mut self.0, Self::at(x, y), STRIDE, 4, 4)
            .map_err(|_| Error::Internal)?;
        Scalar.idct4x4_add(view, scaled);
        Ok(())
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
}

#[derive(Clone, Default)]
struct Neighbour {
    counts: [[u8; 16]; 3],
    modes: Option<[u8; 16]>,
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

fn patterns(luma: &Luma, chroma: &Chroma) -> (u8, u8) {
    let mut y = 0;
    for (k, levels) in luma.levels.iter().enumerate() {
        if levels
            .iter()
            .skip(usize::from(luma.mode.is_some()))
            .any(|&x| x != 0)
        {
            y |= 1 << (k / 4);
        }
    }
    if luma.mode.is_some() && y != 0 {
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

fn macroblock(
    w: &mut Bits,
    luma: &Luma,
    chroma: &Chroma,
    top: Option<&Neighbour>,
    left: Option<&Neighbour>,
) -> Result<Neighbour> {
    let start = w.len;
    let (cbpy, cbpc) = patterns(luma, chroma);
    w.ue(luma.mode.map_or(0, |m| {
        1 + u32::from(m) + 4 * u32::from(cbpc) + if cbpy == 0 { 0 } else { 12 }
    }))?;
    if luma.mode.is_none() {
        let mut modes = [2; 16];
        for k in 0..16 {
            let (x, y) = xy(k);
            let p = y / 4 * 4 + x / 4;
            let mode = *luma.modes.get(p).ok_or(Error::Internal)?;
            mode_bits(w, mode, predicted_mode(&modes, top, left, p))?;
            *modes.get_mut(p).ok_or(Error::Internal)? = mode;
        }
    }
    w.ue(u32::from(chroma.mode))?;
    if luma.mode.is_none() {
        let pattern = cbpy + 16 * cbpc;
        let code = INTRA_CBP
            .iter()
            .position(|&p| p == pattern)
            .ok_or(Error::Internal)?;
        w.ue(code as u32)?;
    }
    let mut state = Neighbour {
        modes: if luma.mode.is_none() {
            Some(luma.modes)
        } else {
            None
        },
        ..Neighbour::default()
    };
    if luma.mode.is_some() || cbpy != 0 || cbpc != 0 {
        w.se(0)?;
    }
    if luma.mode.is_some() {
        cavlc::residual(w, &t::scan(&luma.dc)?, nc(&[0; 16], top, left, 0, 0))?;
    }
    let counts = state.counts.first_mut().ok_or(Error::Internal)?;
    for (k, levels) in luma.levels.iter().enumerate() {
        let (x, y) = xy(k);
        let p = y / 4 * 4 + x / 4;
        if cbpy & (1 << (k / 4)) != 0 {
            let scan = t::scan(levels)?;
            let count = cavlc::residual(
                w,
                scan.get(usize::from(luma.mode.is_some())..)
                    .ok_or(Error::Internal)?,
                nc(counts, top, left, 0, p),
            )?;
            *counts.get_mut(p).ok_or(Error::Internal)? = count;
        }
    }
    if cbpc != 0 {
        for dc in &chroma.dc {
            cavlc::residual(w, dc, -1)?;
        }
    }
    if cbpc == 2 {
        for (c, blocks) in chroma.levels.iter().enumerate() {
            let counts = state.counts.get_mut(c + 1).ok_or(Error::Internal)?;
            for (k, levels) in blocks.iter().enumerate() {
                let scan = t::scan(levels)?;
                let count = cavlc::residual(
                    w,
                    scan.get(1..).ok_or(Error::Internal)?,
                    nc(counts, top, left, c + 1, k),
                )?;
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
    let mut sums = [0; 16];
    for (k, levels) in out.levels.iter_mut().enumerate() {
        let (x, y) = xy(k);
        let transformed = t::forward(&out.tile.residual(source, x, y)?)?;
        *sums.get_mut(y / 4 * 4 + x / 4).ok_or(Error::Internal)? =
            *transformed.first().ok_or(Error::Internal)?;
        *levels = t::quantize(&transformed, qp, true)?;
    }
    let (levels, dc) = t::luma_dc(sums, qp)?;
    out.dc = levels;
    for (k, levels) in out.levels.iter().enumerate() {
        let (x, y) = xy(k);
        let coefficients = t::scaled(
            levels,
            qp,
            Some(*dc.get(y / 4 * 4 + x / 4).ok_or(Error::Internal)?),
        )?;
        out.tile.add(x, y, &coefficients)?;
    }
    Ok(out)
}

fn chroma(base: &[Tile; 2], source: &[Tile; 2], a: IntraAvail, mode: u8, qp: u8) -> Result<Chroma> {
    let mut out = Chroma {
        mode,
        levels: [[[0; 16]; 4]; 2],
        dc: [[0; 4]; 2],
        tiles: base.clone(),
    };
    for (c, tile) in out.tiles.iter_mut().enumerate() {
        tile.prediction(0, 0, 8, a, mode)?;
        let original = source.get(c).ok_or(Error::Internal)?;
        let blocks = out.levels.get_mut(c).ok_or(Error::Internal)?;
        let mut sums = [0; 4];
        for (k, levels) in blocks.iter_mut().enumerate() {
            let transformed = t::forward(&tile.residual(original, k % 2 * 4, k / 2 * 4)?)?;
            *sums.get_mut(k).ok_or(Error::Internal)? =
                *transformed.first().ok_or(Error::Internal)?;
            *levels = t::quantize(&transformed, qp, true)?;
        }
        let (levels, dc) = t::chroma_dc(sums, qp)?;
        *out.dc.get_mut(c).ok_or(Error::Internal)? = levels;
        for (k, levels) in blocks.iter().enumerate() {
            tile.add(
                k % 2 * 4,
                k / 2 * 4,
                &t::scaled(levels, qp, Some(*dc.get(k).ok_or(Error::Internal)?))?,
            )?;
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
) -> Result<Luma> {
    let mut out = Luma {
        mode: None,
        modes: [2; 16],
        levels: [[0; 16]; 16],
        dc: [0; 16],
        tile: base.clone(),
    };
    let mut counts = [0; 16];
    let lambda = 1u64 << (qp / 3);
    for k in 0..16 {
        let (x, y) = xy(k);
        let p = y / 4 * 4 + x / 4;
        let a = availability(k, a);
        let predicted = predicted_mode(&out.modes, top, left, p);
        let mut best = None;
        for mode in 0..9 {
            if !legal(mode, 4, a) {
                continue;
            }
            let result = (|| {
                let mut tile = out.tile.clone();
                tile.prediction(x, y, 4, a, mode)?;
                let transformed = t::forward(&tile.residual(source, x, y)?)?;
                let levels = t::quantize(&transformed, qp, false)?;
                tile.add(x, y, &t::scaled(&levels, qp, None)?)?;
                let mut bits = Bits::count();
                mode_bits(&mut bits, mode, predicted)?;
                let count =
                    cavlc::residual(&mut bits, &t::scan(&levels)?, nc(&counts, top, left, 0, p))?;
                let score = tile.error(source, x, y, 4)? + lambda * bits.len as u64;
                Ok::<_, Error>(((score, bits.len, mode), tile, levels, count))
            })();
            match result {
                Ok(candidate) => {
                    if best
                        .as_ref()
                        .is_none_or(|b: &((u64, usize, u8), Tile, [i32; 16], u8)| candidate.0 < b.0)
                    {
                        best = Some(candidate);
                    }
                }
                Err(Error::Coefficient) => {}
                Err(e) => return Err(e),
            }
        }
        let ((_, _, mode), tile, levels, count) = best.ok_or(Error::Coefficient)?;
        out.tile = tile;
        *out.modes.get_mut(p).ok_or(Error::Internal)? = mode;
        *out.levels.get_mut(k).ok_or(Error::Internal)? = levels;
        *counts.get_mut(p).ok_or(Error::Internal)? = count;
    }
    Ok(out)
}

struct FramePlane {
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

pub(crate) fn picture(e: &Encoder, input: Picture<'_>, id: u32) -> Result<(Vec<u8>, Vec<u8>)> {
    let mut planes = [
        FramePlane::new(e.coded_width, e.coded_height)?,
        FramePlane::new(e.coded_width / 2, e.coded_height / 2)?,
        FramePlane::new(e.coded_width / 2, e.coded_height / 2)?,
    ];
    let mut row: Vec<Neighbour> = zeros(e.coded_width / 16)?;
    let mut writer = Bits::new(e.limits.access_unit_bytes);
    syntax::slice(&mut writer, e, id)?;
    for my in 0..e.coded_height / 16 {
        let mut left = None;
        for mx in 0..e.coded_width / 16 {
            let a = IntraAvail {
                left: mx != 0,
                top: my != 0,
                top_left: mx != 0 && my != 0,
                top_right: my != 0 && mx + 1 < e.coded_width / 16,
            };
            let top = if my != 0 { row.get(mx) } else { None };
            let (base, source) =
                planes
                    .first()
                    .ok_or(Error::Internal)?
                    .tiles(input.y, mx * 16, my * 16, 16)?;
            let (cb, scb) =
                planes
                    .get(1)
                    .ok_or(Error::Internal)?
                    .tiles(input.cb, mx * 8, my * 8, 8)?;
            let (cr, scr) =
                planes
                    .get(2)
                    .ok_or(Error::Internal)?
                    .tiles(input.cr, mx * 8, my * 8, 8)?;
            let cbases = [cb, cr];
            let csources = [scb, scr];
            let mut chromas: [Option<Chroma>; 4] = [None, None, None, None];
            for mode in 0..4u8 {
                if !legal(mode, 8, a) {
                    continue;
                }
                match chroma(&cbases, &csources, a, mode, t::chroma_qp(e.settings.qp)) {
                    Ok(c) => *chromas.get_mut(usize::from(mode)).ok_or(Error::Internal)? = Some(c),
                    Err(Error::Coefficient) => {}
                    Err(err) => return Err(err),
                }
            }
            let mut best = None;
            let mut refusal = Error::Coefficient;
            for choice in 0..5u8 {
                let luma = if choice == 4 {
                    intra4(&base, &source, a, e.settings.qp, top, left.as_ref())
                } else {
                    if !legal(choice, 16, a) {
                        continue;
                    }
                    intra16(&base, &source, a, choice, e.settings.qp)
                };
                let luma = match luma {
                    Ok(y) => y,
                    Err(Error::Coefficient) => continue,
                    Err(err) => return Err(err),
                };
                for chroma in chromas.iter().flatten() {
                    let mut counter = Bits::count();
                    match macroblock(&mut counter, &luma, chroma, top, left.as_ref()) {
                        Ok(_) => {}
                        Err(Error::Coefficient) => continue,
                        Err(Error::MacroblockLimit) => {
                            refusal = Error::MacroblockLimit;
                            continue;
                        }
                        Err(err) => return Err(err),
                    }
                    let mut distortion = luma.tile.error(&source, 0, 0, 16)?;
                    for (tile, source) in chroma.tiles.iter().zip(&csources) {
                        distortion += tile.error(source, 0, 0, 8)?;
                    }
                    let key = (
                        distortion + (1u64 << (e.settings.qp / 3)) * counter.len as u64,
                        counter.len,
                        choice == 4,
                        choice,
                        chroma.mode,
                    );
                    if best
                        .as_ref()
                        .is_none_or(|b: &((u64, usize, bool, u8, u8), Luma, Chroma)| key < b.0)
                    {
                        best = Some((key, luma.clone(), chroma.clone()));
                    }
                }
            }
            let (_, luma, chroma) = best.ok_or(refusal)?;
            let state = macroblock(&mut writer, &luma, &chroma, top, left.as_ref())?;
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
            *row.get_mut(mx).ok_or(Error::Internal)? = state.clone();
            left = Some(state);
        }
    }
    if e.settings.deblocking == Deblocking::Enabled {
        for (c, plane) in planes.iter_mut().enumerate() {
            let qp = if c == 0 {
                e.settings.qp
            } else {
                t::chroma_qp(e.settings.qp)
            };
            deblock::intra(&mut plane.data, plane.width, plane.height, qp, c != 0)?;
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
    Ok((writer.nal(0x65)?, reconstruction))
}
