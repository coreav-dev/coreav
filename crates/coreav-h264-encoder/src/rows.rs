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
    fn at(table: &'a [OnceLock<Candidate>], address: usize, width: usize) -> Result<Self> {
        let (x, y) = (address % width, address / width);
        let read = |at: usize| table.get(at).and_then(OnceLock::get).ok_or(Error::Internal);
        Ok(Self {
            left: if x != 0 {
                Some(read(address - 1)?)
            } else {
                None
            },
            top: if y != 0 {
                Some(read(address - width)?)
            } else {
                None
            },
            top_left: if x != 0 && y != 0 {
                Some(read(address - width - 1)?)
            } else {
                None
            },
            top_right: if y != 0 && x + 1 < width {
                Some(read(address - width + 1)?)
            } else {
                None
            },
        })
    }

    #[inline(always)]
    fn tiles(
        &self,
        source: Plane<'_>,
        mx: usize,
        my: usize,
        component: usize,
        base: &mut Tile,
        input: &mut Tile,
    ) -> Result<()> {
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
        for y in 0..n {
            for x in 0..n {
                *input.0.get_mut(Tile::at(x, y)).ok_or(Error::Internal)? =
                    source.sample(mx * n + x, my * n + y)?;
            }
        }
        Ok(())
    }

    fn context(&self, p_slice: bool) -> MbContext<'a> {
        MbContext {
            left: self.left.map(|c| &c.state),
            top: self.top.map(|c| &c.state),
            p_slice,
        }
    }
}

pub(super) fn context(
    table: &[OnceLock<Candidate>],
    address: usize,
    width: usize,
    p_slice: bool,
) -> Result<MbContext<'_>> {
    Ok(Neighbours::at(table, address, width)?.context(p_slice))
}

pub(super) fn decide(
    e: &Encoder,
    input: Picture<'_>,
    slice: syntax::Slice,
    consume: impl FnMut(usize, &[OnceLock<Candidate>]) -> Result<()>,
    prepare: impl Fn(&[OnceLock<Candidate>]) -> Result<()> + Sync,
) -> Result<Vec<OnceLock<Candidate>>> {
    let width = e.coded_width / 16;
    crate::wavefront::fill(
        width,
        e.coded_height / 16,
        e.threads,
        |address, table| {
            let (mx, my) = (address % width, address / width);
            let neighbours = Neighbours::at(table, address, width)?;
            let border = Border {
                left: neighbours.left.map(|c| &c.state.motion),
                top: neighbours.top.map(|c| &c.state.motion),
                top_left: neighbours.top_left.map(|c| &c.state.motion),
                top_right: neighbours.top_right.map(|c| &c.state.motion),
            };
            let mut base = Tile::empty();
            let mut source = Tile::empty();
            let mut cbases = [Tile::empty(), Tile::empty()];
            let mut csources = [Tile::empty(), Tile::empty()];
            neighbours.tiles(input.y, mx, my, 0, &mut base, &mut source)?;
            for (c, ((base, source), plane)) in cbases
                .iter_mut()
                .zip(&mut csources)
                .zip([input.cb, input.cr])
                .enumerate()
            {
                neighbours.tiles(plane, mx, my, c + 1, base, source)?;
            }
            let context = neighbours.context(!slice.idr);
            let decision = Decision {
                source: &source,
                csources: &csources,
                context,
                qp: slice.qp,
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
                    colocated: e.previous_motion.get(address),
                }
                .candidates(&mut best, &mut refusal)?
            };
            let available = intra_availability(border);
            decision.intra(&base, &cbases, available, scope, &mut best, &mut refusal)?;
            let candidate = decision.refine_levels(
                best.ok_or(refusal)?,
                &base,
                &cbases,
                available,
                e.reference.as_ref(),
                (mx, my),
            )?;
            table
                .get(address)
                .ok_or(Error::Internal)?
                .set(candidate)
                .map_err(|_| Error::Internal)
        },
        consume,
        prepare,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::unwrap_used, clippy::indexing_slicing)]
    fn unavailable_wavefront_neighbours_are_refused_before_border_reads() {
        let settings = crate::Settings {
            frame_rate: Default::default(),
            sample_aspect_ratio: Default::default(),
            width: 64,
            height: 48,
            rate_mode: crate::RateMode::FixedQp(28),
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
        let encoder = Encoder::with_threads(settings, crate::Limits::default(), 3).unwrap();
        let y = [128; 64 * 48];
        let chroma = [128; 32 * 24];
        let picture = Picture {
            y: Plane::new(&y, 64, 64, 48).unwrap(),
            cb: Plane::new(&chroma, 32, 32, 24).unwrap(),
            cr: Plane::new(&chroma, 32, 32, 24).unwrap(),
        };
        let mut table = decide(
            &encoder,
            picture,
            syntax::Slice {
                idr: true,
                id: 0,
                frame_num: 0,
                qp: 28,
            },
            |_, _| Ok(()),
            |_| Ok(()),
        )
        .unwrap();
        let address = 5;
        assert!(Neighbours::at(&table, address, 4).is_ok());
        for dependency in [4, 1, 0, 2] {
            let candidate = table[dependency].take().unwrap();
            assert!(matches!(
                Neighbours::at(&table, address, 4),
                Err(Error::Internal)
            ));
            assert!(table[dependency].set(candidate).is_ok());
            assert!(Neighbours::at(&table, address, 4).is_ok());
        }
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
