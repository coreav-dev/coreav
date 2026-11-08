use super::{ReadSupport, ReferenceRead};
use crate::reconstruction::{Cell, RetainedPicture};
use crate::{Error, PictureInfo, Result, dpb::ReferenceIdentity};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, RwLock};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Building,
    Finishing,
    Complete,
    Failed,
}

struct Progress {
    motion: usize,
    filtered: usize,
    outcome: Outcome,
    #[cfg(test)]
    waits: usize,
}

pub(crate) struct RowStore {
    identity: ReferenceIdentity,
    generation: u64,
    info: PictureInfo,
    deadline: Option<std::time::Instant>,
    backing_bytes: usize,
    completed: OnceLock<RetainedPicture>,
    storage: RwLock<Option<RetainedPicture>>,
    progress: Mutex<Progress>,
    changed: Condvar,
    #[cfg(test)]
    observing: Condvar,
}

pub(crate) struct RowPublisher {
    store: Arc<RowStore>,
}

impl Drop for RowPublisher {
    fn drop(&mut self) {
        self.store.fail();
    }
}

impl RowPublisher {
    #[cfg(test)]
    pub(crate) fn wait_for_reader(&self) -> Result<()> {
        let progress = self.store.progress()?;
        let (progress, _) = self
            .store
            .observing
            .wait_timeout_while(progress, std::time::Duration::from_secs(3), |progress| {
                progress.waits == 0
            })
            .map_err(|_| Error::Internal("reference_row_poison"))?;
        if progress.waits == 0 {
            return Err(Error::Internal("dependent_row_job_did_not_start"));
        }
        Ok(())
    }

    pub(crate) fn bytes(&self) -> Result<usize> {
        self.store.bytes()
    }

    pub(crate) fn fail(&self) {
        self.store.fail();
    }

    pub(crate) fn hash(
        &mut self,
        producer: &mut crate::reconstruction::raster::Producer,
        hash: crate::SeiInfo,
    ) -> Result<()> {
        let result = (|| {
            let storage = self
                .store
                .storage
                .read()
                .map_err(|_| Error::Internal("reference_row_poison"))?;
            let picture = storage
                .as_ref()
                .ok_or(Error::Internal("reference_row_storage"))?;
            producer.hash(
                hash,
                picture.planes.each_ref().map(|plane| plane.as_slice()),
            )
        })();
        if result.is_err() {
            self.store.fail();
        }
        result
    }

    pub(crate) fn complete(
        self,
        completion: crate::reconstruction::raster::Completion,
        timestamp: Option<i64>,
        origin: Option<u64>,
    ) -> Result<()> {
        self.finish(|picture| completion.retain(picture.planes, picture.cells, timestamp, origin))
    }

    pub(super) fn motion(&mut self, rows: core::ops::Range<usize>, cells: &[Cell]) -> Result<()> {
        let result = self.store.motion(rows, cells);
        if result.is_err() {
            self.store.fail();
        }
        result
    }

    pub(super) fn samples(
        &mut self,
        origin: usize,
        dirty: core::ops::Range<usize>,
        filtered: usize,
        planes: &[Vec<u16>; 3],
    ) -> Result<()> {
        let result = self.store.samples(origin, dirty, filtered, planes);
        if result.is_err() {
            self.store.fail();
        }
        result
    }

    pub(super) fn finish(
        self,
        finish: impl FnOnce(RetainedPicture) -> Result<RetainedPicture>,
    ) -> Result<()> {
        let mut storage = match self.store.storage.write() {
            Ok(storage) => storage,
            Err(error) => {
                drop(error.into_inner());
                return Err(Error::Internal("reference_row_poison"));
            }
        };
        let mut progress = self.store.progress()?;
        if progress.outcome != Outcome::Building
            || progress.motion != self.store.info.height as usize
            || progress.filtered != progress.motion
        {
            return Err(Error::InvalidData("selected_picture_incomplete"));
        }
        progress.outcome = Outcome::Finishing;
        let backing = storage
            .take()
            .ok_or(Error::Internal("reference_row_storage"))?;
        drop(progress);
        drop(storage);
        let picture = finish(backing)?;
        if picture.index != self.store.identity.index
            || picture.info != self.store.info
            || picture.heap_bytes()? != self.store.backing_bytes
        {
            return Err(Error::Internal("reference_row_identity"));
        }
        self.store
            .completed
            .set(picture)
            .map_err(|_| Error::Internal("reference_row_storage"))?;
        let mut progress = self.store.progress()?;
        if progress.outcome != Outcome::Finishing {
            return Err(Error::Internal("reference_row_failed"));
        }
        progress.outcome = Outcome::Complete;
        self.store.changed.notify_all();
        Ok(())
    }
}

