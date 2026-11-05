use crate::{
    Error, Result,
    params::{Pps, Sps},
};

#[derive(Clone, Copy, Debug)]
pub(crate) struct Layout {
    pub width: u32,
    pub height: u32,
    columns: [u32; 41],
    rows: [u32; 45],
    column_count: usize,
    row_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Location {
    pub raster: u32,
    pub tile_scan: u32,
    pub tile: u32,
    pub x: u32,
    pub y: u32,
    pub tile_origin: [u32; 2],
    pub tile_size: [u32; 2],
}

fn boundaries<const N: usize>(
    length: u32,
    count: u32,
    uniform: bool,
    sizes: &[u32],
) -> Result<[u32; N]> {
    if count == 0 || count > length || count as usize >= N {
        return Err(Error::InvalidData("tile_grid"));
    }
    let mut result = [0; N];
    let mut previous: u32 = 0;
    for i in 1..count as usize {
        let next = if uniform {
            (i as u64 * u64::from(length) / u64::from(count)) as u32
        } else {
            previous
                .checked_add(*sizes.get(i - 1).ok_or(Error::InvalidData("tile_grid"))?)
                .ok_or(Error::InvalidData("tile_grid"))?
        };
        if next <= previous || next >= length {
            return Err(Error::InvalidData("tile_grid"));
        }
        *result.get_mut(i).ok_or(Error::InvalidData("tile_grid"))? = next;
        previous = next;
    }
    *result
        .get_mut(count as usize)
        .ok_or(Error::InvalidData("tile_grid"))? = length;
    Ok(result)
}

fn interval(position: u32, boundaries: &[u32], count: usize) -> Result<usize> {
    boundaries
        .windows(2)
        .take(count)
        .position(|pair| {
            pair.first().is_some_and(|a| position >= *a)
                && pair.get(1).is_some_and(|b| position < *b)
        })
        .ok_or(Error::InvalidData("ctu_address"))
}

impl Layout {
    pub(crate) fn new(sps: &Sps, pps: &Pps) -> Result<Self> {
        let width = sps.width.div_ceil(1 << sps.ctb_log2);
        let height = sps.height.div_ceil(1 << sps.ctb_log2);
        Ok(Self {
            width,
            height,
            columns: boundaries(
                width,
                pps.tile_columns,
                pps.uniform_tiles,
                &pps.column_width,
            )?,
            rows: boundaries(height, pps.tile_rows, pps.uniform_tiles, &pps.row_height)?,
            column_count: pps.tile_columns as usize,
            row_count: pps.tile_rows as usize,
        })
    }

    pub(crate) fn at_raster(&self, address: u32) -> Result<Location> {
        if address >= self.width * self.height {
            return Err(Error::InvalidData("ctu_address"));
        }
        let x = address % self.width;
        let y = address / self.width;
        let column = interval(x, &self.columns, self.column_count)?;
        let row = interval(y, &self.rows, self.row_count)?;
        let x0 = *self
            .columns
            .get(column)
            .ok_or(Error::InvalidData("tile_grid"))?;
        let y0 = *self.rows.get(row).ok_or(Error::InvalidData("tile_grid"))?;
        let tile_width = *self
            .columns
            .get(column + 1)
            .ok_or(Error::InvalidData("tile_grid"))?
            - x0;
        let tile_height = *self
            .rows
            .get(row + 1)
            .ok_or(Error::InvalidData("tile_grid"))?
            - y0;
        Ok(Location {
            raster: address,
            tile_scan: y0 * self.width + x0 * tile_height + (y - y0) * tile_width + x - x0,
            tile: (row * self.column_count + column) as u32,
            x,
            y,
            tile_origin: [x0, y0],
            tile_size: [tile_width, tile_height],
        })
    }

    pub(crate) fn at_tile_scan(&self, address: u32) -> Result<Location> {
        if address >= self.width * self.height {
            return Err(Error::InvalidData("ctu_address"));
        }
        let row = interval(address / self.width, &self.rows, self.row_count)?;
        let y0 = *self.rows.get(row).ok_or(Error::InvalidData("tile_grid"))?;
        let tile_height = *self
            .rows
            .get(row + 1)
            .ok_or(Error::InvalidData("tile_grid"))?
            - y0;
        let in_row = address - y0 * self.width;
        let column = interval(in_row / tile_height, &self.columns, self.column_count)?;
        let x0 = *self
            .columns
            .get(column)
            .ok_or(Error::InvalidData("tile_grid"))?;
        let tile_width = *self
            .columns
            .get(column + 1)
            .ok_or(Error::InvalidData("tile_grid"))?
            - x0;
        let offset = in_row - x0 * tile_height;
        let raster = (y0 + offset / tile_width) * self.width + x0 + offset % tile_width;
        self.at_raster(raster)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uniform(width: u32, height: u32, columns: u32, rows: u32) -> Result<Layout> {
        Ok(Layout {
            width,
            height,
            columns: boundaries(width, columns, true, &[])?,
            rows: boundaries(height, rows, true, &[])?,
            column_count: columns as usize,
            row_count: rows as usize,
        })
    }

    #[test]
    fn uniform_maps_are_inverse_permutations_for_small_grids() -> Result<()> {
        for width in 1..=16 {
            for height in 1..=16 {
                for columns in 1..=width.min(4) {
                    for rows in 1..=height.min(4) {
                        let layout = uniform(width, height, columns, rows)?;
                        let mut seen = [false; 256];
                        for raster in 0..width * height {
                            let location = layout.at_raster(raster)?;
                            let present = seen
                                .get_mut(location.tile_scan as usize)
                                .ok_or(Error::InvalidData("test_map"))?;
                            assert!(!*present);
                            *present = true;
                            assert_eq!(layout.at_tile_scan(location.tile_scan)?, location);
                        }
                        assert!(
                            seen.get(..(width * height) as usize)
                                .is_some_and(|s| s.iter().all(|v| *v))
                        );
                    }
                }
            }
        }
        Ok(())
    }

    #[test]
    fn nonuniform_tiles_follow_tile_then_raster_order() -> Result<()> {
        let layout = Layout {
            width: 5,
            height: 3,
            columns: boundaries(5, 3, false, &[1, 2])?,
            rows: boundaries(3, 2, false, &[2])?,
            column_count: 3,
            row_count: 2,
        };
        let expected = [0, 5, 1, 2, 6, 7, 3, 4, 8, 9, 10, 11, 12, 13, 14];
        for (tile_scan, raster) in expected.into_iter().enumerate() {
            let location = layout.at_tile_scan(tile_scan as u32)?;
            assert_eq!(location.raster, raster);
            assert_eq!(layout.at_raster(raster)?.tile_scan, tile_scan as u32);
        }
        assert!(layout.at_raster(15).is_err());
        assert!(layout.at_tile_scan(15).is_err());
        Ok(())
    }

    #[test]
    fn zero_overfull_overflowing_and_incomplete_tile_axes_fail() {
        for (length, count, sizes) in [
            (5, 0, &[1][..]),
            (5, 6, &[1][..]),
            (5, 2, &[0][..]),
            (5, 2, &[5][..]),
            (5, 3, &[1][..]),
            (5, 3, &[1, u32::MAX][..]),
        ] {
            assert!(boundaries::<41>(length, count, false, sizes).is_err());
        }
        assert!(boundaries::<41>(100, 41, true, &[]).is_err());
    }
}
