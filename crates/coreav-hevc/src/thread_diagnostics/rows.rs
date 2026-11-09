use std::cell::RefCell;

use crate::{Error, PictureInfo, Result, dpb::ReferenceIdentity};

#[derive(Clone, Copy, Default)]
#[doc(hidden)]
pub struct RowReference {
    pub index: u64,
    pub poc: i32,
    pub sample_row: Option<usize>,
    pub motion_row: Option<usize>,
}

#[derive(Default)]
#[doc(hidden)]
pub struct RowDependency {
    pub references: [Option<RowReference>; 16],
    pub sample_calls: u64,
    pub motion_calls: u64,
}

#[doc(hidden)]
pub struct RowPicture {
    pub index: u64,
    pub poc: i32,
    pub info: PictureInfo,
    pub rows: Vec<RowDependency>,
}

#[derive(Clone, Copy)]
pub(crate) struct ReadContext {
    pub picture: u64,
    pub row: usize,
    pub reference: ReferenceIdentity,
    pub reference_ctb_log2: u32,
    pub scale: usize,
}

struct Recorder {
    pictures: [Option<RowPicture>; 32],
    context: Option<ReadContext>,
    error: Option<Error>,
}

thread_local! {
    static ROWS: RefCell<Option<Recorder>> = const { RefCell::new(None) };
}

#[doc(hidden)]
pub fn begin_rows() -> Result<()> {
    ROWS.with_borrow_mut(|state| {
        if state.is_some() {
            return Err(Error::InvalidData("row_trace_active"));
        }
        *state = Some(Recorder {
            pictures: [const { None }; 32],
            context: None,
            error: None,
        });
        Ok(())
    })
}

#[doc(hidden)]
pub fn take_rows(max_workers: usize) -> Result<Vec<RowPicture>> {
    let state = ROWS.take().ok_or(Error::InvalidData("row_trace_missing"))?;
    if max_workers != 1 {
        return Err(Error::Unsupported("row_trace_workers"));
    }
    if let Some(error) = state.error {
        return Err(error);
    }
    let mut pictures = Vec::new();
    pictures
        .try_reserve_exact(32)
        .map_err(|_| Error::Limit("row_trace_memory"))?;
    pictures.extend(state.pictures.into_iter().flatten());
    Ok(pictures)
}

pub(crate) fn check_row_threads(threads: usize) -> Result<()> {
    ROWS.with_borrow_mut(|state| {
        let Some(state) = state else {
            return Ok(());
        };
        if threads == 1 {
            return Ok(());
        }
        state.error = Some(Error::Unsupported("row_trace_workers"));
        Err(Error::Unsupported("row_trace_workers"))
    })
}

pub(crate) fn row_picture(index: u64, poc: i32, info: PictureInfo) -> Result<()> {
    ROWS.with_borrow_mut(|state| {
        let Some(state) = state else {
            return Ok(());
        };
        if info.wpp {
            return Err(Error::Unsupported("row_trace_wpp"));
        }
        let count = info.height.div_ceil(1 << info.ctb_log2) as usize;
        if count == 0 || count > 2048 {
            return Err(Error::Limit("row_trace_rows"));
        }
        let slot = state
            .pictures
            .get_mut(index as usize)
            .ok_or(Error::Limit("row_trace_pictures"))?;
        if slot.is_some() {
            return Err(Error::InvalidData("row_trace_duplicate_picture"));
        }
        let mut rows = Vec::new();
        rows.try_reserve_exact(count)
            .map_err(|_| Error::Limit("row_trace_memory"))?;
        rows.resize_with(count, RowDependency::default);
        *slot = Some(RowPicture {
            index,
            poc,
            info,
            rows,
        });
        Ok(())
    })
}

pub(crate) struct ReadGuard {
    previous: Option<ReadContext>,
    active: bool,
}

impl ReadGuard {
    pub(crate) fn new(context: ReadContext) -> Self {
        ROWS.with_borrow_mut(|state| match state {
            Some(state) => Self {
                previous: state.context.replace(context),
                active: true,
            },
            None => Self {
                previous: None,
                active: false,
            },
        })
    }
}

impl Drop for ReadGuard {
    fn drop(&mut self) {
        if self.active {
            ROWS.with_borrow_mut(|state| {
                if let Some(state) = state {
                    state.context = self.previous;
                }
            });
        }
    }
}

fn record(last: usize, samples: bool) {
    ROWS.with_borrow_mut(|state| {
        let Some(state) = state else {
            return;
        };
        if state.error.is_some() {
            return;
        }
        let result = (|| {
            let context = state.context.ok_or(Error::Internal("row_trace_context"))?;
            let row = state
                .pictures
                .get_mut(context.picture as usize)
                .and_then(Option::as_mut)
                .and_then(|picture| picture.rows.get_mut(context.row))
                .ok_or(Error::Internal("row_trace_row"))?;
            let at = row
                .references
                .iter()
                .position(|entry| entry.is_none_or(|entry| entry.index == context.reference.index))
                .ok_or(Error::Limit("row_trace_references"))?;
            let reference = row
                .references
                .get_mut(at)
                .ok_or(Error::Internal("row_trace_reference"))?
                .get_or_insert(RowReference {
                    index: context.reference.index,
                    poc: context.reference.poc,
                    ..RowReference::default()
                });
            if reference.poc != context.reference.poc {
                return Err(Error::Internal("row_trace_reference_poc"));
            }
            let maximum = if samples {
                last.checked_mul(context.scale)
                    .and_then(|value| value.checked_add(context.scale - 1))
                    .ok_or(Error::Limit("row_trace_coordinate"))?
            } else {
                last
            } >> context.reference_ctb_log2;
            let (bound, calls) = if samples {
                (&mut reference.sample_row, &mut row.sample_calls)
            } else {
                (&mut reference.motion_row, &mut row.motion_calls)
            };
            *bound = Some(bound.map_or(maximum, |value| value.max(maximum)));
            *calls = calls
                .checked_add(1)
                .ok_or(Error::Limit("row_trace_calls"))?;
            Ok(())
        })();
        if let Err(error) = result {
            state.error = Some(error);
        }
    });
}

pub(crate) fn sample_read(_first: usize, last: usize) {
    record(last, true);
}

pub(crate) fn motion_read(last: usize) {
    record(last, false);
}