impl RowStore {
    pub(crate) const fn record_bytes() -> usize {
        core::mem::size_of::<Self>() + 2 * core::mem::size_of::<usize>()
    }
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn take_picture(&mut self) -> Option<RetainedPicture> {
        (self.progress.get_mut().ok()?.outcome == Outcome::Complete)
            .then(|| self.completed.take())
            .flatten()
    }
    pub(crate) fn new(
        identity: ReferenceIdentity,
        generation: u64,
        backing: RetainedPicture,
        deadline: Option<std::time::Instant>,
        memory: &mut crate::reconstruction::Memory<'_>,
    ) -> Result<(Arc<Self>, RowPublisher)> {
        if identity.index != backing.index
            || backing
                .reference
                .is_some_and(|state| state.poc != identity.poc)
        {
            return Err(Error::Internal("reference_row_identity"));
        }
        let width = backing.info.width as usize;
        let height = backing.info.height as usize;
        let samples = width
            .checked_mul(height)
            .ok_or(Error::Limit("picture_samples"))?;
        if backing.planes[0].len() != samples
            || backing.planes[1].len() != samples / 4
            || backing.planes[2].len() != samples / 4
            || backing.cells.len() != samples / 16
        {
            return Err(Error::InvalidData("reference_row_storage"));
        }
        memory.charge(Self::record_bytes())?;
        let store = Arc::new(Self {
            identity,
            generation,
            info: backing.info,
            deadline,
            backing_bytes: backing.heap_bytes()?,
            completed: OnceLock::new(),
            storage: RwLock::new(Some(backing)),
            progress: Mutex::new(Progress {
                motion: 0,
                filtered: 0,
                outcome: Outcome::Building,
                #[cfg(test)]
                waits: 0,
            }),
            changed: Condvar::new(),
            #[cfg(test)]
            observing: Condvar::new(),
        });
        Ok((Arc::clone(&store), RowPublisher { store }))
    }

    pub(crate) fn bytes(&self) -> Result<usize> {
        Self::record_bytes()
            .checked_add(self.backing_bytes)
            .ok_or(Error::Limit("reconstruction_memory"))
    }

    pub(crate) fn picture(&self) -> Option<&RetainedPicture> {
        (self.progress().ok()?.outcome == Outcome::Complete)
            .then(|| self.completed.get())
            .flatten()
    }

