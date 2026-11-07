#![forbid(unsafe_code)]

use super::*;
use std::sync::OnceLock;

struct Neighbours<'a> {
    left: Option<&'a Candidate>,
    top: Option<&'a Candidate>,
    top_left: Option<&'a Candidate>,
    top_right: Option<&'a Candidate>,
}

impl<'a> Neighbours<'a> {
    fn at(
        current: &'a [OnceLock<Candidate>],
        top: &'a [OnceLock<Candidate>],
        x: usize,
    ) -> Result<Self> {
        let read = |at: usize| {
            current
                .get(at)
                .and_then(OnceLock::get)
                .ok_or(Error::Internal)
        };
        let above = |at: usize| top.get(at).and_then(OnceLock::get).ok_or(Error::Internal);
        Ok(Self {
            left: if x != 0 { Some(read(x - 1)?) } else { None },
            top: if !top.is_empty() {
                Some(above(x)?)
            } else {
                None
            },
            top_left: if x != 0 && !top.is_empty() {
                Some(above(x - 1)?)
            } else {
                None
            },
            top_right: if !top.is_empty() && x + 1 < current.len() {
                Some(above(x + 1)?)
            } else {
                None
            },
        })
    }

    fn tiles(
        &self,
        source: Plane<'_>,
        mx: usize,
        my: usize,
        component: usize,
    ) -> Result<(Tile, Tile)> {
        let n = if component == 0 { 16 } else { 8 };
        let tile = |candidate: &'a Candidate| -> Result<&'a Tile> {
            if component == 0 {
                Ok(&candidate.luma.tile)
            } else {
                candidate
                    .chroma
                    .tiles
                    .get(component - 1)
                    .ok_or(Error::Internal)
            }
        };
        let mut base = Tile::empty();
        if let Some(top) = self.top {
            let top = tile(top)?;
            let at = Tile::at(0, n - 1);
            base.0
                .get_mut(ORIGIN - STRIDE..ORIGIN - STRIDE + n)
                .ok_or(Error::Internal)?
                .copy_from_slice(top.0.get(at..at + n).ok_or(Error::Internal)?);
        }
        if let Some(top_right) = self.top_right {
            let top_right = tile(top_right)?;
            let at = Tile::at(0, n - 1);
            base.0
                .get_mut(ORIGIN - STRIDE + n..ORIGIN - STRIDE + 2 * n)
                .ok_or(Error::Internal)?
                .copy_from_slice(top_right.0.get(at..at + n).ok_or(Error::Internal)?);
        }
        if let Some(left) = self.left {
            let left = tile(left)?;
            for y in 0..n {
                *base
                    .0
                    .get_mut(ORIGIN - 1 + y * STRIDE)
                    .ok_or(Error::Internal)? =
                    *left.0.get(Tile::at(n - 1, y)).ok_or(Error::Internal)?;
            }
        }
        if let Some(top_left) = self.top_left {
            *base.0.get_mut(ORIGIN - STRIDE - 1).ok_or(Error::Internal)? = *tile(top_left)?
                .0
                .get(Tile::at(n - 1, n - 1))
                .ok_or(Error::Internal)?;
        }
        let mut input = Tile::empty();
        for y in 0..n {
            for x in 0..n {
                *input.0.get_mut(Tile::at(x, y)).ok_or(Error::Internal)? =
                    source.sample(mx * n + x, my * n + y)?;
            }
        }
        Ok((base, input))
    }

    fn context(&self, p_slice: bool) -> MbContext<'a> {
        MbContext {
            left: self.left.map(|c| &c.state),
            top: self.top.map(|c| &c.state),
            p_slice,
        }
    }
}

pub(super) fn context<'a>(
    current: &'a [OnceLock<Candidate>],
    top: &'a [OnceLock<Candidate>],
    x: usize,
    p_slice: bool,
) -> Result<MbContext<'a>> {
    Ok(Neighbours::at(current, top, x)?.context(p_slice))
}

