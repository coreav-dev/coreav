use super::{Motion, MotionReference, ReferenceState, RetainedPicture, syntax};
use crate::{
    Error, Result,
    dpb::{ReferenceIdentity, References},
    slice::InterSlice,
};
use coreav_kernels_hevc::{self as kernels, MotionVector};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Partition {
    Full,
    Horizontal,
    Vertical,
    Quad,
    Up,
    Down,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Pu {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

impl Partition {
    pub(super) const fn index(self) -> usize {
        match self {
            Self::Full => 0,
            Self::Horizontal => 1,
            Self::Vertical => 2,
            Self::Quad => 3,
            Self::Up => 4,
            Self::Down => 5,
            Self::Left => 6,
            Self::Right => 7,
        }
    }

    pub(super) fn units(self, area: syntax::Area) -> Result<([Pu; 4], usize)> {
        if !(3..=6).contains(&area.log2) {
            return Err(Error::InvalidData("inter_cu_size"));
        }
        let n = 1usize << area.log2;
        if area.x % n != 0
            || area.y % n != 0
            || area.x.checked_add(n).is_none()
            || area.y.checked_add(n).is_none()
        {
            return Err(Error::InvalidData("inter_cu_position"));
        }
        let full = Pu {
            x: area.x,
            y: area.y,
            width: n,
            height: n,
        };
        let mut units = [full; 4];
        let count = match self {
            Self::Full => 1,
            Self::Horizontal | Self::Up | Self::Down => {
                let first = match self {
                    Self::Up => n / 4,
                    Self::Down => 3 * n / 4,
                    _ => n / 2,
                };
                if first < 4 || n - first < 4 {
                    return Err(Error::InvalidData("inter_partition_size"));
                }
                *units
                    .get_mut(0)
                    .ok_or(Error::InvalidData("inter_partition"))? = Pu {
                    height: first,
                    ..full
                };
                *units
                    .get_mut(1)
                    .ok_or(Error::InvalidData("inter_partition"))? = Pu {
                    y: area.y + first,
                    height: n - first,
                    ..full
                };
                2
            }
            Self::Vertical | Self::Left | Self::Right => {
                let first = match self {
                    Self::Left => n / 4,
                    Self::Right => 3 * n / 4,
                    _ => n / 2,
                };
                if first < 4 || n - first < 4 {
                    return Err(Error::InvalidData("inter_partition_size"));
                }
                *units
                    .get_mut(0)
                    .ok_or(Error::InvalidData("inter_partition"))? = Pu {
                    width: first,
                    ..full
                };
                *units
                    .get_mut(1)
                    .ok_or(Error::InvalidData("inter_partition"))? = Pu {
                    x: area.x + first,
                    width: n - first,
                    ..full
                };
                2
            }
            Self::Quad => {
                for (i, unit) in units.iter_mut().enumerate() {
                    *unit = Pu {
                        x: area.x + i % 2 * (n / 2),
                        y: area.y + i / 2 * (n / 2),
                        width: n / 2,
                        height: n / 2,
                    };
                }
                4
            }
        };
        Ok((units, count))
    }
}

#[derive(Clone, Copy)]
pub(super) struct Resolved {
    pub motion: Motion,
    pub temporal: bool,
}

pub(super) struct ResolvedList {
    pub value: MotionReference,
    pub temporal: bool,
}

#[derive(Clone, Copy)]
struct Lists {
    entries: [[Option<u8>; 15]; 2],
    active: [u8; 2],
}

fn identity(references: References, entry: u8) -> Result<ReferenceIdentity> {
    let count = usize::from(references.before)
        + usize::from(references.after)
        + usize::from(references.long_term);
    if usize::from(entry) >= count || count > 16 {
        return Err(Error::InvalidData("inter_reference_catalog"));
    }
    references
        .entries
        .get(usize::from(entry))
        .copied()
        .flatten()
        .ok_or(Error::InvalidData("inter_reference_catalog"))
}

impl Lists {
    fn new(references: References, inter: InterSlice, kind: u32) -> Result<Self> {
        let before = usize::from(references.before);
        let after = usize::from(references.after);
        let total = before + after + usize::from(references.long_term);
        if total == 0
            || total > 16
            || kind > 1
            || inter.active[0] == 0
            || inter.active[0] > 15
            || kind == 0 && (inter.active[1] == 0 || inter.active[1] > 15)
            || kind == 1 && (inter.active[1] != 0 || inter.modifications[1].is_some())
        {
            return Err(Error::InvalidData("inter_reference_lists"));
        }
        for i in 0..total {
            let reference = identity(references, i as u8)?;
            if reference.long_term != (i >= before + after) {
                return Err(Error::InvalidData("inter_reference_class"));
            }
        }
        let mut lists = Self {
            entries: [[None; 15]; 2],
            active: inter.active,
        };
        for (list, entries) in lists.entries.iter_mut().enumerate() {
            let modification = *inter
                .modifications
                .get(list)
                .ok_or(Error::InvalidData("inter_reference_list"))?;
            let count = usize::from(
                *inter
                    .active
                    .get(list)
                    .ok_or(Error::InvalidData("inter_reference_list"))?,
            );
            for (i, entry) in entries.iter_mut().take(count).enumerate() {
                let position = if let Some(modification) = modification {
                    let value = usize::from(
                        *modification
                            .get(i)
                            .ok_or(Error::InvalidData("inter_list_modification"))?,
                    );
                    if total == 1 || value >= total {
                        return Err(Error::InvalidData("inter_list_modification"));
                    }
                    value
                } else {
                    i % total
                };
                let catalog = if list == 0 || position >= before + after {
                    position
                } else if position < after {
                    before + position
                } else {
                    position - after
                };
                *entry = Some(catalog as u8);
            }
        }
        Ok(lists)
    }

    fn get(self, list: usize, index: u8, vector: MotionVector) -> Result<MotionReference> {
        let reference = self
            .entries
            .get(list)
            .and_then(|entries| entries.get(usize::from(index)))
            .copied()
            .flatten()
            .ok_or(Error::InvalidData("inter_reference_index"))?;
        Ok(MotionReference {
            reference,
            index,
            vector,
        })
    }

    fn validate(self, motion: Motion) -> Result<()> {
        for (list, value) in motion.lists().into_iter().enumerate() {
            if let Some(value) = value {
                if self.get(list, value.index, value.vector)? != value {
                    return Err(Error::InvalidData("inter_reference_identity"));
                }
            }
        }
        Ok(())
    }
}

const ZERO: MotionVector = MotionVector { x: 0, y: 0 };

fn state(walker: &syntax::Walker<'_, '_, '_>) -> Result<(ReferenceState, InterSlice, Lists)> {
    let reference = walker
        .frame
        .reference
        .ok_or(Error::InvalidData("inter_picture_state"))?;
    let inter = walker
        .input
        .syntax
        .inter
        .ok_or(Error::InvalidData("inter_slice_state"))?;
    let lists = Lists::new(reference.references, inter, walker.input.syntax.kind)?;
    Ok((reference, inter, lists))
}

fn validate_partition(
    pu: Pu,
    cu: syntax::Area,
    partition: Partition,
    part_index: usize,
) -> Result<()> {
    let (units, count) = partition.units(cu)?;
    if part_index >= count || units.get(part_index) != Some(&pu) {
        return Err(Error::InvalidData("inter_partition"));
    }
    Ok(())
}

fn morton(x: usize, y: usize, ctb_log2: u32, min_log2: u32) -> usize {
    let mut value = 0;
    for bit in min_log2..ctb_log2 {
        value |= ((x >> bit) & 1) << (2 * (bit - min_log2));
        value |= ((y >> bit) & 1) << (2 * (bit - min_log2) + 1);
    }
    value
}

fn spatial(
    walker: &syntax::Walker<'_, '_, '_>,
    pu: Pu,
    cu: syntax::Area,
    part_index: usize,
    position: (Option<usize>, Option<usize>),
    merge_region: bool,
) -> Result<Option<Motion>> {
    let (Some(x), Some(y)) = position else {
        return Ok(None);
    };
    let n = 1usize << cu.log2;
    let same_cu = x >= cu.x && y >= cu.y && x < cu.x + n && y < cu.y + n;
    if same_cu
        && pu.width * 2 == n
        && pu.height * 2 == n
        && part_index == 1
        && y >= cu.y + pu.height
        && x < cu.x + pu.width
    {
        return Ok(None);
    }
    if merge_region
        && pu.x >> walker.input.pps.parallel_merge_log2 == x >> walker.input.pps.parallel_merge_log2
        && pu.y >> walker.input.pps.parallel_merge_log2 == y >> walker.input.pps.parallel_merge_log2
    {
        return Ok(None);
    }
    let Some(cell) = walker.neighbour(Some(x), Some(y))? else {
        return Ok(None);
    };
    if !same_cu {
        let ctb_log2 = walker.input.sps.ctb_log2;
        let raster = (y as u32 >> ctb_log2) * walker.input.layout.width + (x as u32 >> ctb_log2);
        let neighbour = walker.input.layout.at_raster(raster)?;
        let current = walker.input.layout.at_raster(
            (pu.y as u32 >> ctb_log2) * walker.input.layout.width + (pu.x as u32 >> ctb_log2),
        )?;
        if neighbour.tile_scan > current.tile_scan
            || neighbour.tile_scan == current.tile_scan
                && morton(x, y, ctb_log2, walker.input.sps.min_tb_log2)
                    > morton(pu.x, pu.y, ctb_log2, walker.input.sps.min_tb_log2)
        {
            return Ok(None);
        }
    }
    Ok(cell.motion)
}

fn scale(vector: MotionVector, from: i64, to: i64) -> Result<MotionVector> {
    if from == to {
        return Ok(vector);
    }
    let td = from.clamp(-128, 127);
    let tb = to.clamp(-128, 127);
    if td == 0 {
        return Err(Error::InvalidData("inter_motion_distance"));
    }
    let tx = (16384 + (td.abs() >> 1)) / td;
    let factor = ((tb * tx + 32) >> 6).clamp(-4096, 4095);
    let component = |value: i16| {
        let product = factor * i64::from(value);
        (product.signum() * ((product.abs() + 127) >> 8)).clamp(-32768, 32767) as i16
    };
    Ok(MotionVector {
        x: component(vector.x),
        y: component(vector.y),
    })
}

fn collocated_vector(
    picture: &RetainedPicture,
    position: (usize, usize),
    list: usize,
    no_backward: bool,
    from_l0: bool,
    target: ReferenceIdentity,
    current_poc: i32,
) -> Result<Option<MotionVector>> {
    let (x, y) = position;
    if x >= picture.info.width as usize || y >= picture.info.height as usize {
        return Err(Error::InvalidData("inter_collocated_position"));
    }
    let cell = picture
        .cells
        .get((y / 4) * (picture.info.width as usize / 4) + x / 4)
        .ok_or(Error::InvalidData("inter_collocated_cell"))?;
    let Some(motion) = cell.motion else {
        return Ok(None);
    };
    let selected = match motion {
        Motion::L0(value) | Motion::L1(value) => value,
        Motion::Bi(values) => *values
            .get(if no_backward {
                list
            } else {
                usize::from(from_l0)
            })
            .ok_or(Error::InvalidData("inter_collocated_list"))?,
    };
    let catalog = picture
        .reference
        .ok_or(Error::InvalidData("inter_collocated_catalog"))?;
    let source = identity(catalog.references, selected.reference)?;
    if source.long_term != target.long_term {
        return Ok(None);
    }
    if target.long_term {
        return Ok(Some(selected.vector));
    }
    let from = i64::from(catalog.poc) - i64::from(source.poc);
    let to = i64::from(current_poc) - i64::from(target.poc);
    Ok(Some(scale(selected.vector, from, to)?))
}

fn temporal(
    walker: &syntax::Walker<'_, '_, '_>,
    pu: Pu,
    list: usize,
    index: u8,
) -> Result<Option<MotionVector>> {
    let (current, inter, lists) = state(walker)?;
    let Some(collocated) = inter.collocated else {
        return Ok(None);
    };
    let dpb = walker.dpb.ok_or(Error::InvalidData("inter_dpb"))?;
    let selected = lists.get(
        if collocated.from_l0 { 0 } else { 1 },
        collocated.index,
        ZERO,
    )?;
    let picture = dpb.reference(identity(current.references, selected.reference)?)?;
    let target = identity(current.references, lists.get(list, index, ZERO)?.reference)?;
    let mut no_backward = true;
    for entries in lists.entries {
        for entry in entries.into_iter().flatten() {
            if identity(current.references, entry)?.poc > current.poc {
                no_backward = false;
            }
        }
    }
    let x = pu.x + pu.width;
    let y = pu.y + pu.height;
    if pu.y >> walker.input.sps.ctb_log2 == y >> walker.input.sps.ctb_log2
        && x < walker.frame.info.width as usize
        && y < walker.frame.info.height as usize
    {
        if let Some(vector) = collocated_vector(
            picture,
            (x & !15, y & !15),
            list,
            no_backward,
            collocated.from_l0,
            target,
            current.poc,
        )? {
            return Ok(Some(vector));
        }
    }
    collocated_vector(
        picture,
        ((pu.x + pu.width / 2) & !15, (pu.y + pu.height / 2) & !15),
        list,
        no_backward,
        collocated.from_l0,
        target,
        current.poc,
    )
}

fn motion(first: Option<MotionReference>, second: Option<MotionReference>) -> Result<Motion> {
    match (first, second) {
        (Some(a), Some(b)) => Ok(Motion::Bi([a, b])),
        (Some(a), None) => Ok(Motion::L0(a)),
        (None, Some(b)) => Ok(Motion::L1(b)),
        _ => Err(Error::InvalidData("inter_empty_motion")),
    }
}

struct Candidates {
    values: [Option<Resolved>; 5],
    count: usize,
    limit: usize,
}

impl Candidates {
    fn new(limit: u8) -> Result<Self> {
        if !(1..=5).contains(&limit) {
            return Err(Error::InvalidData("inter_merge_count"));
        }
        Ok(Self {
            values: [None; 5],
            count: 0,
            limit: usize::from(limit),
        })
    }

    fn push(&mut self, motion: Motion, temporal: bool) -> Result<()> {
        if self.count < self.limit {
            *self
                .values
                .get_mut(self.count)
                .ok_or(Error::InvalidData("inter_merge_count"))? =
                Some(Resolved { motion, temporal });
            self.count += 1;
        }
        Ok(())
    }

    fn get(&self, index: usize) -> Result<Resolved> {
        if index >= self.count {
            return Err(Error::InvalidData("inter_merge_index"));
        }
        self.values
            .get(index)
            .copied()
            .flatten()
            .ok_or(Error::InvalidData("inter_merge_index"))
    }

    fn spatial(&mut self, neighbours: [Option<Motion>; 5], lists: Lists) -> Result<()> {
        let [a1, b1, b0, a0, b2] = neighbours;
        let kept = [
            a1,
            b1.filter(|v| Some(*v) != a1),
            b0.filter(|v| Some(*v) != b1),
            a0.filter(|v| Some(*v) != a1),
        ];
        for candidate in kept.into_iter().flatten() {
            lists.validate(candidate)?;
            self.push(candidate, false)?;
        }
        if kept.into_iter().flatten().count() < 4 {
            if let Some(candidate) = b2.filter(|v| Some(*v) != a1 && Some(*v) != b1) {
                lists.validate(candidate)?;
                self.push(candidate, false)?;
            }
        }
        Ok(())
    }

    fn complete(&mut self, references: References, lists: Lists, kind: u32) -> Result<()> {
        let original = self.count;
        if kind == 0 && original > 1 && original < self.limit {
            for (first, second) in [
                (0, 1),
                (1, 0),
                (0, 2),
                (2, 0),
                (1, 2),
                (2, 1),
                (0, 3),
                (3, 0),
                (1, 3),
                (3, 1),
                (2, 3),
                (3, 2),
            ]
            .into_iter()
            .take(original * (original - 1))
            {
                if self.count == self.limit {
                    break;
                }
                let a = self.get(first)?;
                let b = self.get(second)?;
                let Some(first) = a.motion.lists().first().copied().flatten() else {
                    continue;
                };
                let Some(second) = b.motion.lists().get(1).copied().flatten() else {
                    continue;
                };
                if identity(references, first.reference)?.poc
                    != identity(references, second.reference)?.poc
                    || first.vector != second.vector
                {
                    self.push(Motion::Bi([first, second]), a.temporal || b.temporal)?;
                }
            }
        }
        let reference_count = if kind == 1 {
            lists.active[0]
        } else {
            lists.active[0].min(lists.active[1])
        };
        let mut zero_index = 0;
        while self.count < self.limit {
            let index = if zero_index < reference_count {
                zero_index
            } else {
                0
            };
            let first = lists.get(0, index, ZERO)?;
            self.push(
                if kind == 0 {
                    Motion::Bi([first, lists.get(1, index, ZERO)?])
                } else {
                    Motion::L0(first)
                },
                false,
            )?;
            zero_index += 1;
        }
        Ok(())
    }
}

pub(super) fn merge(
    walker: &syntax::Walker<'_, '_, '_>,
    pu: Pu,
    cu: syntax::Area,
    partition: Partition,
    part_index: usize,
    merge_index: u8,
) -> Result<Resolved> {
    validate_partition(pu, cu, partition, part_index)?;
    let (current, inter, lists) = state(walker)?;
    let mut candidates = Candidates::new(inter.max_merge_candidates)?;
    if usize::from(merge_index) >= candidates.limit {
        return Err(Error::InvalidData("inter_merge_index"));
    }
    let (area, part_index) = if walker.input.pps.parallel_merge_log2 > 2 && cu.log2 == 3 {
        (
            Pu {
                x: cu.x,
                y: cu.y,
                width: 8,
                height: 8,
            },
            0,
        )
    } else {
        (pu, part_index)
    };
    let left = area.x.checked_sub(1);
    let above = area.y.checked_sub(1);
    let a1 = if part_index == 1
        && matches!(
            partition,
            Partition::Vertical | Partition::Left | Partition::Right
        ) {
        None
    } else {
        spatial(
            walker,
            area,
            cu,
            part_index,
            (left, Some(area.y + area.height - 1)),
            true,
        )?
    };
    let b1 = if part_index == 1
        && matches!(
            partition,
            Partition::Horizontal | Partition::Up | Partition::Down
        ) {
        None
    } else {
        spatial(
            walker,
            area,
            cu,
            part_index,
            (Some(area.x + area.width - 1), above),
            true,
        )?
    };
    let b0 = spatial(
        walker,
        area,
        cu,
        part_index,
        (Some(area.x + area.width), above),
        true,
    )?;
    let a0 = spatial(
        walker,
        area,
        cu,
        part_index,
        (left, Some(area.y + area.height)),
        true,
    )?;
    let b2 = spatial(walker, area, cu, part_index, (left, above), true)?;
    candidates.spatial([a1, b1, b0, a0, b2], lists)?;
    if candidates.count < candidates.limit {
        let first = temporal(walker, area, 0, 0)?
            .map(|v| lists.get(0, 0, v))
            .transpose()?;
        let second = if walker.input.syntax.kind == 0 {
            temporal(walker, area, 1, 0)?
                .map(|v| lists.get(1, 0, v))
                .transpose()?
        } else {
            None
        };
        if first.is_some() || second.is_some() {
            candidates.push(motion(first, second)?, true)?;
        }
    }
    candidates.complete(current.references, lists, walker.input.syntax.kind)?;
    let mut resolved = candidates.get(usize::from(merge_index))?;
    if pu.width + pu.height == 12 {
        if let Motion::Bi([first, _]) = resolved.motion {
            resolved.motion = Motion::L0(first);
        }
    }
    Ok(resolved)
}

fn spatial_predictor(
    pool: &[Option<Motion>],
    references: ReferenceState,
    target: ReferenceIdentity,
    list: usize,
    scaling: bool,
) -> Result<Option<MotionVector>> {
    for candidate in pool.iter().flatten() {
        let values = candidate.lists();
        for selected in [list, 1 - list] {
            let Some(value) = values.get(selected).copied().flatten() else {
                continue;
            };
            let source = identity(references.references, value.reference)?;
            if !scaling && source.poc == target.poc {
                return Ok(Some(value.vector));
            }
            if scaling && source.long_term == target.long_term {
                return Ok(Some(if target.long_term {
                    value.vector
                } else {
                    scale(
                        value.vector,
                        i64::from(references.poc) - i64::from(source.poc),
                        i64::from(references.poc) - i64::from(target.poc),
                    )?
                }));
            }
        }
    }
    Ok(None)
}

fn spatial_predictors(
    left: [Option<Motion>; 2],
    above: [Option<Motion>; 3],
    references: ReferenceState,
    target: ReferenceIdentity,
    list: usize,
) -> Result<[Option<MotionVector>; 2]> {
    let has_left = left.iter().any(Option::is_some);
    let mut a = spatial_predictor(&left, references, target, list, false)?;
    if a.is_none() {
        a = spatial_predictor(&left, references, target, list, true)?;
    }
    let mut b = spatial_predictor(&above, references, target, list, false)?;
    if !has_left {
        if b.is_some() {
            a = b;
        }
        b = spatial_predictor(&above, references, target, list, true)?;
    }
    Ok([a, b])
}

#[expect(
    clippy::too_many_arguments,
    reason = "The shared syntax contract supplies resolved PU geometry, list selection and decoded motion difference separately"
)]
pub(super) fn amvp(
    walker: &syntax::Walker<'_, '_, '_>,
    pu: Pu,
    cu: syntax::Area,
    partition: Partition,
    part_index: usize,
    list: usize,
    ref_idx: u8,
    mvp_index: bool,
    mvd: MotionVector,
) -> Result<ResolvedList> {
    validate_partition(pu, cu, partition, part_index)?;
    if list > 1 {
        return Err(Error::InvalidData("inter_reference_list"));
    }
    let (current, _, lists) = state(walker)?;
    let target = lists.get(list, ref_idx, ZERO)?;
    let left = [
        spatial(
            walker,
            pu,
            cu,
            part_index,
            (pu.x.checked_sub(1), Some(pu.y + pu.height)),
            false,
        )?,
        spatial(
            walker,
            pu,
            cu,
            part_index,
            (pu.x.checked_sub(1), Some(pu.y + pu.height - 1)),
            false,
        )?,
    ];
    let above = [
        spatial(
            walker,
            pu,
            cu,
            part_index,
            (Some(pu.x + pu.width), pu.y.checked_sub(1)),
            false,
        )?,
        spatial(
            walker,
            pu,
            cu,
            part_index,
            (Some(pu.x + pu.width - 1), pu.y.checked_sub(1)),
            false,
        )?,
        spatial(
            walker,
            pu,
            cu,
            part_index,
            (pu.x.checked_sub(1), pu.y.checked_sub(1)),
            false,
        )?,
    ];
    for candidate in left.into_iter().chain(above).flatten() {
        lists.validate(candidate)?;
    }
    let [a, b] = spatial_predictors(
        left,
        above,
        current,
        identity(current.references, target.reference)?,
        list,
    )?;
    let mut predictors = [(ZERO, false); 2];
    let mut count = 0;
    for value in [a, b.filter(|v| Some(*v) != a)].into_iter().flatten() {
        *predictors
            .get_mut(count)
            .ok_or(Error::InvalidData("inter_amvp_count"))? = (value, false);
        count += 1;
    }
    if count < 2 {
        if let Some(value) = temporal(walker, pu, list, ref_idx)? {
            *predictors
                .get_mut(count)
                .ok_or(Error::InvalidData("inter_amvp_count"))? = (value, true);
        }
    }
    let (predictor, temporal) = *predictors
        .get(usize::from(mvp_index))
        .ok_or(Error::InvalidData("inter_amvp_index"))?;
    Ok(ResolvedList {
        value: MotionReference {
            vector: add_motion(predictor, mvd),
            ..target
        },
        temporal,
    })
}

