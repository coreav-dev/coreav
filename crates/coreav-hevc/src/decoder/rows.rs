use super::{DecodeLimits, scheduler};
use crate::{
    Error, PictureInfo, Result,
    dpb::Publication,
    native_rows::{OutputHeader, RowReceive, SinkCost},
    params::Sps,
    reconstruction::continuation_capacity,
};
use std::sync::Arc;

pub(super) const ATOMIC_RUNTIME_BYTES: usize = 65536;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_receive_offsets_follow_unreturned_old_geometry_debt() -> Result<()> {
        let mut owner = Rows::allocate(
            Plan {
                ceiling: 4,
                workers: 4,
                lag: 3,
                frame_bytes: 384,
                reserved: 1 << 20,
            },
            (0, [0; 32]),
            0,
            3,
        )?;
        let rows = &mut owner[0];
        rows.validated = 0;
        let first = rows.receive()?;
        assert_eq!(first.consume, Some(0));
        assert!(!first.pop);
        let second = rows.receive()?;
        assert_eq!(second.consume, Some(1));
        assert!(!second.pop);
        rows.validated = 1;
        let third = rows.receive()?;
        assert_eq!(third.consume, Some(2));
        assert!(third.pop);
        assert_eq!(rows.debt()?, 2);
        let empty = rows.receive()?;
        assert!(empty.consume.is_none());
        assert!(!empty.pop);
        rows.validated = 3;
        for debt in [1, 0] {
            let catch_up = rows.receive()?;
            assert!(catch_up.consume.is_none());
            assert!(catch_up.pop);
            assert_eq!(rows.debt()?, debt);
        }
        Ok(())
    }
}

pub(super) struct Pending {
    pub(super) header: OutputHeader,
    pub(super) publication: Arc<Publication>,
    pub(super) fence: u64,
}

#[derive(Clone, Copy)]
pub(super) struct Plan {
    pub(super) ceiling: usize,
    pub(super) workers: usize,
    pub(super) lag: usize,
    pub(super) frame_bytes: usize,
    pub(super) reserved: usize,
}

pub(super) struct Rows {
    pub(super) identity: (u32, [u8; 32]),
    pub(super) plan: Plan,
    pub(super) pending: Vec<Pending>,
    pub(super) emitted: u64,
    pub(super) validated: u64,
    pub(super) serial_received: u64,
    pub(super) returned: u64,
    pub(super) reconciled: u64,
}

pub(super) struct Admission<'a> {
    pub(super) info: PictureInfo,
    pub(super) sps: &'a Sps,
    pub(super) limits: DecodeLimits,
    pub(super) fixed: usize,
    pub(super) sink: SinkCost,
    pub(super) allow_zero: bool,
}

fn sum(values: &[usize]) -> Result<usize> {
    values
        .iter()
        .try_fold(0usize, |total, value| total.checked_add(*value))
        .ok_or(Error::Limit("reconstruction_memory"))
}

fn frame_bytes(info: PictureInfo) -> Result<usize> {
    (info.visible_width as usize)
        .checked_mul(info.visible_height as usize)
        .and_then(|n| n.checked_mul(3))
        .and_then(|n| n.checked_div(2))
        .and_then(|n| n.checked_mul(if info.bit_depth == 10 { 2 } else { 1 }))
        .filter(|n| *n != 0)
        .ok_or(Error::Limit("reconstruction_memory"))
}

