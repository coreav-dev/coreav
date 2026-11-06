use crate::{Error, Result, bits::Bits};

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Mv {
    pub(crate) x: i32,
    pub(crate) y: i32,
}

impl Mv {
    pub(crate) fn valid(self) -> bool {
        (-8192..=8191).contains(&self.x) && (-2048..=2047).contains(&self.y)
    }

    pub(crate) fn difference(self, predictor: Self) -> Result<Self> {
        let d = Self {
            x: self.x.checked_sub(predictor.x).ok_or(Error::Syntax)?,
            y: self.y.checked_sub(predictor.y).ok_or(Error::Syntax)?,
        };
        if !self.valid() || !(-32768..=32767).contains(&d.x) || !(-32768..=32767).contains(&d.y) {
            return Err(Error::Syntax);
        }
        Ok(d)
    }

    pub(crate) fn write(self, w: &mut Bits) -> Result<()> {
        if !(-32768..=32767).contains(&self.x) || !(-32768..=32767).contains(&self.y) {
            return Err(Error::Syntax);
        }
        w.se(self.x)?;
        w.se(self.y)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct MotionMb {
    pub(crate) intra: bool,
    pub(crate) vectors: [Mv; 16],
}
impl Default for MotionMb {
    fn default() -> Self {
        Self {
            intra: true,
            vectors: [Mv::default(); 16],
        }
    }
}

#[derive(Clone, Copy, Default)]
pub(crate) struct Border<'a> {
    pub(crate) left: Option<&'a MotionMb>,
    pub(crate) top: Option<&'a MotionMb>,
    pub(crate) top_right: Option<&'a MotionMb>,
    pub(crate) top_left: Option<&'a MotionMb>,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct Part {
    pub(crate) x: usize,
    pub(crate) y: usize,
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) mv: Mv,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Layout {
    Whole,
    #[cfg(test)]
    Horizontal,
    #[cfg(test)]
    Vertical,
    Sub([u8; 4]),
}
impl Layout {
    pub(crate) fn parts(self) -> Result<([Part; 16], usize)> {
        let mut parts = [Part::default(); 16];
        let mut len = 0;
        let (groups, w, h) = match self {
            Self::Whole => (1, 16, 16),
            #[cfg(test)]
            Self::Horizontal => (2, 16, 8),
            #[cfg(test)]
            Self::Vertical => (2, 8, 16),
            Self::Sub(_) => (4, 8, 8),
        };
        for g in 0..groups {
            let (sw, sh) = if let Self::Sub(shapes) = self {
                match shapes.get(g).ok_or(Error::Internal)? {
                    0 => (8, 8),
                    1 => (8, 4),
                    2 => (4, 8),
                    3 => (4, 4),
                    _ => return Err(Error::Internal),
                }
            } else {
                (w, h)
            };
            for s in 0..(w / sw) * (h / sh) {
                *parts.get_mut(len).ok_or(Error::Internal)? = Part {
                    x: g % (16 / w) * w + s % (w / sw) * sw,
                    y: g / (16 / w) * h + s / (w / sw) * sh,
                    width: sw,
                    height: sh,
                    mv: Mv::default(),
                };
                len += 1;
            }
        }
        Ok((parts, len))
    }
    pub(crate) fn code(self) -> u32 {
        match self {
            Self::Whole => 0,
            #[cfg(test)]
            Self::Horizontal => 1,
            #[cfg(test)]
            Self::Vertical => 2,
            Self::Sub(_) => 3,
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Sample {
    available: bool,
    reference: bool,
    mv: Mv,
}

impl Border<'_> {
    fn sample(self, grid: &[Option<Mv>; 16], x: i32, y: i32) -> Result<Sample> {
        if (0..16).contains(&x) && (0..16).contains(&y) {
            return Ok(grid
                .get((y / 4 * 4 + x / 4) as usize)
                .ok_or(Error::Internal)?
                .map_or(Sample::default(), |mv| Sample {
                    available: true,
                    reference: true,
                    mv,
                }));
        }
        let mb = match (x, y) {
            (-1, -1) => self.top_left,
            (-1, 0..=15) => self.left,
            (0..=15, -1) => self.top,
            (16..=31, -1) => self.top_right,
            _ => None,
        };
        let Some(mb) = mb else {
            return Ok(Sample::default());
        };
        let mv = *mb
            .vectors
            .get((y.rem_euclid(16) / 4 * 4 + x.rem_euclid(16) / 4) as usize)
            .ok_or(Error::Internal)?;
        Ok(Sample {
            available: true,
            reference: !mb.intra,
            mv,
        })
    }

    fn neighbours(self, grid: &[Option<Mv>; 16], part: Part) -> Result<[Sample; 3]> {
        let (x, y) = (part.x as i32, part.y as i32);
        let a = self.sample(grid, x - 1, y)?;
        let b = self.sample(grid, x, y - 1)?;
        let mut c = self.sample(grid, x + part.width as i32, y - 1)?;
        if !c.available {
            c = self.sample(grid, x - 1, y - 1)?;
        }
        Ok([a, b, c])
    }

    pub(crate) fn predictor(
        self,
        grid: &[Option<Mv>; 16],
        part: Part,
        layout: Layout,
    ) -> Result<Mv> {
        let [a, mut b, mut c] = self.neighbours(grid, part)?;
        let preferred: Option<Sample> = match layout {
            #[cfg(test)]
            Layout::Horizontal if part.y == 0 => Some(b),
            #[cfg(test)]
            Layout::Horizontal => Some(a),
            #[cfg(test)]
            Layout::Vertical if part.x == 0 => Some(a),
            #[cfg(test)]
            Layout::Vertical => Some(c),
            _ => None,
        };
        if let Some(p) = preferred.filter(|p| p.reference) {
            return Ok(p.mv);
        }
        if !b.available && !c.available && a.available {
            b = a;
            c = a;
        }
        let matching = [a, b, c];
        if matching.iter().filter(|p| p.reference).count() == 1 {
            return matching
                .iter()
                .find(|p| p.reference)
                .map(|p| p.mv)
                .ok_or(Error::Internal);
        }
        fn median(a: i32, b: i32, c: i32) -> i32 {
            a.max(b.min(c)).min(b.max(c))
        }
        Ok(Mv {
            x: median(a.mv.x, b.mv.x, c.mv.x),
            y: median(a.mv.y, b.mv.y, c.mv.y),
        })
    }

    pub(crate) fn skip(self) -> Result<Mv> {
        let part = Part {
            width: 16,
            height: 16,
            ..Part::default()
        };
        let [a, b, _] = self.neighbours(&[None; 16], part)?;
        if !a.available
            || !b.available
            || [a, b].iter().any(|p| p.reference && p.mv == Mv::default())
        {
            return Ok(Mv::default());
        }
        self.predictor(&[None; 16], part, Layout::Whole)
    }
}

pub(crate) fn commit(grid: &mut [Option<Mv>; 16], part: Part) -> Result<()> {
    for y in (part.y..part.y + part.height).step_by(4) {
        for x in (part.x..part.x + part.width).step_by(4) {
            *grid.get_mut(y / 4 * 4 + x / 4).ok_or(Error::Internal)? = Some(part.mv);
        }
    }
    Ok(())
}

pub(crate) fn admitted(previous: u8, current: usize) -> bool {
    usize::from(previous) + current <= 16
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn mb(x: i32, y: i32) -> MotionMb {
        MotionMb {
            intra: false,
            vectors: [Mv { x, y }; 16],
        }
    }

    #[test]
    fn derived_and_difference_limits_are_separate_quarter_sample_domains() {
        for (x, y) in [(-8192, -2048), (8191, 2047)] {
            let mv = Mv { x, y };
            assert!(mv.valid());
            assert!(
                mv.difference(Mv {
                    x: -x - 1,
                    y: -y - 1
                })
                .is_ok()
            );
        }
        for (x, y) in [(-8193, 0), (8192, 0), (0, -2049), (0, 2048)] {
            assert!(!Mv { x, y }.valid());
            assert!(Mv { x, y }.difference(Mv::default()).is_err());
        }
        for value in [-32768, 32767] {
            assert!(Mv { x: value, y: value }.write(&mut Bits::count()).is_ok());
        }
        for value in [-32769, 32768] {
            assert_eq!(
                Mv { x: value, y: 0 }.write(&mut Bits::count()),
                Err(Error::Syntax)
            );
        }
        assert!(Mv::default().difference(Mv { x: 32769, y: 0 }).is_err());
    }

    #[test]
    fn predictor_obeys_directional_choices_substitution_and_intra_availability() {
        let (a, b, c, d) = (mb(4, 40), mb(12, 8), mb(20, 24), mb(28, 32));
        let border = Border {
            left: Some(&a),
            top: Some(&b),
            top_right: Some(&c),
            top_left: Some(&d),
        };
        let part = Part {
            width: 16,
            height: 16,
            ..Part::default()
        };
        assert_eq!(
            border.predictor(&[None; 16], part, Layout::Whole).unwrap(),
            Mv { x: 12, y: 24 }
        );
        let horizontal = Part { height: 8, ..part };
        assert_eq!(
            border
                .predictor(&[None; 16], horizontal, Layout::Horizontal)
                .unwrap(),
            b.vectors[0]
        );
        let vertical = Part { width: 8, ..part };
        assert_eq!(
            border
                .predictor(&[None; 16], vertical, Layout::Vertical)
                .unwrap(),
            a.vectors[0]
        );
        let vertical_right = Part { x: 8, ..vertical };
        assert_eq!(
            border
                .predictor(&[None; 16], vertical_right, Layout::Vertical)
                .unwrap(),
            c.vectors[0]
        );
        let bottom = Part { y: 8, ..horizontal };
        let mut grid = [None; 16];
        commit(
            &mut grid,
            Part {
                mv: Mv { x: 44, y: 16 },
                ..horizontal
            },
        )
        .unwrap();
        assert_eq!(
            border.predictor(&grid, bottom, Layout::Horizontal).unwrap(),
            a.vectors[0]
        );
        let only_left = Border {
            left: Some(&a),
            ..Border::default()
        };
        assert_eq!(
            only_left
                .predictor(&[None; 16], part, Layout::Whole)
                .unwrap(),
            a.vectors[0]
        );
        let intra = MotionMb::default();
        let intra_top = Border {
            left: Some(&a),
            top: Some(&intra),
            ..Border::default()
        };
        assert_eq!(
            intra_top
                .predictor(&[None; 16], part, Layout::Whole)
                .unwrap(),
            a.vectors[0]
        );
        let no_c = Border {
            top_right: None,
            ..border
        };
        assert_eq!(
            no_c.predictor(&[None; 16], part, Layout::Whole).unwrap(),
            Mv { x: 12, y: 32 }
        );
        let intra_c = Border {
            top_right: Some(&intra),
            ..border
        };
        assert_eq!(
            intra_c.predictor(&[None; 16], part, Layout::Whole).unwrap(),
            Mv { x: 4, y: 8 }
        );
    }

    #[test]
    fn subpartition_predictors_only_see_preceding_partitions() {
        for shape in 0..4 {
            let layout = Layout::Sub([shape; 4]);
            let (mut parts, len) = layout.parts().unwrap();
            let mut grid = [None; 16];
            for (i, part) in parts.iter_mut().take(len).enumerate() {
                let predictor = Border::default().predictor(&grid, *part, layout).unwrap();
                assert!(predictor.valid());
                part.mv = Mv {
                    x: 4 * (i as i32 + 1),
                    y: 0,
                };
                commit(&mut grid, *part).unwrap();
            }
            assert!(grid.iter().all(Option::is_some));
            assert_eq!(len, [4, 8, 8, 16][usize::from(shape)]);
        }
        let mut grid = [None; 16];
        let part = Part {
            x: 4,
            y: 12,
            width: 4,
            height: 4,
            ..Part::default()
        };
        grid[8] = Some(Mv { x: 7, y: 9 });
        grid[9] = Some(Mv { x: 1, y: 2 });
        grid[12] = Some(Mv { x: 4, y: 5 });
        assert_eq!(
            Border::default()
                .predictor(&grid, part, Layout::Sub([3; 4]))
                .unwrap(),
            Mv { x: 4, y: 5 }
        );
    }

    #[test]
    fn skip_is_normatively_derived_including_unavailable_and_zero_neighbours() {
        let a = mb(12, 16);
        let b = mb(20, 8);
        let border = Border {
            left: Some(&a),
            top: Some(&b),
            ..Border::default()
        };
        assert_eq!(border.skip().unwrap(), Mv { x: 12, y: 8 });
        assert_eq!(
            Border {
                top: None,
                ..border
            }
            .skip()
            .unwrap(),
            Mv::default()
        );
        let zero = mb(0, 0);
        assert_eq!(
            Border {
                left: Some(&zero),
                ..border
            }
            .skip()
            .unwrap(),
            Mv::default()
        );
        let intra = MotionMb::default();
        assert_eq!(
            Border {
                left: Some(&intra),
                ..border
            }
            .skip()
            .unwrap(),
            b.vectors[0]
        );
        for mv in [
            Mv { x: -8192, y: -2048 },
            Mv { x: 8191, y: 2047 },
            Mv { x: 8192, y: 0 },
        ] {
            let n = MotionMb {
                intra: false,
                vectors: [mv; 16],
            };
            let skip = Border {
                left: Some(&n),
                top: Some(&n),
                top_right: Some(&n),
                ..Border::default()
            }
            .skip()
            .unwrap();
            assert_eq!(skip, mv);
            assert_eq!(skip.valid(), mv.valid());
        }
    }

    #[test]
    fn consecutive_partition_counts_accept_sixteen_and_reject_seventeen() {
        assert!(admitted(15, 1));
        assert!(admitted(0, 16));
        assert!(!admitted(16, 1));
        assert!(!admitted(1, 16));
        assert!(admitted(16, 0));
        for shape in 0..4 {
            let (_, count) = Layout::Sub([shape; 4]).parts().unwrap();
            assert!(admitted((16 - count) as u8, count));
            assert!(!admitted((17 - count) as u8, count));
        }
    }
}
