use super::{Cell, Frame, Motion, ReferenceState, RetainedPicture};
use crate::{Error, Result};
use std::sync::RwLockReadGuard;

mod store;
pub(crate) use store::{RowPublisher, RowStore};

#[derive(Clone, Copy)]
pub(crate) enum ReadSupport {
    Samples { component: usize, last_row: usize },
    Motion { row: usize },
}

pub(crate) struct ReferenceRead<'a> {
    picture: ReadPicture<'a>,
    support: ReadSupport,
}

enum ReadPicture<'a> {
    Complete(&'a RetainedPicture),
    Building(RwLockReadGuard<'a, Option<RetainedPicture>>),
}

pub(super) struct MotionProbe {
    pub(super) motion: Option<Motion>,
    pub(super) reference: Option<ReferenceState>,
}

impl<'a> ReferenceRead<'a> {
    pub(crate) fn new(picture: &'a RetainedPicture, support: ReadSupport) -> Self {
        Self {
            picture: ReadPicture::Complete(picture),
            support,
        }
    }

    fn guarded(
        picture: RwLockReadGuard<'a, Option<RetainedPicture>>,
        support: ReadSupport,
    ) -> Self {
        Self {
            picture: ReadPicture::Building(picture),
            support,
        }
    }

    fn picture(&self) -> Result<&RetainedPicture> {
        match &self.picture {
            ReadPicture::Complete(picture) => Ok(picture),
            ReadPicture::Building(picture) => picture
                .as_ref()
                .ok_or(Error::Internal("reference_row_storage")),
        }
    }

    pub(crate) fn plane(&self) -> Result<coreav_kernels_hevc::RefPlane<'_>> {
        let ReadSupport::Samples {
            component,
            last_row,
        } = self.support
        else {
            return Err(Error::Internal("reference_read_kind"));
        };
        let scale = match component {
            0 => 1,
            1 | 2 => 2,
            _ => return Err(Error::InvalidData("inter_reference_plane")),
        };
        let picture = self.picture()?;
        let width = picture.info.width as usize / scale;
        let height = picture.info.height as usize / scale;
        let rows = last_row
            .checked_add(1)
            .filter(|rows| *rows <= height)
            .ok_or(Error::InvalidData("inter_reference_rows"))?;
        let length = rows.checked_mul(width).ok_or(Error::Limit("row_address"))?;
        let data = picture
            .planes
            .get(component)
            .and_then(|plane| plane.get(..length))
            .ok_or(Error::InvalidData("inter_reference_plane"))?;
        Ok(coreav_kernels_hevc::RefPlane::prefix(
            data, width, width, height, rows,
        )?)
    }

    pub(super) fn motion(&self, x: usize, y: usize) -> Result<MotionProbe> {
        let ReadSupport::Motion { row } = self.support else {
            return Err(Error::Internal("reference_read_kind"));
        };
        let picture = self.picture()?;
        if x >= picture.info.width as usize || y >= picture.info.height as usize || y > row {
            return Err(Error::InvalidData("inter_collocated_position"));
        }
        let Cell { motion, .. } = *picture
            .cells
            .get((y / 4) * (picture.info.width as usize / 4) + x / 4)
            .ok_or(Error::InvalidData("inter_collocated_cell"))?;
        Ok(MotionProbe {
            motion,
            reference: picture.reference,
        })
    }
}

#[derive(Clone, Copy)]
pub(super) struct SampleRows<'a> {
    pub(super) origin: usize,
    pub(super) above: Option<&'a [Vec<u16>; 3]>,
    pub(super) above_y: usize,
}