    fn progress(&self) -> Result<MutexGuard<'_, Progress>> {
        match self.progress.lock() {
            Ok(progress) => Ok(progress),
            Err(error) => {
                error.into_inner().outcome = Outcome::Failed;
                self.changed.notify_all();
                Err(Error::Internal("reference_row_poison"))
            }
        }
    }

    pub(crate) fn fail(&self) {
        let mut progress = self
            .progress
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if progress.outcome != Outcome::Complete {
            progress.outcome = Outcome::Failed;
        }
        self.changed.notify_all();
    }

    fn check_identity(&self, identity: ReferenceIdentity, generation: u64) -> Result<()> {
        if self.identity.index != identity.index
            || self.identity.poc != identity.poc
            || self.generation != generation
        {
            return Err(Error::InvalidData("reference_row_identity"));
        }
        Ok(())
    }

    pub(crate) fn info(&self, identity: ReferenceIdentity, generation: u64) -> Result<PictureInfo> {
        self.check_identity(identity, generation)?;
        if self.progress()?.outcome == Outcome::Failed {
            return Err(Error::Internal("reference_row_failed"));
        }
        Ok(self.info)
    }

    fn required(&self, support: ReadSupport) -> Result<usize> {
        let (last, scale) = match support {
            ReadSupport::Motion { row } => (row, 1),
            ReadSupport::Samples {
                component: 0,
                last_row,
            } => (last_row, 1),
            ReadSupport::Samples {
                component: 1 | 2,
                last_row,
            } => (last_row, 2),
            _ => return Err(Error::InvalidData("inter_reference_plane")),
        };
        last.checked_add(1)
            .and_then(|rows| rows.checked_mul(scale))
            .filter(|rows| *rows <= self.info.height as usize)
            .ok_or(Error::InvalidData("inter_reference_rows"))
    }

    fn ready(progress: &Progress, support: ReadSupport, required: usize) -> bool {
        progress.outcome == Outcome::Complete
            || progress.outcome == Outcome::Building
                && match support {
                    ReadSupport::Motion { .. } => progress.motion >= required,
                    ReadSupport::Samples { .. } => progress.filtered >= required,
                }
    }

    pub(crate) fn read(
        &self,
        identity: ReferenceIdentity,
        generation: u64,
        support: ReadSupport,
    ) -> Result<ReferenceRead<'_>> {
        let required = self.required(support)?;
        loop {
            self.check_identity(identity, generation)?;
            let mut progress = self.progress()?;
            while progress.outcome != Outcome::Failed && !Self::ready(&progress, support, required)
            {
                #[cfg(test)]
                {
                    progress.waits += 1;
                    self.observing.notify_all();
                }
                progress = if let Some(deadline) = self.deadline {
                    let remaining = deadline
                        .checked_duration_since(std::time::Instant::now())
                        .ok_or(Error::Limit("decode_deadline"))?;
                    match self.changed.wait_timeout(progress, remaining) {
                        Ok((progress, _)) => progress,
                        Err(error) => {
                            error.into_inner().0.outcome = Outcome::Failed;
                            self.changed.notify_all();
                            return Err(Error::Internal("reference_row_poison"));
                        }
                    }
                } else {
                    match self.changed.wait(progress) {
                        Ok(progress) => progress,
                        Err(error) => {
                            error.into_inner().outcome = Outcome::Failed;
                            self.changed.notify_all();
                            return Err(Error::Internal("reference_row_poison"));
                        }
                    }
                };
            }
            if progress.outcome == Outcome::Failed {
                return Err(Error::Internal("reference_row_failed"));
            }
            if progress.outcome == Outcome::Complete {
                return self
                    .completed
                    .get()
                    .map(|picture| ReferenceRead::new(picture, support))
                    .ok_or(Error::Internal("reference_row_storage"));
            }
            drop(progress);
            let storage = match self.storage.read() {
                Ok(storage) => storage,
                Err(error) => {
                    drop(error.into_inner());
                    self.fail();
                    return Err(Error::Internal("reference_row_poison"));
                }
            };
            let progress = self.progress()?;
            self.check_identity(identity, generation)?;
            if progress.outcome == Outcome::Failed {
                return Err(Error::Internal("reference_row_failed"));
            }
            if Self::ready(&progress, support, required) && progress.outcome == Outcome::Building {
                if storage.is_none() {
                    return Err(Error::Internal("reference_row_storage"));
                }
                drop(progress);
                return Ok(ReferenceRead::guarded(storage, support));
            }
        }
    }

    fn commit(
        &self,
        work: impl FnOnce(&mut RetainedPicture, &Progress) -> Result<(usize, usize)>,
    ) -> Result<()> {
        let mut storage = match self.storage.write() {
            Ok(storage) => storage,
            Err(error) => {
                drop(error.into_inner());
                self.fail();
                return Err(Error::Internal("reference_row_poison"));
            }
        };
        let progress = self.progress()?;
        if progress.outcome != Outcome::Building {
            return Err(Error::Internal("reference_row_failed"));
        }
        let frontiers = work(
            storage
                .as_mut()
                .ok_or(Error::Internal("reference_row_storage"))?,
            &progress,
        )?;
        drop(progress);
        drop(storage);
        let mut progress = self.progress()?;
        if progress.outcome != Outcome::Building {
            return Err(Error::Internal("reference_row_failed"));
        }
        progress.motion = frontiers.0;
        progress.filtered = frontiers.1;
        self.changed.notify_all();
        Ok(())
    }

    fn motion(&self, rows: core::ops::Range<usize>, cells: &[Cell]) -> Result<()> {
        self.commit(|picture, progress| {
            let width = self.info.width as usize / 4;
            let ctb = 1usize << self.info.ctb_log2;
            if rows.start != progress.motion
                || rows.end <= rows.start
                || rows.end != (rows.start + ctb).min(self.info.height as usize)
                || cells.len() != (rows.end - rows.start) / 4 * width
                || cells.iter().any(|cell| cell.reconstructed != 7)
            {
                return Err(Error::InvalidData("reference_motion_commit"));
            }
            picture
                .cells
                .get_mut(rows.start / 4 * width..rows.end / 4 * width)
                .ok_or(Error::InvalidData("reference_motion_commit"))?
                .copy_from_slice(cells);
            Ok((rows.end, progress.filtered))
        })
    }

    fn samples(
        &self,
        origin: usize,
        dirty: core::ops::Range<usize>,
        filtered: usize,
        planes: &[Vec<u16>; 3],
    ) -> Result<()> {
        self.commit(|picture, progress| {
            let ctb = 1usize << self.info.ctb_log2;
            if origin % 4 != 0
                || dirty.start != progress.filtered
                || dirty.start < origin
                || dirty.end != progress.motion
                || filtered != (progress.filtered + ctb).min(self.info.height as usize)
                || filtered > dirty.end
                || filtered <= progress.filtered
            {
                return Err(Error::InvalidData("reference_sample_commit"));
            }
            for component in 0..3 {
                let scale = if component == 0 { 1 } else { 2 };
                let width = self.info.width as usize / scale;
                planes
                    .get(component)
                    .and_then(|plane| {
                        plane.get(
                            (dirty.start - origin) / scale * width
                                ..(dirty.end - origin) / scale * width,
                        )
                    })
                    .ok_or(Error::InvalidData("reference_sample_commit"))?;
            }
            for (component, (source, destination)) in
                planes.iter().zip(&mut picture.planes).enumerate()
            {
                let scale = if component == 0 { 1 } else { 2 };
                let width = self.info.width as usize / scale;
                destination
                    .get_mut(dirty.start / scale * width..dirty.end / scale * width)
                    .ok_or(Error::InvalidData("reference_sample_commit"))?
                    .copy_from_slice(
                        source
                            .get(
                                (dirty.start - origin) / scale * width
                                    ..(dirty.end - origin) / scale * width,
                            )
                            .ok_or(Error::InvalidData("reference_sample_commit"))?,
                    );
            }
            Ok((progress.motion, filtered))
        })
    }
}

