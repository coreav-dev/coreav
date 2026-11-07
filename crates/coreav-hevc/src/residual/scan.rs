use super::Scan;
use crate::{Error, Result};

struct Table {
    coordinates: [(u8, u8); 85],
    positions: [u8; 85],
}

static DIAGONAL: Table = table(Scan::Diagonal);
static HORIZONTAL: Table = table(Scan::Horizontal);
static VERTICAL: Table = table(Scan::Vertical);

#[allow(clippy::indexing_slicing)]
const fn table(scan: Scan) -> Table {
    let mut out = Table {
        coordinates: [(0, 0); 85],
        positions: [0; 85],
    };
    let mut side = 1;
    let mut start = 0;
    while side <= 8 {
        let mut position = 0;
        let mut outer = 0;
        while outer < 2 * side - 1 {
            let mut x = (outer + 1usize).saturating_sub(side);
            while x <= outer && x < side {
                let diagonal = (x, outer - x);
                let (cx, cy) = match scan {
                    Scan::Diagonal => diagonal,
                    Scan::Horizontal => (position % side, position / side),
                    Scan::Vertical => (position / side, position % side),
                };
                out.coordinates[start + position] = (cx as u8, cy as u8);
                out.positions[start + cy * side + cx] = position as u8;
                position += 1;
                x += 1;
            }
            outer += 1;
        }
        start += side * side;
        side *= 2;
    }
    out
}

pub(super) struct Order {
    coordinates: &'static [(u8, u8)],
    positions: &'static [u8],
    side: usize,
}

impl Scan {
    pub(super) fn order(self, side: usize) -> Result<Order> {
        let start = match side {
            1 => 0,
            2 => 1,
            4 => 5,
            8 => 21,
            _ => return Err(Error::InvalidData("residual_scan")),
        };
        let table = match self {
            Self::Diagonal => &DIAGONAL,
            Self::Horizontal => &HORIZONTAL,
            Self::Vertical => &VERTICAL,
        };
        let end = start + side * side;
        Ok(Order {
            coordinates: table
                .coordinates
                .get(start..end)
                .ok_or(Error::InvalidData("residual_scan"))?,
            positions: table
                .positions
                .get(start..end)
                .ok_or(Error::InvalidData("residual_scan"))?,
            side,
        })
    }
}

impl Order {
    pub(super) fn coordinate(&self, position: usize) -> Result<(usize, usize)> {
        self.coordinates
            .get(position)
            .map(|&(x, y)| (usize::from(x), usize::from(y)))
            .ok_or(Error::InvalidData("residual_scan"))
    }

    pub(super) fn index(&self, (x, y): (usize, usize)) -> Result<usize> {
        if x >= self.side || y >= self.side {
            return Err(Error::InvalidData("residual_last"));
        }
        self.positions
            .get(y * self.side + x)
            .map(|&position| usize::from(position))
            .ok_or(Error::InvalidData("residual_last"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_reject_coordinates_and_positions_outside_the_selected_grid() -> Result<()> {
        for scan in [Scan::Diagonal, Scan::Horizontal, Scan::Vertical] {
            for side in [1, 2, 4, 8] {
                let order = scan.order(side)?;
                for position in [side * side, 85, usize::MAX] {
                    assert!(order.coordinate(position).is_err());
                }
                for coordinate in [(side, 0), (0, side), (usize::MAX, usize::MAX)] {
                    assert!(order.index(coordinate).is_err());
                }
            }
            for side in [0, 3, 5, 16, usize::MAX] {
                assert!(scan.order(side).is_err());
            }
        }
        Ok(())
    }
}