impl Admission<'_> {
    pub(super) fn choose(&self) -> Result<Option<Plan>> {
        if !cfg!(hevc_exact_capacities) {
            return Ok(None);
        }
        let overflow = || Error::Limit("reconstruction_memory");
        let area = (self.info.width as usize)
            .checked_mul(self.info.height as usize)
            .ok_or_else(overflow)?;
        let frame_bytes = frame_bytes(self.info)?;
        let capacity = continuation_capacity(self.info, self.sps.ctu_count as usize)?;
        let dpb = self
            .sps
            .max_dpb
            .get(self.sps.sub_layers as usize)
            .copied()
            .filter(|n| *n != 0)
            .ok_or(Error::Internal("row_dpb_capacity"))? as usize;
        let ctus = self.sps.ctu_count as usize;
        let nal = self
            .limits
            .headers
            .max_nal_bytes
            .min(self.sink.max_packet_bytes);
        let entries = (self.limits.headers.max_ctus as usize)
            .checked_sub(1)
            .and_then(|n| n.checked_mul(core::mem::size_of::<u64>()))
            .ok_or_else(overflow)?;
        let parser = sum(&[
            nal.checked_sub(2).ok_or_else(overflow)?,
            entries,
            ctus.checked_mul(core::mem::size_of::<core::ops::Range<usize>>())
                .ok_or_else(overflow)?,
        ])?;
        let pending_bytes = usize::try_from(self.sink.max_pending_bytes).map_err(|_| overflow())?;
        let caller_records = pending_bytes
            .checked_div(frame_bytes)
            .ok_or_else(overflow)?
            .min(usize::try_from(self.limits.headers.max_pictures).unwrap_or(usize::MAX));
        let ceiling = self.limits.threads.clamp(1, 16);
        let count = self
            .sink
            .queued_frames
            .checked_add(caller_records)
            .and_then(|n| n.checked_add(ceiling))
            .map(|n| n.min(usize::try_from(self.limits.headers.max_pictures).unwrap_or(usize::MAX)))
            .ok_or_else(overflow)?;
        let queue = if count > self.sink.queue_capacity {
            count
                .checked_mul(2)
                .and_then(|n| n.checked_sub(1))
                .ok_or_else(overflow)?
        } else {
            self.sink.queue_capacity
        };
        let output = sum(&[
            pending_bytes,
            queue
                .checked_mul(self.sink.record_bytes)
                .ok_or_else(overflow)?,
            count
                .checked_mul(self.sink.plane_records_bytes)
                .ok_or_else(overflow)?,
            self.sink.owner_bytes,
        ])?;
        let common = sum(&[
            self.fixed,
            parser,
            output,
            core::mem::size_of::<Rows>(),
            65536,
            ceiling
                .checked_mul(ATOMIC_RUNTIME_BYTES)
                .ok_or_else(overflow)?,
        ])?;
        let job =
            scheduler::continuation_job_bytes(self.info, &capacity, self.sink, dpb - 1, ctus, nal)?;
        let owner = sum(&[
            capacity.retained,
            Publication::record_bytes(),
            crate::reconstruction::RowStore::record_bytes(),
            65536,
        ])?;
        for lag in (1..ceiling).rev() {
            for workers in (lag + 1..=ceiling).rev() {
                let live = workers.checked_add(1).ok_or_else(overflow)?;
                let owners = (dpb - 1)
                    .checked_add(live)
                    .and_then(|n| n.checked_add(workers))
                    .ok_or_else(overflow)?;
                let base = sum(&[
                    common,
                    owners.checked_mul(owner).ok_or_else(overflow)?,
                    live.checked_mul(job).ok_or_else(overflow)?,
                    workers
                        .checked_mul(core::mem::size_of::<Pending>())
                        .ok_or_else(overflow)?,
                ])?;
                let debt = lag.checked_mul(frame_bytes).ok_or_else(overflow)?;
                let reserved = base
                    .checked_add(self.sink.debt_peak_bytes.saturating_sub(debt))
                    .ok_or_else(overflow)?;
                if reserved
                    .checked_add(debt)
                    .is_some_and(|bytes| bytes <= self.limits.max_memory_bytes)
                {
                    return Ok(Some(Plan {
                        ceiling,
                        workers,
                        lag,
                        frame_bytes,
                        reserved,
                    }));
                }
            }
        }
        if self.allow_zero {
            let reserved = sum(&[
                common,
                dpb.checked_mul(capacity.retained).ok_or_else(overflow)?,
                capacity.base,
                area.checked_mul(2).ok_or_else(overflow)?,
                self.sink.debt_peak_bytes,
            ])?;
            if reserved <= self.limits.max_memory_bytes {
                return Ok(Some(Plan {
                    ceiling,
                    workers: 1,
                    lag: 0,
                    frame_bytes,
                    reserved,
                }));
            }
        }
        Ok(None)
    }
}