fn add_motion(predictor: MotionVector, difference: MotionVector) -> MotionVector {
    MotionVector {
        x: predictor.x.wrapping_add(difference.x),
        y: predictor.y.wrapping_add(difference.y),
    }
}

pub(super) fn predict(
    walker: &mut syntax::Walker<'_, '_, '_>,
    pu: Pu,
    motion: Motion,
    temporal: bool,
    merge: bool,
) -> Result<()> {
    let (current, inter, lists) = state(walker)?;
    lists.validate(motion)?;
    if pu.width < 4
        || pu.height < 4
        || pu.width > 64
        || pu.height > 64
        || pu.width % 4 != 0
        || pu.height % 4 != 0
        || pu.x % 4 != 0
        || pu.y % 4 != 0
        || pu
            .x
            .checked_add(pu.width)
            .is_none_or(|v| v > walker.frame.info.width as usize)
        || pu
            .y
            .checked_add(pu.height)
            .is_none_or(|v| v > walker.frame.info.height as usize)
        || matches!(motion, Motion::Bi(_)) && pu.width + pu.height == 12
    {
        return Err(Error::InvalidData("inter_prediction_area"));
    }
    let dpb = walker.dpb.ok_or(Error::InvalidData("inter_dpb"))?;
    let active = motion.lists();
    let pictures = active.map(|value| {
        value
            .map(|value| {
                identity(current.references, value.reference)
                    .and_then(|reference| dpb.reference(reference))
            })
            .transpose()
    });
    let [first, second] = pictures;
    let pictures = [first?, second?];
    let mut long_term = false;
    let mut fractional_luma = false;
    let mut fractional_chroma = false;
    let mut negative = false;
    let mut modified = false;
    for (list, value) in active.into_iter().enumerate() {
        if let Some(value) = value {
            long_term |= identity(current.references, value.reference)?.long_term;
            fractional_luma |= value.vector.x & 3 != 0 || value.vector.y & 3 != 0;
            fractional_chroma |= value.vector.x & 7 != 0 || value.vector.y & 7 != 0;
            negative |= value.vector.x < 0 || value.vector.y < 0;
            modified |= inter.modifications.get(list).is_some_and(Option::is_some);
        }
    }
    for picture in pictures.into_iter().flatten() {
        if picture.info.width != walker.frame.info.width
            || picture.info.height != walker.frame.info.height
            || picture.info.bit_depth != walker.frame.info.bit_depth
        {
            return Err(Error::InvalidData("inter_reference_format"));
        }
    }
    for y in (pu.y..pu.y + pu.height).step_by(4) {
        for x in (pu.x..pu.x + pu.width).step_by(4) {
            let cell = walker.frame.cell(x, y)?;
            if cell.slice != walker.slice
                || cell.motion.is_some()
                || cell.mode.is_some()
                || cell.reconstructed != 0
            {
                return Err(Error::InvalidData("inter_prediction_overlap"));
            }
        }
    }
    let mut weighted = false;
    for component in 0..3 {
        let divisor = if component == 0 { 1 } else { 2 };
        let width = pu.width / divisor;
        let height = pu.height / divisor;
        let plane_width = walker.frame.info.width as usize / divisor;
        let plane_height = walker.frame.info.height as usize / divisor;
        let x = pu.x / divisor;
        let y = pu.y / divisor;
        let mut weights = [None; 2];
        let mut nondefault = false;
        for (list, value) in active.into_iter().enumerate() {
            let Some(value) = value else {
                continue;
            };
            let picture = pictures
                .get(list)
                .copied()
                .flatten()
                .ok_or(Error::InvalidData("inter_reference_samples"))?;
            let plane = picture
                .planes
                .get(component)
                .ok_or(Error::InvalidData("inter_reference_plane"))?;
            kernels::interpolate_inter(
                kernels::InterPrediction {
                    depth: walker.frame.depth,
                    component: if component == 0 {
                        kernels::Component::Luma
                    } else {
                        kernels::Component::Chroma
                    },
                    x,
                    y,
                    motion: value.vector,
                },
                kernels::RefPlane::new(plane, plane_width, plane_width, plane_height)?,
                kernels::RectMut::new(
                    walker
                        .frame
                        .scratch
                        .predictors
                        .get_mut(list)
                        .ok_or(Error::InvalidData("inter_predictor"))?,
                    width,
                    width,
                    height,
                )?,
            )?;
            if let Some(parameters) = inter.weights {
                let denom = *parameters
                    .denominators
                    .get(usize::from(component != 0))
                    .ok_or(Error::InvalidData("inter_weight_denom"))?;
                let weight = *parameters
                    .lists
                    .get(list)
                    .and_then(|values| values.get(usize::from(value.index)))
                    .and_then(|values| values.get(component))
                    .ok_or(Error::InvalidData("inter_weight_index"))?;
                *weights
                    .get_mut(list)
                    .ok_or(Error::InvalidData("inter_weight_list"))? = Some(
                    kernels::PredictionWeight::new(denom, weight.weight, weight.offset)?,
                );
                nondefault |= weight.weight != 1 << denom || weight.offset != 0;
            }
        }
        let frame = &mut walker.frame;
        let predictors = &frame.scratch.predictors;
        let a = kernels::Rect::new(
            predictors
                .first()
                .ok_or(Error::InvalidData("inter_predictor"))?,
            width,
            width,
            height,
        )?;
        let b = kernels::Rect::new(
            predictors
                .get(1)
                .ok_or(Error::InvalidData("inter_predictor"))?,
            width,
            width,
            height,
        )?;
        let output = kernels::RectMut::new(&mut frame.scratch.inter_output, width, width, height)?;
        match motion {
            Motion::L0(_) => kernels::predict_uni(frame.depth, a, output)?,
            Motion::L1(_) => kernels::predict_uni(frame.depth, b, output)?,
            Motion::Bi(_) => kernels::predict_bi(frame.depth, a, b, output)?,
        }
        let offset = y * plane_width + x;
        let plane = frame
            .planes
            .get_mut(component)
            .ok_or(Error::InvalidData("inter_destination_plane"))?;
        let destination = plane
            .get_mut(offset..)
            .ok_or(Error::InvalidData("inter_destination"))?;
        let output = kernels::RectMut::new(destination, plane_width, width, height)?;
        let [w0, w1] = weights;
        match motion {
            Motion::L0(_) => {
                if let Some(weight) = w0 {
                    kernels::weight_uni(frame.depth, weight, a, output)?;
                } else {
                    kernels::predict_uni(frame.depth, a, output)?;
                }
            }
            Motion::L1(_) => {
                if let Some(weight) = w1 {
                    kernels::weight_uni(frame.depth, weight, b, output)?;
                } else {
                    kernels::predict_uni(frame.depth, b, output)?;
                }
            }
            Motion::Bi(_) => {
                if let (Some(first), Some(second)) = (w0, w1) {
                    kernels::weight_bi(frame.depth, first, second, a, b, output)?;
                } else {
                    kernels::predict_bi(frame.depth, a, b, output)?;
                }
            }
        }
        if nondefault {
            weighted = true;
            let mut changed = 0;
            for dy in 0..height {
                for dx in 0..width {
                    let expected = frame
                        .scratch
                        .inter_output
                        .get(dy * width + dx)
                        .ok_or(Error::InvalidData("inter_weight_comparison"))?;
                    let actual = plane
                        .get(offset + dy * plane_width + dx)
                        .ok_or(Error::InvalidData("inter_destination"))?;
                    changed += u64::from(actual != expected);
                }
            }
            *frame
                .inter_stats
                .weighted_changed_samples
                .get_mut(component)
                .ok_or(Error::InvalidData("inter_component"))? += changed;
        }
    }
    for y in (pu.y..pu.y + pu.height).step_by(4) {
        for x in (pu.x..pu.x + pu.width).step_by(4) {
            let index = walker.frame.cell_index(x, y)?;
            let cell = walker
                .frame
                .cells
                .get_mut(index)
                .ok_or(Error::InvalidData("inter_cell"))?;
            cell.motion = Some(motion);
            cell.edges |= u8::from(x == pu.x) | (u8::from(y == pu.y) << 1);
        }
    }
    let stats = &mut walker.frame.inter_stats;
    stats.prediction_units += 1;
    stats.p_units += u64::from(walker.input.syntax.kind == 1);
    stats.b_units += u64::from(walker.input.syntax.kind == 0);
    stats.bi_units += u64::from(matches!(motion, Motion::Bi(_)));
    stats.merge_units += u64::from(merge);
    stats.temporal_units += u64::from(temporal);
    stats.weighted_units += u64::from(weighted);
    stats.fractional_luma_units += u64::from(fractional_luma);
    stats.fractional_chroma_units += u64::from(fractional_chroma);
    stats.negative_motion_units += u64::from(negative);
    stats.long_term_units += u64::from(long_term);
    stats.list_modification_units += u64::from(modified);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        sps: crate::params::Sps,
        pps: crate::params::Pps,
        layout: crate::ctu::Layout,
        info: crate::PictureInfo,
    }

    impl Fixture {
        fn new(depth: u32, columns: u32) -> Result<Self> {
            let bytes = crate::params::tests::sequence(depth, false, false, false);
            let mut sps = crate::params::sps(
                &mut crate::bits::Bits::new(&bytes),
                &crate::Limits::default(),
            )?;
            sps.width = 64;
            sps.height = 64;
            sps.visible_width = 64;
            sps.visible_height = 64;
            sps.conformance_window = [0; 4];
            sps.ctb_log2 = 4;
            sps.ctu_count = 16;
            sps.max_dpb = [4; 7];
            sps.max_reorder = [0; 7];
            let pps = crate::params::Pps {
                id: 0,
                sps_id: 0,
                dependent_segments: false,
                output_flag: false,
                extra_header_bits: 0,
                sign_data_hiding: false,
                cabac_init: false,
                ref_l0: 1,
                ref_l1: 1,
                weighted_pred: false,
                weighted_bipred: false,
                tiles: columns > 1,
                wpp: false,
                lists_modification: false,
                deblock_override: false,
                deblock_disabled: true,
                deblock_offsets: [0; 2],
                loop_filter_tiles: true,
                loop_filter_slices: true,
                chroma_offsets: false,
                header_extension: false,
                init_qp: 26,
                cb_qp_offset: 0,
                cr_qp_offset: 0,
                qp_delta_depth: 0,
                qp_delta_enabled: false,
                transform_skip: false,
                constrained_intra: false,
                transquant_bypass: false,
                parallel_merge_log2: 2,
                scaling_lists: None,
                tile_columns: columns,
                tile_rows: 1,
                uniform_tiles: true,
                column_width: [0; 39],
                row_height: [0; 43],
            };
            let layout = crate::ctu::Layout::new(&sps, &pps)?;
            let info = crate::PictureInfo {
                width: 64,
                height: 64,
                visible_width: 64,
                visible_height: 64,
                bit_depth: depth,
                profile_idc: if depth == 8 { 1 } else { 2 },
                ctb_log2: 4,
                tiles: columns > 1,
                wpp: false,
                conformance_window: [0; 4],
                vui: sps.vui,
            };
            Ok(Self {
                sps,
                pps,
                layout,
                info,
            })
        }

        fn input(&self, kind: u32, inter: InterSlice) -> crate::parser::SliceInput<'_> {
            crate::parser::SliceInput {
                index: 1,
                header: test_header(2, kind, inter),
                picture: self.info,
                original: &[],
                rbsp: &[],
                entry_lengths: &[],
                syntax: crate::parser::SliceSyntax {
                    kind,
                    independent_address: 0,
                    qp_y: 26,
                    chroma_qp_offsets: [0; 2],
                    initialization: if kind == 0 {
                        crate::cabac::Initialization::B
                    } else {
                        crate::cabac::Initialization::P
                    },
                    sao: [false; 2],
                    deblock_disabled: true,
                    deblock_offsets: [0; 2],
                    loop_filter_slices: true,
                    inter: Some(inter),
                },
                sps: &self.sps,
                pps: &self.pps,
                layout: &self.layout,
                scratch_bytes: 0,
            }
        }

        fn dpb(&self) -> Result<crate::dpb::Dpb> {
            self.dpb_with(|_| {})
        }

        fn dpb_with(&self, prepare: impl FnOnce(&mut RetainedPicture)) -> Result<crate::dpb::Dpb> {
            let mut dpb = crate::dpb::Dpb::new_reconstruction();
            dpb.picture(
                crate::NalHeader {
                    unit_type: 19,
                    layer_id: 0,
                    temporal_id: 0,
                },
                &test_header(0, 2, controls([1, 0])),
                &self.sps,
                &crate::Limits::default(),
                |_| Ok(()),
            )?;
            let mut picture = RetainedPicture::for_test(0, self.info, None);
            let mut references = References {
                before: 1,
                ..References::default()
            };
            if let Some(slot) = references.entries.first_mut() {
                *slot = Some(ReferenceIdentity {
                    index: 999,
                    poc: -2,
                    long_term: false,
                });
            }
            picture.reference = Some(ReferenceState { poc: 0, references });
            for cell in &mut picture.cells {
                cell.mode = None;
                cell.motion = Some(Motion::L0(MotionReference {
                    reference: 0,
                    index: 0,
                    vector: vector(12, 20),
                }));
            }
            prepare(&mut picture);
            dpb.attach(picture)?;
            dpb.resume(|_| Ok(()))?;
            dpb.picture(
                crate::NalHeader {
                    unit_type: 1,
                    layer_id: 0,
                    temporal_id: 0,
                },
                &test_header(2, 1, controls([1, 0])),
                &self.sps,
                &crate::Limits::default(),
                |_| Ok(()),
            )?;
            Ok(dpb)
        }
    }

    fn test_header(poc: u32, kind: u32, inter: InterSlice) -> crate::SliceHeader {
        let mut rps = crate::rps::Rps::default();
        if poc != 0 {
            rps.count = 1;
            rps.negative_count = 1;
            if let Some(delta) = rps.delta.first_mut() {
                *delta = -(poc as i32);
            }
            if let Some(used) = rps.used.first_mut() {
                *used = true;
            }
        }
        crate::SliceHeader {
            first: true,
            dependent: false,
            pps_id: 0,
            address: 0,
            slice_type: Some(kind),
            poc_lsb: Some(poc),
            output: Some(true),
            slice_qp_y: Some(26),
            chroma_qp_offsets: Some([0; 2]),
            cabac_init_flag: Some(false),
            sao: Some([false; 2]),
            deblock_disabled: Some(true),
            deblock_offsets: Some([0; 2]),
            loop_filter_slices: Some(true),
            entry_points: 0,
            rbsp_header_bytes: 0,
            no_output_of_prior_pics: false,
            short_term_rps: rps,
            long_term: [crate::LongTermRef::default(); 16],
            long_term_count: 0,
            reference_syntax: None,
            inter: (kind != 2).then_some(inter),
        }
    }

    fn catalog() -> References {
        let mut references = References {
            before: 2,
            after: 1,
            long_term: 1,
            ..References::default()
        };
        for (i, (poc, long_term)) in [(8, false), (6, false), (12, false), (0, true)]
            .into_iter()
            .enumerate()
        {
            if let Some(slot) = references.entries.get_mut(i) {
                *slot = Some(ReferenceIdentity {
                    index: i as u64,
                    poc,
                    long_term,
                });
            }
        }
        references
    }

    fn controls(active: [u8; 2]) -> InterSlice {
        InterSlice {
            active,
            modifications: [None; 2],
            mvd_l1_zero: false,
            collocated: None,
            weights: None,
            max_merge_candidates: 5,
        }
    }

    fn vector(x: i16, y: i16) -> MotionVector {
        MotionVector { x, y }
    }

    #[test]
    fn partitions_follow_literal_normative_geometry_and_order() -> Result<()> {
        let area = syntax::Area {
            x: 32,
            y: 64,
            log2: 5,
        };
        let expected = [
            (Partition::Full, vec![(32, 64, 32, 32)]),
            (
                Partition::Horizontal,
                vec![(32, 64, 32, 16), (32, 80, 32, 16)],
            ),
            (
                Partition::Vertical,
                vec![(32, 64, 16, 32), (48, 64, 16, 32)],
            ),
            (
                Partition::Quad,
                vec![
                    (32, 64, 16, 16),
                    (48, 64, 16, 16),
                    (32, 80, 16, 16),
                    (48, 80, 16, 16),
                ],
            ),
            (Partition::Up, vec![(32, 64, 32, 8), (32, 72, 32, 24)]),
            (Partition::Down, vec![(32, 64, 32, 24), (32, 88, 32, 8)]),
            (Partition::Left, vec![(32, 64, 8, 32), (40, 64, 24, 32)]),
            (Partition::Right, vec![(32, 64, 24, 32), (56, 64, 8, 32)]),
        ];
        for (index, (partition, expected)) in expected.into_iter().enumerate() {
            assert_eq!(partition.index(), index);
            let (units, count) = partition.units(area)?;
            assert_eq!(count, expected.len());
            for (i, (x, y, width, height)) in expected.into_iter().enumerate() {
                let pu = Pu {
                    x,
                    y,
                    width,
                    height,
                };
                assert_eq!(units[i], pu);
                validate_partition(pu, area, partition, i)?;
            }
            assert!(validate_partition(units[0], area, partition, count).is_err());
        }
        for log2 in [0, 2, 7, 32, u32::MAX] {
            assert!(
                Partition::Full
                    .units(syntax::Area { log2, ..area })
                    .is_err()
            );
        }
        assert!(
            Partition::Full
                .units(syntax::Area {
                    x: usize::MAX - 31,
                    y: 0,
                    log2: 5
                })
                .is_err()
        );
        assert!(
            Partition::Full
                .units(syntax::Area { x: 4, ..area })
                .is_err()
        );
        for partition in [
            Partition::Up,
            Partition::Down,
            Partition::Left,
            Partition::Right,
        ] {
            assert!(
                partition
                    .units(syntax::Area {
                        x: 0,
                        y: 0,
                        log2: 3
                    })
                    .is_err()
            );
        }
        Ok(())
    }

    #[test]
    fn reference_lists_repeat_before_after_long_order_and_preserve_active_indices() -> Result<()> {
        let references = catalog();
        let lists = Lists::new(references, controls([6, 5]), 0)?;
        for (list, expected) in [(0, vec![0, 1, 2, 3, 0, 1]), (1, vec![2, 0, 1, 3, 2])] {
            for (i, reference) in expected.into_iter().enumerate() {
                let value = lists.get(list, i as u8, vector(-7, 9))?;
                assert_eq!(
                    value,
                    MotionReference {
                        reference,
                        index: i as u8,
                        vector: vector(-7, 9)
                    }
                );
            }
        }
        let mut inter = controls([3, 3]);
        let mut first = [0; 15];
        first[..3].copy_from_slice(&[3, 0, 0]);
        let mut second = [0; 15];
        second[..3].copy_from_slice(&[1, 0, 2]);
        inter.modifications = [Some(first), Some(second)];
        let modified = Lists::new(references, inter, 0)?;
        assert_eq!(modified.get(0, 0, ZERO)?.reference, 3);
        assert_eq!(modified.get(0, 1, ZERO)?.reference, 0);
        assert_eq!(modified.get(0, 2, ZERO)?.reference, 0);
        assert_ne!(modified.get(0, 1, ZERO)?, modified.get(0, 2, ZERO)?);
        assert_eq!(modified.get(1, 0, ZERO)?.reference, 0);
        assert_eq!(modified.get(1, 1, ZERO)?.reference, 2);
        assert_eq!(modified.get(1, 2, ZERO)?.reference, 1);
        Ok(())
    }

    #[test]
    fn invalid_lists_catalogs_and_motion_identity_never_invent_a_reference() -> Result<()> {
        let references = catalog();
        for (inter, kind) in [
            (controls([0, 0]), 1),
            (controls([16, 0]), 1),
            (controls([1, 1]), 1),
            (controls([1, 0]), 0),
            (controls([1, 16]), 0),
            (controls([1, 0]), 2),
        ] {
            assert!(Lists::new(references, inter, kind).is_err());
        }
        assert!(Lists::new(References::default(), controls([1, 0]), 1).is_err());
        let mut missing = references;
        missing.entries[1] = None;
        assert!(Lists::new(missing, controls([1, 0]), 1).is_err());
        let mut wrong = references;
        wrong.before = 1;
        assert!(Lists::new(wrong, controls([1, 0]), 1).is_err());
        assert!(identity(references, 4).is_err());
        let mut inter = controls([1, 0]);
        inter.modifications[0] = Some([4; 15]);
        assert!(Lists::new(references, inter, 1).is_err());
        let lists = Lists::new(references, controls([1, 0]), 1)?;
        assert!(lists.get(2, 0, ZERO).is_err());
        assert!(lists.get(1, 0, ZERO).is_err());
        assert!(lists.get(0, 1, ZERO).is_err());
        assert!(
            lists
                .validate(Motion::L0(MotionReference {
                    reference: 3,
                    index: 0,
                    vector: ZERO
                }))
                .is_err()
        );
        assert!(motion(None, None).is_err());
        Ok(())
    }

    #[test]
    fn spatial_merge_prunes_only_the_specified_pairs_and_caps_four_spatial_entries() -> Result<()> {
        let references = catalog();
        let lists = Lists::new(references, controls([4, 4]), 0)?;
        let a = Motion::L0(lists.get(0, 0, vector(4, 0))?);
        let b = Motion::L0(lists.get(0, 1, vector(8, 0))?);
        let c = Motion::L1(lists.get(1, 0, vector(12, 0))?);
        let d = Motion::L1(lists.get(1, 1, vector(16, 0))?);
        let e = Motion::Bi([
            lists.get(0, 0, vector(20, 0))?,
            lists.get(1, 0, vector(24, 0))?,
        ]);
        let mut candidates = Candidates::new(5)?;
        candidates.spatial([Some(a), Some(a), Some(a), Some(a), Some(e)], lists)?;
        assert_eq!(candidates.count, 2);
        assert_eq!(candidates.get(0)?.motion, a);
        assert_eq!(candidates.get(1)?.motion, e);
        let mut candidates = Candidates::new(5)?;
        candidates.spatial([Some(a), Some(b), Some(a), Some(b), Some(e)], lists)?;
        assert_eq!(candidates.count, 4);
        assert_eq!(candidates.get(2)?.motion, a);
        assert_eq!(candidates.get(3)?.motion, b);
        let mut candidates = Candidates::new(5)?;
        candidates.spatial([Some(a), Some(b), Some(c), Some(d), Some(e)], lists)?;
        assert_eq!(candidates.count, 4);
        let mut candidates = Candidates::new(1)?;
        candidates.spatial([Some(a), Some(b), Some(c), Some(d), Some(e)], lists)?;
        assert_eq!(candidates.count, 1);
        assert!(candidates.get(1).is_err());
        assert!(Candidates::new(0).is_err());
        assert!(Candidates::new(6).is_err());
        Ok(())
    }

    #[test]
    fn combined_merge_follows_table_order_and_propagates_selected_temporal_provenance() -> Result<()>
    {
        let references = catalog();
        let lists = Lists::new(references, controls([4, 4]), 0)?;
        let a = lists.get(0, 0, vector(4, 8))?;
        let b = lists.get(0, 1, vector(12, 16))?;
        let c = lists.get(1, 0, vector(20, 24))?;
        let mut candidates = Candidates::new(5)?;
        candidates.push(Motion::L0(a), false)?;
        candidates.push(Motion::L0(b), true)?;
        candidates.push(Motion::L1(c), false)?;
        candidates.complete(references, lists, 0)?;
        assert_eq!(candidates.get(3)?.motion, Motion::Bi([a, c]));
        assert!(!candidates.get(3)?.temporal);
        assert_eq!(candidates.get(4)?.motion, Motion::Bi([b, c]));
        assert!(candidates.get(4)?.temporal);
        let mut candidates = Candidates::new(5)?;
        let same = lists.get(1, 1, a.vector)?;
        candidates.push(Motion::L0(a), false)?;
        candidates.push(Motion::L1(same), false)?;
        candidates.complete(references, lists, 0)?;
        assert_eq!(
            candidates.get(2)?.motion,
            Motion::Bi([lists.get(0, 0, ZERO)?, lists.get(1, 0, ZERO)?])
        );
        Ok(())
    }

    #[test]
    fn zero_merge_uses_each_active_index_then_only_index_zero() -> Result<()> {
        let references = catalog();
        for (kind, active, expected) in [
            (1, [2, 0], [0, 1, 0, 0, 0]),
            (0, [4, 2], [0, 1, 0, 0, 0]),
            (0, [1, 4], [0, 0, 0, 0, 0]),
        ] {
            let lists = Lists::new(references, controls(active), kind)?;
            let mut candidates = Candidates::new(5)?;
            candidates.complete(references, lists, kind)?;
            for (i, index) in expected.into_iter().enumerate() {
                let candidate = candidates.get(i)?;
                assert!(!candidate.temporal);
                for (list, value) in candidate.motion.lists().into_iter().enumerate() {
                    if let Some(value) = value {
                        assert_eq!(value, lists.get(list, index, ZERO)?);
                    }
                }
            }
        }
        Ok(())
    }

    #[test]
    fn distance_scaling_matches_signed_rounding_clipping_and_unclipped_equality() -> Result<()> {
        assert_eq!(scale(vector(256, -256), 4, 2)?, vector(128, -128));
        assert_eq!(scale(vector(256, -256), -4, 2)?, vector(-128, 128));
        assert_eq!(scale(vector(1, -1), 2, 1)?, ZERO);
        assert_eq!(scale(vector(3, -3), 2, 1)?, vector(1, -1));
        assert_eq!(scale(vector(3, -3), 2, 3)?, vector(4, -4));
        assert_eq!(
            scale(vector(i16::MAX, i16::MIN), 1, 127)?,
            vector(i16::MAX, i16::MIN)
        );
        assert_eq!(
            scale(vector(i16::MAX, i16::MIN), -1, 127)?,
            vector(i16::MIN, i16::MAX)
        );
        assert_eq!(scale(vector(256, -256), 127, 128)?, vector(256, -256));
        assert_eq!(
            scale(
                vector(256, -256),
                i64::from(i32::MAX) - i64::from(i32::MIN),
                128
            )?,
            vector(256, -256)
        );
        assert_eq!(scale(vector(7, -9), 0, 0)?, vector(7, -9));
        assert!(scale(vector(7, -9), 0, 1).is_err());
        assert_eq!(
            add_motion(vector(i16::MAX, i16::MIN), vector(1, -1)),
            vector(i16::MIN, i16::MAX)
        );
        assert_eq!(
            add_motion(vector(-30000, 30000), vector(-30000, 30000)),
            vector(5536, -5536)
        );
        Ok(())
    }

    #[test]
    fn amvp_prefers_exact_references_before_scaled_neighbours_and_same_list_first() -> Result<()> {
        let current = ReferenceState {
            poc: 10,
            references: catalog(),
        };
        let lists = Lists::new(current.references, controls([4, 4]), 0)?;
        let target = identity(current.references, 0)?;
        let a0 = Motion::L0(lists.get(0, 1, vector(80, 40))?);
        let a1 = Motion::Bi([
            lists.get(0, 0, vector(12, 16))?,
            lists.get(1, 1, vector(20, 24))?,
        ]);
        let left = [Some(a0), Some(a1)];
        assert_eq!(
            spatial_predictors(left, [None; 3], current, target, 0)?,
            [Some(vector(12, 16)), None]
        );
        let opposite = Motion::L1(lists.get(1, 1, vector(28, 32))?);
        assert_eq!(
            spatial_predictors([Some(a0), Some(opposite)], [None; 3], current, target, 0)?,
            [Some(vector(28, 32)), None]
        );
        assert_eq!(
            spatial_predictors([Some(a0), None], [None; 3], current, target, 0)?,
            [Some(vector(40, 20)), None]
        );
        let b0 = Motion::L0(lists.get(0, 1, vector(80, 40))?);
        let b1 = Motion::L0(lists.get(0, 0, vector(12, 16))?);
        assert_eq!(
            spatial_predictors([None; 2], [Some(b0), Some(b1), None], current, target, 0)?,
            [Some(vector(12, 16)), Some(vector(40, 20))]
        );
        assert_eq!(
            spatial_predictors([Some(a0), None], [Some(b0), None, None], current, target, 0)?,
            [Some(vector(40, 20)), None]
        );
        let long = identity(current.references, 3)?;
        assert_eq!(
            spatial_predictors([Some(a0), None], [None; 3], current, long, 0)?,
            [None, None]
        );
        let long_motion = Motion::L0(lists.get(0, 3, vector(32767, -32768))?);
        assert_eq!(
            spatial_predictors([Some(long_motion), None], [None; 3], current, long, 0)?,
            [Some(vector(32767, -32768)), None]
        );
        Ok(())
    }

    #[test]
    fn collocated_selection_uses_historical_catalog_not_current_dpb_slots() -> Result<()> {
        let info = crate::PictureInfo {
            width: 32,
            height: 32,
            visible_width: 32,
            visible_height: 32,
            bit_depth: 8,
            profile_idc: 1,
            ctb_log2: 5,
            tiles: false,
            wpp: false,
            conformance_window: [0; 4],
            vui: crate::Vui::default(),
        };
        let mut picture = RetainedPicture::for_test(30, info, None);
        picture.reference = Some(ReferenceState {
            poc: 10,
            references: catalog(),
        });
        let lists = Lists::new(catalog(), controls([4, 4]), 0)?;
        let a = lists.get(0, 0, vector(80, 40))?;
        let b = lists.get(1, 2, vector(20, 12))?;
        picture.cells[36].mode = None;
        picture.cells[36].motion = Some(Motion::Bi([a, b]));
        let target = ReferenceIdentity {
            index: 99,
            poc: 16,
            long_term: false,
        };
        assert_eq!(
            collocated_vector(&picture, (16, 16), 0, true, true, target, 20)?,
            Some(vector(160, 80))
        );
        assert_eq!(
            collocated_vector(&picture, (16, 16), 1, true, false, target, 20)?,
            Some(vector(20, 12))
        );
        assert_eq!(
            collocated_vector(&picture, (16, 16), 0, false, true, target, 20)?,
            Some(vector(20, 12))
        );
        assert_eq!(
            collocated_vector(&picture, (16, 16), 1, false, false, target, 20)?,
            Some(vector(160, 80))
        );
        assert_eq!(
            collocated_vector(&picture, (0, 0), 0, true, true, target, 20)?,
            None
        );
        assert_eq!(
            collocated_vector(
                &picture,
                (16, 16),
                0,
                true,
                true,
                ReferenceIdentity {
                    long_term: true,
                    ..target
                },
                20
            )?,
            None
        );
        picture.cells[36].motion = Some(Motion::L0(lists.get(0, 3, vector(-17, 23))?));
        assert_eq!(
            collocated_vector(
                &picture,
                (16, 16),
                0,
                true,
                true,
                ReferenceIdentity {
                    long_term: true,
                    ..target
                },
                20
            )?,
            Some(vector(-17, 23))
        );
        picture.reference = None;
        assert!(collocated_vector(&picture, (16, 16), 0, true, true, target, 20).is_err());
        assert!(collocated_vector(&picture, (32, 16), 0, true, true, target, 20).is_err());
        Ok(())
    }

    #[test]
    fn minimum_transform_z_scan_addresses_are_morton_ordered() {
        let expected = [0, 1, 4, 5, 2, 3, 6, 7, 8, 9, 12, 13, 10, 11, 14, 15];
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(morton(x * 4, y * 4, 4, 2), expected[y * 4 + x]);
            }
        }
        assert_eq!(morton(0, 0, 6, 2), morton(64, 64, 6, 2));
        assert!(morton(12, 4, 4, 2) < morton(0, 8, 4, 2));
        assert_eq!(morton(3, 3, 4, 2), morton(0, 0, 4, 2));
    }

    #[test]
    fn prediction_uses_duplicate_active_index_weights_and_marks_motion_without_reconstruction()
    -> Result<()> {
        for bits in [8, 10] {
            let fixture = Fixture::new(bits, 1)?;
            let dpb = fixture.dpb()?;
            let mut inter = controls([2, 0]);
            inter.collocated = Some(crate::slice::Collocated {
                from_l0: true,
                index: 0,
            });
            let mut weights = crate::slice::Weights {
                denominators: [0; 2],
                lists: [[[crate::slice::Weight {
                    weight: 1,
                    offset: 0,
                }; 3]; 15]; 2],
            };
            weights.lists[0][1] = [crate::slice::Weight {
                weight: 2,
                offset: -1,
            }; 3];
            inter.weights = Some(weights);
            let input = fixture.input(1, inter);
            let mut frame = super::super::Frame::new_full(
                &input,
                2,
                dpb.current_references(),
                0,
                1_000_000,
                None,
            )?;
            for cell in &mut frame.cells {
                cell.slice = 1;
            }
            let mut walker = syntax::Walker {
                frame: &mut frame,
                input: &input,
                location: fixture.layout.at_raster(0)?,
                slice: 1,
                dpb: Some(&dpb),
            };
            let area = syntax::Area {
                x: 0,
                y: 0,
                log2: 3,
            };
            let pu = Partition::Full.units(area)?.0[0];
            let resolved = amvp(
                &walker,
                pu,
                area,
                Partition::Full,
                0,
                0,
                1,
                false,
                vector(-13, -20),
            )?;
            assert!(resolved.temporal);
            assert_eq!(resolved.value.vector, vector(-1, 0));
            assert_eq!((resolved.value.reference, resolved.value.index), (0, 1));
            predict(
                &mut walker,
                pu,
                Motion::L0(resolved.value),
                resolved.temporal,
                false,
            )?;
            for component in 0..3 {
                let divisor = if component == 0 { 1 } else { 2 };
                let width = 64 / divisor;
                let expected = 2 * (42 + component as u16) - (1 << (bits - 8));
                for y in 0..pu.height / divisor {
                    for x in 0..pu.width / divisor {
                        assert_eq!(walker.frame.planes[component][y * width + x], expected);
                    }
                }
            }
            for y in (0..8).step_by(4) {
                for x in (0..8).step_by(4) {
                    let cell = walker.frame.cell(x, y)?;
                    assert_eq!(cell.motion, Some(Motion::L0(resolved.value)));
                    assert_eq!(cell.reconstructed, 0);
                    assert_eq!(cell.edges, u8::from(x == 0) | (u8::from(y == 0) << 1));
                }
            }
            let stats = walker.frame.inter_stats;
            assert_eq!(
                (
                    stats.prediction_units,
                    stats.p_units,
                    stats.b_units,
                    stats.bi_units,
                    stats.temporal_units
                ),
                (1, 1, 0, 0, 1)
            );
            assert_eq!(
                (
                    stats.weighted_units,
                    stats.fractional_luma_units,
                    stats.fractional_chroma_units,
                    stats.negative_motion_units
                ),
                (1, 1, 1, 1)
            );
            assert_eq!(stats.weighted_changed_samples, [64, 16, 16]);
            assert_eq!(stats.merge_units, 0);
            assert_eq!(stats.amvp_lists, 0);
            assert_eq!(stats.partitions, [0; 8]);
            assert!(predict(&mut walker, pu, Motion::L0(resolved.value), false, false).is_err());
            assert_eq!(walker.frame.inter_stats, stats);
        }
        Ok(())
    }

    #[test]
    fn biprediction_and_l1_only_weighting_use_their_exact_list_parameters() -> Result<()> {
        let fixture = Fixture::new(8, 1)?;
        let dpb = fixture.dpb()?;
        for bi in [false, true] {
            let mut inter = controls([1, 1]);
            let mut weights = crate::slice::Weights {
                denominators: [0; 2],
                lists: [[[crate::slice::Weight {
                    weight: 1,
                    offset: 0,
                }; 3]; 15]; 2],
            };
            weights.lists[1][0] = [crate::slice::Weight {
                weight: 3,
                offset: 0,
            }; 3];
            inter.weights = Some(weights);
            let input = fixture.input(0, inter);
            let mut frame = super::super::Frame::new_full(
                &input,
                2,
                dpb.current_references(),
                0,
                1_000_000,
                None,
            )?;
            for cell in &mut frame.cells {
                cell.slice = 1;
            }
            let mut walker = syntax::Walker {
                frame: &mut frame,
                input: &input,
                location: fixture.layout.at_raster(0)?,
                slice: 1,
                dpb: Some(&dpb),
            };
            let lists = state(&walker)?.2;
            let value = if bi {
                Motion::Bi([lists.get(0, 0, ZERO)?, lists.get(1, 0, ZERO)?])
            } else {
                Motion::L1(lists.get(1, 0, ZERO)?)
            };
            let pu = Pu {
                x: 0,
                y: 0,
                width: 16,
                height: 8,
            };
            predict(&mut walker, pu, value, false, true)?;
            assert_eq!(walker.frame.planes[0][0], if bi { 84 } else { 126 });
            assert_eq!(walker.frame.planes[1][0], if bi { 86 } else { 129 });
            assert_eq!(walker.frame.inter_stats.b_units, 1);
            assert_eq!(walker.frame.inter_stats.bi_units, u64::from(bi));
            assert_eq!(walker.frame.inter_stats.merge_units, 1);
            assert_eq!(
                walker.frame.inter_stats.weighted_changed_samples,
                [128, 32, 32]
            );
        }
        Ok(())
    }

    #[test]
    fn spatial_availability_enforces_partition_parallel_region_slice_tile_and_z_scan_rules()
    -> Result<()> {
        let fixture = Fixture::new(8, 1)?;
        let dpb = fixture.dpb()?;
        let input = fixture.input(1, controls([1, 0]));
        let reference = MotionReference {
            reference: 0,
            index: 0,
            vector: vector(40, -8),
        };
        for partition in [Partition::Horizontal, Partition::Vertical] {
            let mut frame = super::super::Frame::new_full(
                &input,
                2,
                dpb.current_references(),
                0,
                1_000_000,
                None,
            )?;
            for cell in &mut frame.cells {
                cell.slice = 1;
            }
            let neighbour = if partition == Partition::Horizontal {
                (15, 7)
            } else {
                (7, 15)
            };
            let index = frame.cell_index(neighbour.0, neighbour.1)?;
            frame.cells[index].motion = Some(Motion::L0(reference));
            let walker = syntax::Walker {
                frame: &mut frame,
                input: &input,
                location: fixture.layout.at_raster(0)?,
                slice: 1,
                dpb: Some(&dpb),
            };
            let cu = syntax::Area {
                x: 0,
                y: 0,
                log2: 4,
            };
            let pu = partition.units(cu)?.0[1];
            assert_eq!(
                merge(&walker, pu, cu, partition, 1, 0)?.motion,
                Motion::L0(MotionReference {
                    vector: ZERO,
                    ..reference
                })
            );
            assert_eq!(
                amvp(&walker, pu, cu, partition, 1, 0, 0, false, ZERO)?.value,
                reference
            );
        }
        let mut frame =
            super::super::Frame::new_full(&input, 2, dpb.current_references(), 0, 1_000_000, None)?;
        for cell in &mut frame.cells {
            cell.slice = 1;
        }
        frame.cells[16].motion = Some(Motion::L0(reference));
        frame.cells[32].motion = Some(Motion::L0(reference));
        let walker = syntax::Walker {
            frame: &mut frame,
            input: &input,
            location: fixture.layout.at_raster(0)?,
            slice: 1,
            dpb: Some(&dpb),
        };
        let cu = syntax::Area {
            x: 0,
            y: 0,
            log2: 3,
        };
        let pu = Partition::Quad.units(cu)?.0[1];
        assert!(spatial(&walker, pu, cu, 1, (Some(3), Some(4)), false)?.is_none());
        let cu = syntax::Area {
            x: 8,
            y: 0,
            log2: 3,
        };
        let pu = Partition::Full.units(cu)?.0[0];
        assert!(spatial(&walker, pu, cu, 0, (Some(3), Some(4)), false)?.is_some());
        assert!(spatial(&walker, pu, cu, 0, (Some(3), Some(8)), false)?.is_none());
        let mut pps = fixture.pps;
        pps.parallel_merge_log2 = 4;
        let mut input = fixture.input(1, controls([1, 0]));
        input.pps = &pps;
        let mut frame =
            super::super::Frame::new_full(&input, 2, dpb.current_references(), 0, 1_000_000, None)?;
        for cell in &mut frame.cells {
            cell.slice = 1;
        }
        frame.cells[49].motion = Some(Motion::L0(reference));
        let walker = syntax::Walker {
            frame: &mut frame,
            input: &input,
            location: fixture.layout.at_raster(0)?,
            slice: 1,
            dpb: Some(&dpb),
        };
        let cu = syntax::Area {
            x: 8,
            y: 8,
            log2: 3,
        };
        let pu = Partition::Full.units(cu)?.0[0];
        assert_eq!(
            merge(&walker, pu, cu, Partition::Full, 0, 0)?.motion,
            Motion::L0(MotionReference {
                vector: ZERO,
                ..reference
            })
        );
        assert_eq!(
            amvp(&walker, pu, cu, Partition::Full, 0, 0, 0, false, ZERO)?.value,
            reference
        );
        let fixture = Fixture::new(8, 2)?;
        let input = fixture.input(1, controls([1, 0]));
        let mut frame =
            super::super::Frame::new_full(&input, 2, dpb.current_references(), 0, 1_000_000, None)?;
        for cell in &mut frame.cells {
            cell.slice = 1;
        }
        frame.cells[55].motion = Some(Motion::L0(reference));
        let walker = syntax::Walker {
            frame: &mut frame,
            input: &input,
            location: fixture.layout.at_raster(2)?,
            slice: 1,
            dpb: Some(&dpb),
        };
        let cu = syntax::Area {
            x: 32,
            y: 0,
            log2: 4,
        };
        let pu = Partition::Full.units(cu)?.0[0];
        assert!(spatial(&walker, pu, cu, 0, (Some(31), Some(15)), false)?.is_none());
        walker.frame.cells[56].motion = Some(Motion::L0(reference));
        walker.frame.cells[56].slice = 2;
        assert!(spatial(&walker, pu, cu, 0, (Some(32), Some(12)), false)?.is_none());
        Ok(())
    }

    #[test]
    fn temporal_positions_align_to_sixteen_fall_back_and_share_eight_by_eight_merge_lists()
    -> Result<()> {
        let fixture = Fixture::new(8, 1)?;
        let dpb = fixture.dpb_with(|picture| {
            if let Some(cell) = picture.cells.get_mut(4) {
                cell.motion = Some(Motion::L0(MotionReference {
                    reference: 0,
                    index: 0,
                    vector: vector(100, 200),
                }));
            }
        })?;
        let mut inter = controls([1, 0]);
        inter.collocated = Some(crate::slice::Collocated {
            from_l0: true,
            index: 0,
        });
        for level in [2, 3] {
            let mut pps = fixture.pps;
            pps.parallel_merge_log2 = level;
            let mut input = fixture.input(1, inter);
            input.pps = &pps;
            let mut frame = super::super::Frame::new_full(
                &input,
                2,
                dpb.current_references(),
                0,
                1_000_000,
                None,
            )?;
            for cell in &mut frame.cells {
                cell.slice = 1;
            }
            let walker = syntax::Walker {
                frame: &mut frame,
                input: &input,
                location: fixture.layout.at_raster(0)?,
                slice: 1,
                dpb: Some(&dpb),
            };
            let cu = syntax::Area {
                x: 8,
                y: 8,
                log2: 3,
            };
            let (units, _) = Partition::Horizontal.units(cu)?;
            let first = merge(&walker, units[0], cu, Partition::Horizontal, 0, 0)?;
            let second = merge(&walker, units[1], cu, Partition::Horizontal, 1, 0)?;
            assert!(first.temporal && second.temporal);
            let expected = if level == 2 {
                vector(100, 200)
            } else {
                vector(12, 20)
            };
            assert_eq!(
                first.motion,
                Motion::L0(MotionReference {
                    reference: 0,
                    index: 0,
                    vector: expected
                })
            );
            assert_eq!(
                second.motion,
                Motion::L0(MotionReference {
                    reference: 0,
                    index: 0,
                    vector: vector(12, 20)
                })
            );
        }
        let dpb = fixture.dpb_with(|picture| {
            if let Some(cell) = picture.cells.get_mut(4) {
                cell.mode = coreav_kernels_hevc::IntraMode::new(1).ok();
                cell.motion = None;
            }
        })?;
        let input = fixture.input(1, inter);
        let mut frame =
            super::super::Frame::new_full(&input, 2, dpb.current_references(), 0, 1_000_000, None)?;
        let walker = syntax::Walker {
            frame: &mut frame,
            input: &input,
            location: fixture.layout.at_raster(0)?,
            slice: 1,
            dpb: Some(&dpb),
        };
        assert_eq!(
            temporal(
                &walker,
                Pu {
                    x: 8,
                    y: 8,
                    width: 8,
                    height: 4
                },
                0,
                0
            )?,
            Some(vector(12, 20))
        );
        Ok(())
    }

    #[test]
    fn four_by_eight_merge_restricts_bipred_and_missing_dpb_samples_never_become_zero_pixels()
    -> Result<()> {
        let fixture = Fixture::new(8, 1)?;
        let dpb = fixture.dpb()?;
        let input = fixture.input(0, controls([1, 1]));
        let mut frame =
            super::super::Frame::new_full(&input, 2, dpb.current_references(), 0, 1_000_000, None)?;
        for cell in &mut frame.cells {
            cell.slice = 1;
        }
        let mut walker = syntax::Walker {
            frame: &mut frame,
            input: &input,
            location: fixture.layout.at_raster(0)?,
            slice: 1,
            dpb: Some(&dpb),
        };
        let cu = syntax::Area {
            x: 0,
            y: 0,
            log2: 3,
        };
        let pu = Partition::Horizontal.units(cu)?.0[0];
        let resolved = merge(&walker, pu, cu, Partition::Horizontal, 0, 0)?;
        assert!(matches!(resolved.motion, Motion::L0(_)));
        let lists = state(&walker)?.2;
        assert!(
            predict(
                &mut walker,
                pu,
                Motion::Bi([lists.get(0, 0, ZERO)?, lists.get(1, 0, ZERO)?]),
                false,
                false
            )
            .is_err()
        );
        assert_eq!(walker.frame.inter_stats.prediction_units, 0);
        let empty = crate::dpb::Dpb::new_reconstruction();
        walker.dpb = Some(&empty);
        assert!(predict(&mut walker, pu, resolved.motion, false, true).is_err());
        assert_eq!(walker.frame.inter_stats.prediction_units, 0);
        assert!(
            walker
                .frame
                .cells
                .iter()
                .all(|cell| cell.motion.is_none() && cell.reconstructed == 0)
        );
        let mut inter = controls([1, 1]);
        inter.collocated = Some(crate::slice::Collocated {
            from_l0: true,
            index: 0,
        });
        let input = fixture.input(0, inter);
        let mut frame =
            super::super::Frame::new_full(&input, 2, dpb.current_references(), 0, 1_000_000, None)?;
        let walker = syntax::Walker {
            frame: &mut frame,
            input: &input,
            location: fixture.layout.at_raster(0)?,
            slice: 1,
            dpb: Some(&empty),
        };
        assert!(temporal(&walker, pu, 0, 0).is_err());
        Ok(())
    }
}