impl crate::reconstruction::raster::Sink for RowPublisher {
    fn motion(&mut self, band: crate::reconstruction::raster::MotionBand<'_>) -> Result<()> {
        self.motion(band.rows, band.cells)
    }

    fn samples(&mut self, band: crate::reconstruction::raster::SampleBand<'_>) -> Result<()> {
        self.samples(band.origin, band.dirty, band.filtered_through, band.planes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reconstruction::{Memory, ReferenceState};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    const IDENTITY: ReferenceIdentity = ReferenceIdentity {
        index: 0,
        poc: 0,
        long_term: false,
    };
    const GENERATION: u64 = 7;

    fn fixture() -> Result<(Arc<RowStore>, RowPublisher, RetainedPicture)> {
        fixture_deadline(Some(Instant::now() + Duration::from_secs(3)))
    }

    fn fixture_deadline(
        deadline: Option<Instant>,
    ) -> Result<(Arc<RowStore>, RowPublisher, RetainedPicture)> {
        let info = PictureInfo {
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
        let mut backing = RetainedPicture::for_test(0, info, None);
        backing.reference = Some(ReferenceState {
            poc: 0,
            references: Default::default(),
        });
        for plane in &mut backing.planes {
            plane.fill(u16::MAX);
        }
        for cell in &mut backing.cells {
            cell.reconstructed = 0;
        }
        let mut memory = Memory::new(backing.heap_bytes()?, usize::MAX)?;
        let (store, publisher) =
            RowStore::new(IDENTITY, GENERATION, backing, deadline, &mut memory)?;
        assert_eq!(store.bytes()?, memory.peak());
        Ok((store, publisher, RetainedPicture::for_test(0, info, None)))
    }

    fn await_waiters(store: &RowStore, count: usize) {
        let progress = store.progress.lock().unwrap();
        let (progress, _) = store
            .observing
            .wait_timeout_while(progress, Duration::from_secs(1), |progress| {
                progress.waits < count
            })
            .unwrap();
        assert!(progress.waits >= count);
    }

    #[test]
    fn scoped_row_reads_separate_motion_and_chroma_and_pin_the_committed_prefix() -> Result<()> {
        let (store, mut publisher, source) = fixture()?;
        publisher.motion(0..16, &source.cells[..32 * 16 / 16])?;
        publisher.motion(16..32, &source.cells[32 * 16 / 16..])?;
        publisher.samples(0, 0..32, 16, &source.planes)?;
        assert!(
            store
                .read(IDENTITY, GENERATION, ReadSupport::Motion { row: 17 })?
                .motion(0, 17)?
                .motion
                .is_none()
        );
        let prefix = store.read(
            IDENTITY,
            GENERATION,
            ReadSupport::Samples {
                component: 0,
                last_row: 15,
            },
        )?;
        let pointer = store.storage.read().unwrap().as_ref().unwrap().planes[0].as_ptr();
        let result = std::thread::scope(|scope| -> Result<RowPublisher> {
            let store = store.as_ref();
            let (ready, received) = mpsc::channel();
            let reader = scope.spawn(move || {
                let result = store
                    .read(
                        IDENTITY,
                        GENERATION,
                        ReadSupport::Samples {
                            component: 1,
                            last_row: 11,
                        },
                    )
                    .and_then(|read| read.plane().map(|_| ()));
                ready.send(result).unwrap();
            });
            await_waiters(store, 1);
            assert!(received.try_recv().is_err());
            let window: [Vec<u16>; 3] = core::array::from_fn(|component| {
                let scale = if component == 0 { 1 } else { 2 };
                source.planes[component][12 / scale * (32 / scale)..].to_vec()
            });
            let (enter, entered) = mpsc::channel();
            let (committed, committed_rx) = mpsc::channel();
            let writer = scope.spawn(move || -> Result<RowPublisher> {
                enter.send(()).unwrap();
                publisher.samples(12, 16..32, 32, &window)?;
                committed.send(()).unwrap();
                Ok(publisher)
            });
            entered.recv_timeout(Duration::from_secs(1)).unwrap();
            assert!(matches!(
                committed_rx.recv_timeout(Duration::from_millis(30)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ));
            drop(prefix);
            committed_rx.recv_timeout(Duration::from_secs(1)).unwrap();
            received.recv_timeout(Duration::from_secs(1)).unwrap()?;
            reader.join().unwrap();
            writer.join().unwrap()
        })?;
        result.finish(Ok)?;
        let picture = store.picture().unwrap();
        assert_eq!(picture.planes, source.planes);
        assert_eq!(picture.planes[0].as_ptr(), pointer);
        assert!(
            store
                .read(
                    IDENTITY,
                    GENERATION,
                    ReadSupport::Samples {
                        component: 2,
                        last_row: 15
                    }
                )?
                .plane()
                .is_ok()
        );
        Ok(())
    }

    #[test]
    fn completion_moves_the_owner_outside_the_lock_and_wakes_transition_readers() -> Result<()> {
        let (store, mut publisher, source) = fixture()?;
        publisher.motion(0..16, &source.cells[..32])?;
        publisher.motion(16..32, &source.cells[32..])?;
        publisher.samples(0, 0..32, 16, &source.planes)?;
        publisher.samples(0, 16..32, 32, &source.planes)?;
        let pointer = store.storage.read().unwrap().as_ref().unwrap().planes[0].as_ptr() as usize;
        std::thread::scope(|scope| -> Result<()> {
            let (entered, entering) = mpsc::channel();
            let (release, released) = mpsc::channel();
            let writer = scope.spawn(move || {
                publisher.finish(|picture| {
                    entered.send(()).unwrap();
                    released.recv_timeout(Duration::from_secs(1)).unwrap();
                    Ok(picture)
                })
            });
            entering.recv_timeout(Duration::from_secs(1)).unwrap();
            assert!(store.storage.try_read().unwrap().is_none());
            assert!(store.picture().is_none());
            let reader = scope.spawn(|| {
                store
                    .read(IDENTITY, GENERATION, ReadSupport::Motion { row: 0 })
                    .map(|_| ())
            });
            await_waiters(&store, 1);
            release.send(()).unwrap();
            writer.join().unwrap()?;
            reader.join().unwrap()?;
            Ok(())
        })?;
        assert_eq!(
            store.picture().unwrap().planes[0].as_ptr() as usize,
            pointer
        );
        Ok(())
    }

    #[test]
    fn dropping_the_only_publisher_wakes_motion_and_sample_waiters() -> Result<()> {
        let (store, publisher, _) = fixture()?;
        std::thread::scope(|scope| {
            let store = store.as_ref();
            let (done, receiver) = mpsc::channel();
            let first = done.clone();
            scope.spawn(move || {
                first
                    .send(
                        store
                            .read(IDENTITY, GENERATION, ReadSupport::Motion { row: 31 })
                            .map(|_| ()),
                    )
                    .unwrap();
            });
            scope.spawn(move || {
                done.send(
                    store
                        .read(
                            IDENTITY,
                            GENERATION,
                            ReadSupport::Samples {
                                component: 0,
                                last_row: 31,
                            },
                        )
                        .map(|_| ()),
                )
                .unwrap();
            });
            await_waiters(store, 2);
            drop(publisher);
            for _ in 0..2 {
                assert!(matches!(
                    receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
                    Err(Error::Internal("reference_row_failed"))
                ));
            }
        });
        Ok(())
    }

    #[test]
    fn invalid_commits_latch_failure_without_overwriting_published_samples() -> Result<()> {
        let (store, mut publisher, source) = fixture()?;
        assert!(matches!(
            store.info(IDENTITY, GENERATION + 1),
            Err(Error::InvalidData("reference_row_identity"))
        ));
        assert!(matches!(
            store.read(
                IDENTITY,
                GENERATION,
                ReadSupport::Samples {
                    component: 3,
                    last_row: 0
                }
            ),
            Err(Error::InvalidData("inter_reference_plane"))
        ));
        publisher.motion(0..16, &source.cells[..32])?;
        publisher.motion(16..32, &source.cells[32..])?;
        publisher.samples(0, 0..32, 16, &source.planes)?;
        assert!(matches!(
            publisher.samples(0, 0..32, 32, &source.planes),
            Err(Error::InvalidData("reference_sample_commit"))
        ));
        assert!(matches!(
            store.read(IDENTITY, GENERATION, ReadSupport::Motion { row: 0 }),
            Err(Error::Internal("reference_row_failed"))
        ));
        assert_eq!(
            store.storage.read().unwrap().as_ref().unwrap().planes,
            source.planes
        );
        Ok(())
    }

    #[test]
    fn row_wait_deadlines_and_storage_poison_report_their_original_failures() -> Result<()> {
        let (store, _publisher, _) = fixture_deadline(Some(Instant::now()))?;
        assert!(matches!(
            store.read(IDENTITY, GENERATION, ReadSupport::Motion { row: 0 }),
            Err(Error::Limit("decode_deadline"))
        ));
        let (store, mut publisher, source) = fixture()?;
        publisher.motion(0..16, &source.cells[..32])?;
        publisher.samples(0, 0..16, 16, &source.planes)?;
        std::thread::scope(|scope| {
            assert!(
                scope
                    .spawn(|| {
                        let _guard = store.storage.write().unwrap();
                        panic!("row storage poison");
                    })
                    .join()
                    .is_err()
            );
        });
        assert!(matches!(
            store.read(IDENTITY, GENERATION, ReadSupport::Motion { row: 0 }),
            Err(Error::Internal("reference_row_poison"))
        ));
        assert!(matches!(
            store.read(
                IDENTITY,
                GENERATION,
                ReadSupport::Samples {
                    component: 0,
                    last_row: 0
                }
            ),
            Err(Error::Internal("reference_row_failed"))
        ));
        Ok(())
    }
}