impl Rows {
    pub(super) fn allocate(
        plan: Plan,
        identity: (u32, [u8; 32]),
        current: u64,
        queued: usize,
    ) -> Result<Box<[Self; 1]>> {
        let mut pending = Vec::new();
        if plan.lag != 0 {
            pending
                .try_reserve_exact(plan.workers)
                .map_err(|_| Error::Limit("picture_allocation"))?;
        }
        let mut owner = Vec::new();
        owner
            .try_reserve_exact(1)
            .map_err(|_| Error::Limit("picture_allocation"))?;
        let emitted = u64::try_from(queued).map_err(|_| Error::Limit("reconstruction_memory"))?;
        owner.push(Self {
            identity,
            plan,
            pending,
            emitted,
            validated: emitted,
            serial_received: 0,
            returned: 0,
            reconciled: current,
        });
        owner
            .into_boxed_slice()
            .try_into()
            .map_err(|_| Error::Internal("row_owner"))
    }

    pub(super) fn debt(&self) -> Result<usize> {
        self.serial_received
            .checked_sub(self.returned)
            .and_then(|n| usize::try_from(n).ok())
            .ok_or(Error::Internal("output_ordinal"))
    }

    pub(super) fn heap_bytes(&self) -> Result<usize> {
        let bytes = self
            .pending
            .capacity()
            .checked_mul(core::mem::size_of::<Pending>())
            .and_then(|n| n.checked_add(core::mem::size_of::<Self>()))
            .ok_or(Error::Limit("reconstruction_memory"))?;
        if bytes > self.plan.reserved {
            return Err(Error::Internal("row_admission"));
        }
        Ok(bytes)
    }

    pub(super) fn receive(&mut self) -> Result<RowReceive> {
        let consume = if self.serial_received < self.emitted {
            let index = self.debt()?;
            self.serial_received = self
                .serial_received
                .checked_add(1)
                .ok_or(Error::Limit("decode_work_count"))?;
            Some(index)
        } else {
            None
        };
        let pop = self.returned < self.validated;
        if pop {
            self.returned = self
                .returned
                .checked_add(1)
                .ok_or(Error::Limit("decode_work_count"))?;
        }
        Ok(RowReceive { consume, pop })
    }
}

