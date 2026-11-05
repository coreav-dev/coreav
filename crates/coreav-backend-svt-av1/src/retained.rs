//! Input storage defaults to remaining alive unless destruction succeeds.

pub(crate) struct Retained<T> {
    values: Option<(Vec<T>, T)>,
}

impl<T> Retained<T> {
    pub(crate) fn new(values: Vec<T>, eof: T) -> Self {
        Self {
            values: Some((values, eof)),
        }
    }

    pub(crate) fn get(&mut self) -> Option<(&mut Vec<T>, &mut T)> {
        self.values.as_mut().map(|(values, eof)| (values, eof))
    }

    pub(crate) fn release_after<E>(
        mut self,
        destroy: impl FnOnce() -> Result<(), E>,
    ) -> Result<(), E> {
        destroy()?;
        drop(self.values.take());
        Ok(())
    }
}

impl<T> Drop for Retained<T> {
    fn drop(&mut self) {
        if let Some(values) = self.values.take() {
            std::mem::forget(values);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Retained;
    use std::sync::{
        Arc, Barrier,
        atomic::{AtomicUsize, Ordering},
    };

    struct Watched(Arc<AtomicUsize>);

    impl Drop for Watched {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn storage(drops: &Arc<AtomicUsize>) -> Retained<Watched> {
        Retained::new(
            vec![Watched(Arc::clone(drops)), Watched(Arc::clone(drops))],
            Watched(Arc::clone(drops)),
        )
    }

    #[test]
    fn pictures_and_eof_outlive_delayed_work_and_successful_destruction() {
        let drops = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(2));
        let reader = {
            let drops = Arc::clone(&drops);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                assert_eq!(drops.load(Ordering::SeqCst), 0);
            })
        };
        let mut retained = storage(&drops);
        assert_eq!(retained.get().map(|(pictures, _)| pictures.len()), Some(2));
        let result = retained.release_after(|| {
            barrier.wait();
            reader.join().map_err(|_| ())?;
            assert_eq!(drops.load(Ordering::SeqCst), 0);
            Ok::<_, ()>(())
        });
        assert!(result.is_ok());
        assert_eq!(drops.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn failed_or_missing_destruction_never_releases_pictures_or_eof() {
        let drops = Arc::new(AtomicUsize::new(0));
        assert!(storage(&drops).release_after(|| Err::<(), _>(())).is_err());
        drop(storage(&drops));
        assert_eq!(drops.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn early_reclamation_control_is_detected() {
        let drops = Arc::new(AtomicUsize::new(0));
        let (pictures, eof) = (
            vec![Watched(Arc::clone(&drops))],
            Watched(Arc::clone(&drops)),
        );
        drop((pictures, eof));
        assert_ne!(drops.load(Ordering::SeqCst), 0);
    }
}