impl SampleRows<'_> {
    pub(super) fn cell_index(self, frame: &Frame, x: usize, y: usize) -> Result<usize> {
        if self.origin % 4 != 0 {
            return Err(Error::InvalidData("row_origin"));
        }
        let offset = (self.origin / 4)
            .checked_mul(frame.info.width as usize / 4)
            .ok_or(Error::Limit("row_address"))?;
        frame
            .cell_index(x, y)?
            .checked_sub(offset)
            .ok_or(Error::InvalidData("intra_cell_address"))
    }

    pub(super) fn cell(self, frame: &Frame, x: usize, y: usize) -> Result<&Cell> {
        frame
            .cells
            .get(self.cell_index(frame, x, y)?)
            .ok_or(Error::InvalidData("intra_cell_address"))
    }

    pub(super) const fn full() -> Self {
        Self {
            origin: 0,
            above: None,
            above_y: 0,
        }
    }

    pub(super) fn offset(self, component: usize, width: usize) -> Result<usize> {
        let scale = match component {
            0 => 1,
            1 | 2 => 2,
            _ => return Err(Error::InvalidData("row_component")),
        };
        if self.origin % 4 != 0 {
            return Err(Error::InvalidData("row_origin"));
        }
        (self.origin / scale)
            .checked_mul(width)
            .ok_or(Error::Limit("row_address"))
    }

    pub(super) fn sample(
        self,
        planes: &[Vec<u16>; 3],
        component: usize,
        width: usize,
        x: usize,
        y: usize,
    ) -> Result<u16> {
        let offset = self.offset(component, width)?;
        let scale = if component == 0 { 1 } else { 2 };
        if let Some(above) = self.above {
            if y.checked_add(1) == Some(self.above_y / scale) {
                return above
                    .get(component)
                    .and_then(|plane| plane.get(x))
                    .copied()
                    .ok_or(Error::InvalidData("row_intra_border"));
            }
        }
        y.checked_mul(width)
            .and_then(|n| n.checked_add(x))
            .and_then(|n| n.checked_sub(offset))
            .and_then(|n| planes.get(component).and_then(|plane| plane.get(n)))
            .copied()
            .ok_or(Error::InvalidData("intra_reference_sample"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_capabilities_separate_motion_from_filtered_component_prefixes() -> Result<()> {
        let info = crate::PictureInfo {
            width: 32,
            height: 32,
            visible_width: 32,
            visible_height: 32,
            bit_depth: 8,
            profile_idc: 1,
            ctb_log2: 4,
            tiles: false,
            wpp: false,
            conformance_window: [0; 4],
            vui: Default::default(),
        };
        let picture = RetainedPicture::for_test(0, info, None);
        let motion = ReferenceRead::new(&picture, ReadSupport::Motion { row: 15 });
        assert!(motion.motion(0, 15)?.motion.is_none());
        assert!(matches!(
            motion.motion(0, 16),
            Err(Error::InvalidData("inter_collocated_position"))
        ));
        assert!(matches!(
            motion.plane(),
            Err(Error::Internal("reference_read_kind"))
        ));
        let samples = ReferenceRead::new(
            &picture,
            ReadSupport::Samples {
                component: 0,
                last_row: 15,
            },
        );
        assert!(matches!(
            samples.motion(0, 0),
            Err(Error::Internal("reference_read_kind"))
        ));
        let plane = samples.plane()?;
        let mut output = [0; 16];
        let config = coreav_kernels_hevc::InterPrediction {
            depth: coreav_kernels_hevc::BitDepth::new(8)?,
            component: coreav_kernels_hevc::Component::Luma,
            x: 0,
            y: 12,
            motion: coreav_kernels_hevc::MotionVector { x: 0, y: 0 },
        };
        coreav_kernels_hevc::interpolate_inter(
            config,
            plane,
            coreav_kernels_hevc::RectMut::new(&mut output, 4, 4, 4)?,
        )?;
        assert!(
            coreav_kernels_hevc::interpolate_inter(
                coreav_kernels_hevc::InterPrediction { y: 13, ..config },
                plane,
                coreav_kernels_hevc::RectMut::new(&mut output, 4, 4, 4)?
            )
            .is_err()
        );
        assert!(
            ReferenceRead::new(
                &picture,
                ReadSupport::Samples {
                    component: 0,
                    last_row: usize::MAX
                }
            )
            .plane()
            .is_err()
        );
        Ok(())
    }
}