impl<F: super::Delivery> super::Observer<'_, F> {
    pub(super) fn codec_bytes(&self, dpb: &crate::dpb::Dpb, scratch: usize) -> Result<usize> {
        sum(&[
            self.fixed,
            dpb.heap_bytes()?,
            self.pictures
                .frame
                .as_ref()
                .map_or(Ok(0), crate::reconstruction::Frame::heap_bytes)?,
            scratch,
            self.pictures.frame.scheduler.bytes(Some(dpb))?,
        ])
    }

    pub(super) fn row_publish(&mut self, dpb: &crate::dpb::Dpb, scratch: usize) -> Result<()> {
        loop {
            let Some(rows) = self.pictures.frame.scheduler.rows() else {
                return Ok(());
            };
            let Some(front) = rows.pending.first() else {
                return Ok(());
            };
            if front.fence > rows.reconciled {
                return Ok(());
            }
            let header = front.header;
            let publication = Arc::clone(&front.publication);
            let offset = rows
                .validated
                .checked_sub(rows.returned)
                .and_then(|n| usize::try_from(n).ok())
                .ok_or(Error::Internal("output_ordinal"))?;
            let picture = publication
                .picture()
                .ok_or(Error::InvalidData("missing_output_samples"))?;
            let view = picture.view();
            self.emit
                .row_sink()
                .ok_or(Error::Internal("row_sink"))?
                .fill(
                    offset,
                    crate::DecodedPicture {
                        index: header.index,
                        poc: header.poc,
                        timestamp: header.timestamp,
                        origin: header.origin,
                        info: view.info,
                        planes: view.planes,
                        stats: view.stats,
                        inter: picture.inter_stats(),
                    },
                )?;
            drop(publication);
            let rows = self
                .pictures
                .frame
                .scheduler
                .rows_mut()
                .ok_or(Error::Internal("row_owner"))?;
            rows.pending.remove(0);
            rows.validated = rows
                .validated
                .checked_add(1)
                .ok_or(Error::Limit("decode_work_count"))?;
            let bytes = self.codec_bytes(dpb, scratch)?;
            self.pictures.summary.memory_bytes = self
                .pictures
                .summary
                .memory_bytes
                .max(bytes.saturating_add(self.pictures.external_memory));
        }
    }

    pub(super) fn row_settle(&mut self, dpb: &crate::dpb::Dpb, scratch: usize) -> Result<()> {
        self.row_publish(dpb, scratch)?;
        while let Some(fence) = self
            .pictures
            .frame
            .scheduler
            .rows()
            .and_then(|rows| rows.pending.first().map(|front| front.fence))
        {
            let result = self.pictures.frame.scheduler.drain(
                &mut self.pictures.summary,
                &mut self.pictures.stream_remaining,
                fence.checked_sub(1),
            );
            self.row_publish(dpb, scratch)?;
            result?;
            if self.pictures.frame.scheduler.rows().is_some_and(|rows| {
                rows.pending
                    .first()
                    .is_some_and(|front| front.fence == fence)
            }) {
                return Err(Error::Internal("output_fence"));
            }
        }
        let result = self.pictures.synchronize();
        self.row_publish(dpb, scratch)?;
        result
    }

    pub(super) fn row_enforce(&mut self, dpb: &crate::dpb::Dpb, scratch: usize) -> Result<()> {
        self.row_publish(dpb, scratch)?;
        while self
            .pictures
            .frame
            .scheduler
            .rows()
            .is_some_and(|rows| rows.pending.len() > rows.plan.lag)
        {
            let fence = self
                .pictures
                .frame
                .scheduler
                .rows()
                .and_then(|rows| rows.pending.first())
                .map(|front| front.fence)
                .ok_or(Error::Internal("output_fence"))?;
            let result = self.pictures.frame.scheduler.drain(
                &mut self.pictures.summary,
                &mut self.pictures.stream_remaining,
                fence.checked_sub(1),
            );
            self.row_publish(dpb, scratch)?;
            result?;
            if self.pictures.frame.scheduler.rows().is_some_and(|rows| {
                rows.pending
                    .first()
                    .is_some_and(|front| front.fence == fence)
            }) {
                return Err(Error::Internal("output_fence"));
            }
        }
        Ok(())
    }

    pub(super) fn row_output(
        &mut self,
        emission: crate::dpb::Emission<'_>,
        scratch: usize,
    ) -> Result<()> {
        self.row_publish(emission.dpb, scratch)?;
        let immediate = emission.publication.is_none()
            || self
                .pictures
                .frame
                .scheduler
                .rows()
                .is_some_and(|rows| rows.plan.lag == 0);
        if immediate {
            self.row_settle(emission.dpb, scratch)?;
        }
        let samples = emission.picture.or_else(|| {
            emission
                .publication
                .and_then(|publication| publication.picture())
        });
        let header = match samples {
            Some(samples) => OutputHeader {
                index: emission.event.picture_index,
                poc: emission.event.poc,
                timestamp: samples.timestamp(),
                origin: samples.origin(),
                info: samples.info(),
            },
            None => self
                .pictures
                .frame
                .scheduler
                .header(emission.event.picture_index, emission.event.poc)?,
        };
        let fence = self.pictures.frame.scheduler.fence()?;
        if self
            .pictures
            .frame
            .scheduler
            .rows()
            .is_none_or(|rows| frame_bytes(header.info).ok() != Some(rows.plan.frame_bytes))
        {
            return Err(Error::Internal("row_geometry"));
        }
        let codec_bytes = self.codec_bytes(emission.dpb, scratch)?;
        let mut memory = super::OutputMemory {
            codec_bytes,
            reserved: &mut self.pictures.external_memory,
            limit: self.pictures.limits.max_memory_bytes,
            reclaim: Some(&mut self.pictures.frame.cache),
        };
        let created = self
            .emit
            .row_sink()
            .ok_or(Error::Internal("row_sink"))?
            .prepare(header, &mut memory)?;
        if !created {
            return Ok(());
        }
        let rows = self
            .pictures
            .frame
            .scheduler
            .rows_mut()
            .ok_or(Error::Internal("row_owner"))?;
        let offset = rows
            .emitted
            .checked_sub(rows.returned)
            .and_then(|n| usize::try_from(n).ok())
            .ok_or(Error::Internal("output_ordinal"))?;
        rows.emitted = rows
            .emitted
            .checked_add(1)
            .ok_or(Error::Limit("decode_work_count"))?;
        if immediate {
            let samples = samples.ok_or(Error::InvalidData("missing_output_samples"))?;
            let view = samples.view();
            self.emit
                .row_sink()
                .ok_or(Error::Internal("row_sink"))?
                .fill(
                    offset,
                    crate::DecodedPicture {
                        index: header.index,
                        poc: header.poc,
                        timestamp: header.timestamp,
                        origin: header.origin,
                        info: header.info,
                        planes: view.planes,
                        stats: view.stats,
                        inter: samples.inter_stats(),
                    },
                )?;
            let rows = self
                .pictures
                .frame
                .scheduler
                .rows_mut()
                .ok_or(Error::Internal("row_owner"))?;
            rows.validated = rows
                .validated
                .checked_add(1)
                .ok_or(Error::Limit("decode_work_count"))?;
        } else {
            if rows.pending.len() == rows.pending.capacity() {
                return Err(Error::Internal("output_records"));
            }
            rows.pending.push(Pending {
                header,
                publication: Arc::clone(
                    emission
                        .publication
                        .ok_or(Error::Internal("output_publication"))?,
                ),
                fence,
            });
        }
        self.row_enforce(emission.dpb, scratch)
    }

    pub(super) fn row_admit(
        &mut self,
        input: &crate::parser::SliceInput<'_>,
        dpb: &mut crate::dpb::Dpb,
        frame_bytes: usize,
    ) -> Result<()> {
        let Some(identity) = self.activation else {
            return Ok(());
        };
        if self.emit.row_sink().is_none() || self.pictures.limits.threads == 1 {
            return Ok(());
        }
        if self
            .pictures
            .frame
            .scheduler
            .rows()
            .is_some_and(|rows| rows.identity == identity)
        {
            return Ok(());
        }
        if self.pictures.frame.scheduler.rows().is_none() {
            let sink = self
                .emit
                .row_sink()
                .ok_or(Error::Internal("row_sink"))?
                .cost(0, frame_bytes)?;
            let initial = Admission {
                info: input.picture,
                sps: input.sps,
                limits: self.pictures.limits,
                fixed: self.fixed,
                sink,
                allow_zero: false,
            }
            .choose();
            match initial {
                Ok(Some(_)) => {}
                Ok(None) | Err(Error::Limit("reconstruction_memory")) => return Ok(()),
                Err(error) => return Err(error),
            }
        }
        self.row_settle(dpb, input.scratch_bytes)?;
        dpb.unshare();
        self.pictures.frame.cache.clear();
        let debt = self
            .pictures
            .frame
            .scheduler
            .rows()
            .map_or(Ok(0), Rows::debt)?;
        if debt == 0 {
            self.pictures.frame.scheduler.rows = None;
        } else if let Some(rows) = self.pictures.frame.scheduler.rows_mut() {
            rows.pending = Vec::new();
        }
        let sink = self
            .emit
            .row_sink()
            .ok_or(Error::Internal("row_sink"))?
            .cost(debt, frame_bytes)?;
        let admission = Admission {
            info: input.picture,
            sps: input.sps,
            limits: self.pictures.limits,
            fixed: self.fixed,
            sink,
            allow_zero: debt != 0,
        };
        let plan = match admission.choose() {
            Ok(plan) => plan,
            Err(Error::Limit("reconstruction_memory")) if debt == 0 => None,
            Err(error) => return Err(error),
        };
        let Some(plan) = plan else {
            return if debt == 0 {
                Ok(())
            } else {
                Err(Error::Limit("row_admission"))
            };
        };
        if let Some(rows) = self.pictures.frame.scheduler.rows_mut() {
            rows.identity = identity;
            rows.plan = plan;
            if plan.lag != 0 {
                rows.pending
                    .try_reserve_exact(plan.workers)
                    .map_err(|_| Error::Limit("picture_allocation"))?;
            }
            rows.reconciled = rows.reconciled.max(input.index);
        } else {
            match Rows::allocate(plan, identity, input.index, sink.queued_frames) {
                Ok(rows) => self.pictures.frame.scheduler.rows = Some(rows),
                Err(Error::Limit("picture_allocation")) => return Ok(()),
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}