pub(super) fn stream(
    e: &Encoder,
    input: Picture<'_>,
    slice: syntax::Slice,
    emit: impl FnMut(usize, &[OnceLock<Candidate>], &[OnceLock<Candidate>]) -> Result<()>,
) -> Result<()> {
    let width = e.coded_width / 16;
    crate::wavefront::run(
        width,
        e.coded_height / 16,
        e.threads,
        |address, current, top| {
            let (mx, my) = (address % width, address / width);
            let neighbours = Neighbours::at(current, top, mx)?;
            let border = Border {
                left: neighbours.left.map(|c| &c.state.motion),
                top: neighbours.top.map(|c| &c.state.motion),
                top_left: neighbours.top_left.map(|c| &c.state.motion),
                top_right: neighbours.top_right.map(|c| &c.state.motion),
            };
            let (base, source) = neighbours.tiles(input.y, mx, my, 0)?;
            let (cb, scb) = neighbours.tiles(input.cb, mx, my, 1)?;
            let (cr, scr) = neighbours.tiles(input.cr, mx, my, 2)?;
            let context = neighbours.context(!slice.idr);
            let csources = [scb, scr];
            let decision = Decision {
                source: &source,
                csources: &csources,
                context,
                qp: e.settings.qp,
                pending_skip: 0,
            };
            let previous = if address == 0 {
                e.previous_mv_count
            } else {
                neighbours
                    .left
                    .and_then(|c| c.inter.as_ref())
                    .map_or(0, |i| i.len as u8)
            };
            let previous = motion::row_previous(previous, mx == 0, mx + 1 == width);
            let mut best = None;
            let mut refusal = Error::Coefficient;
            let scope = if slice.idr {
                IntraScope::Exhaustive
            } else {
                inter::Search {
                    reference: e.reference.as_ref().ok_or(Error::Internal)?,
                    decision: &decision,
                    border,
                    mx,
                    my,
                    previous,
                }
                .candidates(&mut best, &mut refusal)?
            };
            decision.intra(
                &base,
                &[cb, cr],
                intra_availability(border),
                scope,
                &mut best,
                &mut refusal,
            )?;
            best.ok_or(refusal)
        },
        emit,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::unwrap_used, clippy::indexing_slicing)]
    fn every_neighbour_borrows_the_exact_completed_macroblock() {
        for (width, height) in [(16, 16), (80, 64)] {
            let settings = crate::Settings {
                width,
                height,
                qp: 28,
                deblocking: crate::Deblocking::Disabled,
                gop: crate::Gop::AllIntra,
                colour: crate::Colour {
                    full_range: false,
                    primaries: 1,
                    transfer: 1,
                    matrix: 1,
                    chroma_location: 0,
                },
            };
            let raw = vec![128; width * height * 3 / 2];
            let (y, uv) = raw.split_at(width * height);
            let (cb, cr) = uv.split_at(width * height / 4);
            let input = Picture {
                y: Plane::new(y, width, width, height).unwrap(),
                cb: Plane::new(cb, width / 2, width / 2, height / 2).unwrap(),
                cr: Plane::new(cr, width / 2, width / 2, height / 2).unwrap(),
            };
            for threads in [1, 3, 8] {
                let encoder =
                    Encoder::with_threads(settings, crate::Limits::default(), threads).unwrap();
                let mut checked = 0;
                stream(
                    &encoder,
                    input,
                    syntax::Slice {
                        idr: true,
                        id: 0,
                        frame_num: 0,
                    },
                    |row, current, top| {
                        for x in 0..current.len() {
                            let neighbours = Neighbours::at(current, top, x).unwrap();
                            assert_eq!(
                                neighbours
                                    .left
                                    .map(|n| std::ptr::eq(n, current[x - 1].get().unwrap())),
                                (x > 0).then_some(true)
                            );
                            assert_eq!(
                                neighbours
                                    .top
                                    .map(|n| std::ptr::eq(n, top[x].get().unwrap())),
                                (row > 0).then_some(true)
                            );
                            assert_eq!(
                                neighbours
                                    .top_left
                                    .map(|n| std::ptr::eq(n, top[x - 1].get().unwrap())),
                                (row > 0 && x > 0).then_some(true)
                            );
                            assert_eq!(
                                neighbours
                                    .top_right
                                    .map(|n| std::ptr::eq(n, top[x + 1].get().unwrap())),
                                (row > 0 && x + 1 < current.len()).then_some(true)
                            );
                            checked += 1;
                        }
                        Ok(())
                    },
                )
                .unwrap();
                assert_eq!(checked, width * height / 256);
            }
        }
    }

    #[test]
    fn compact_records_and_rolling_rows_bound_retained_workspace() {
        let slot = std::mem::size_of::<OnceLock<Candidate>>();
        let record = std::mem::size_of::<deblock::Record>();
        for workers in [1usize, 4, 8, 16] {
            let candidates = (workers + 1) * 120 * slot;
            let records = 120 * 68 * record;
            assert!(candidates + records < 12 * 1024 * 1024);
            println!("{workers} workers: {candidates} candidate bytes + {records} record bytes");
        }
        println!("slot bytes: {slot}, record bytes: {record}");
    }

    #[test]
    fn retained_decisions_fit_the_bounded_picture_workspace() {
        let slot_bytes = std::mem::size_of::<OnceLock<Candidate>>();
        let max_macroblocks = 1920 * 1088 / 256;
        assert!(slot_bytes * max_macroblocks <= 64 * 1024 * 1024);
        println!(
            "decision slot: {slot_bytes} bytes, maximum table: {} bytes",
            slot_bytes * max_macroblocks
        );
    }
}
