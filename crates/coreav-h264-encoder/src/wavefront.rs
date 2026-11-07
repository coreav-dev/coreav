#![forbid(unsafe_code)]

use crate::{Error, Result};
use std::sync::{Condvar, Mutex, OnceLock, RwLock, mpsc};

type Row<T> = Vec<OnceLock<T>>;

fn cells<T>(width: usize) -> Result<Row<T>> {
    let mut row = Vec::new();
    row.try_reserve_exact(width)
        .map_err(|_| Error::Allocation)?;
    row.resize_with(width, OnceLock::new);
    Ok(row)
}

pub(crate) fn run<T: Send + Sync>(
    width: usize,
    height: usize,
    threads: usize,
    compute: impl Fn(usize, &[OnceLock<T>], &[OnceLock<T>]) -> Result<T> + Sync,
    mut emit: impl FnMut(usize, &[OnceLock<T>], &[OnceLock<T>]) -> Result<()>,
) -> Result<()> {
    if width == 0 || height == 0 || threads == 0 {
        return Err(Error::Internal);
    }
    width.checked_mul(height).ok_or(Error::Internal)?;
    let workers = threads.min(height).min(width.div_ceil(2));
    if workers > 1 && parallel(width, height, workers, &compute, &mut emit)? {
        return Ok(());
    }
    serial(width, height, &compute, &mut emit)
}

fn serial<T>(
    width: usize,
    height: usize,
    compute: &impl Fn(usize, &[OnceLock<T>], &[OnceLock<T>]) -> Result<T>,
    emit: &mut impl FnMut(usize, &[OnceLock<T>], &[OnceLock<T>]) -> Result<()>,
) -> Result<()> {
    let mut current = cells(width)?;
    let mut top = cells(if height > 1 { width } else { 0 })?;
    let mut emission_error = None;
    for row in 0..height {
        let previous = if row == 0 { &[][..] } else { &top };
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
            for x in 0..width {
                let value = compute(row * width + x, &current, previous)?;
                current
                    .get(x)
                    .ok_or(Error::Internal)?
                    .set(value)
                    .map_err(|_| Error::Internal)?;
            }
            Ok(())
        }))
        .unwrap_or(Err(Error::Internal))?;
        if emission_error.is_none() {
            emission_error = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                emit(row, &current, previous)
            }))
            .unwrap_or(Err(Error::Internal))
            .err();
        }
        for slot in &mut top {
            slot.take();
        }
        std::mem::swap(&mut current, &mut top);
    }
    emission_error.map_or(Ok(()), Err)
}

fn parallel<T: Send + Sync>(
    width: usize,
    height: usize,
    workers: usize,
    compute: &(impl Fn(usize, &[OnceLock<T>], &[OnceLock<T>]) -> Result<T> + Sync),
    emit: &mut impl FnMut(usize, &[OnceLock<T>], &[OnceLock<T>]) -> Result<()>,
) -> Result<bool> {
    let mut window = Vec::new();
    let window_rows = height.min(workers + 1);
    window
        .try_reserve_exact(window_rows)
        .map_err(|_| Error::Allocation)?;
    for _ in 0..window_rows {
        window.push(RwLock::new(cells(width)?));
    }
    let mut progress = Vec::new();
    let mut senders = Vec::new();
    let mut completions = Vec::new();
    progress
        .try_reserve_exact(height)
        .map_err(|_| Error::Allocation)?;
    senders
        .try_reserve_exact(height)
        .map_err(|_| Error::Allocation)?;
    completions
        .try_reserve_exact(height)
        .map_err(|_| Error::Allocation)?;
    for _ in 0..height {
        let (sender, receiver) = mpsc::sync_channel::<Result<usize>>(width);
        progress.push((sender, Mutex::new(receiver)));
        let (sender, receiver) = mpsc::sync_channel(1);
        senders.push(sender);
        completions.push(receiver);
    }
    let retired = Mutex::new(0usize);
    let recycled = Condvar::new();
    let next_row = std::sync::atomic::AtomicUsize::new(0);
    let work = || {
        loop {
            let row = next_row.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if row >= height {
                break;
            }
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
                    let mut available = retired.lock().map_err(|_| Error::Internal)?;
                    while row >= *available + window_rows {
                        available = recycled.wait(available).map_err(|_| Error::Internal)?;
                    }
                    drop(available);
                    let current = window
                        .get(row % window_rows)
                        .ok_or(Error::Internal)?
                        .read()
                        .map_err(|_| Error::Internal)?;
                    let top = if row == 0 {
                        None
                    } else {
                        Some(
                            window
                                .get((row - 1) % window_rows)
                                .ok_or(Error::Internal)?
                                .read()
                                .map_err(|_| Error::Internal)?,
                        )
                    };
                    let preceding = if row == 0 {
                        None
                    } else {
                        Some(
                            progress
                                .get(row - 1)
                                .ok_or(Error::Internal)?
                                .1
                                .lock()
                                .map_err(|_| Error::Internal)?,
                        )
                    };
                    let mut completed = 0;
                    for x in 0..width {
                        if let Some(preceding) = &preceding {
                            while completed < (x + 2).min(width) {
                                completed = preceding.recv().map_err(|_| Error::Internal)??;
                            }
                        }
                        let value = compute(
                            row * width + x,
                            &current,
                            top.as_deref().map_or(&[], |row| row.as_slice()),
                        )?;
                        current
                            .get(x)
                            .ok_or(Error::Internal)?
                            .set(value)
                            .map_err(|_| Error::Internal)?;
                        if row + 1 < height {
                            progress
                                .get(row)
                                .ok_or(Error::Internal)?
                                .0
                                .send(Ok(x + 1))
                                .map_err(|_| Error::Internal)?;
                        }
                    }
                    Ok(())
                }))
                .unwrap_or(Err(Error::Internal));
            if let Err(error) = result {
                if let Some((sender, _)) = progress.get(row) {
                    let _ = sender.send(Err(error));
                }
            }
            if let Some(sender) = senders.get(row) {
                let _ = sender.send(result);
            }
        }
    };
    std::thread::scope(|scope| -> Result<bool> {
        let mut handles = Vec::new();
        handles
            .try_reserve_exact(workers)
            .map_err(|_| Error::Allocation)?;
        for _ in 0..workers {
            #[cfg(test)]
            if tests::SPAWN_LIMIT.with(|limit| handles.len() >= limit.get()) {
                break;
            }
            match std::thread::Builder::new().spawn_scoped(scope, work) {
                Ok(handle) => handles.push(handle),
                Err(_) => break,
            }
        }
        if handles.is_empty() {
            return Ok(false);
        }
        let mut decision_error = None;
        let mut emission_error = None;
        for (row, completion) in completions.iter().enumerate() {
            if let Err(error) = completion.recv().map_err(|_| Error::Internal)? {
                decision_error.get_or_insert(error);
            }
            if decision_error.is_none() && emission_error.is_none() {
                emission_error = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let current = window
                        .get(row % window_rows)
                        .ok_or(Error::Internal)?
                        .read()
                        .map_err(|_| Error::Internal)?;
                    let top = if row == 0 {
                        None
                    } else {
                        Some(
                            window
                                .get((row - 1) % window_rows)
                                .ok_or(Error::Internal)?
                                .read()
                                .map_err(|_| Error::Internal)?,
                        )
                    };
                    emit(
                        row,
                        &current,
                        top.as_deref().map_or(&[], |row| row.as_slice()),
                    )
                }))
                .unwrap_or(Err(Error::Internal))
                .err();
            }
            if row != 0 {
                let mut previous = window
                    .get((row - 1) % window_rows)
                    .ok_or(Error::Internal)?
                    .write()
                    .map_err(|_| Error::Internal)?;
                for slot in previous.iter_mut() {
                    slot.take();
                }
                drop(previous);
                *retired.lock().map_err(|_| Error::Internal)? += 1;
                recycled.notify_all();
            }
        }
        for handle in handles {
            if handle.join().is_err() {
                decision_error.get_or_insert(Error::Internal);
            }
        }
        decision_error.or(emission_error).map_or(Ok(true), Err)
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    std::thread_local! {
        pub(super) static SPAWN_LIMIT: std::cell::Cell<usize> = const { std::cell::Cell::new(usize::MAX) };
    }

    #[test]
    fn dependencies_are_published_before_dispatch_under_reordered_completions() {
        for (width, height) in [(1, 1), (1, 8), (2, 7), (3, 8), (12, 9)] {
            for threads in [1, 2, 3, 8] {
                let calls = AtomicUsize::new(0);
                let mut actual = Vec::new();
                let caller = std::thread::current().id();
                run(
                    width,
                    height,
                    threads,
                    |address, current, top| {
                        let (x, y) = (address % width, address / width);
                        let mut value = address;
                        if x != 0 {
                            value += *current[x - 1].get().unwrap();
                        }
                        if y != 0 {
                            value += *top[x].get().unwrap();
                            if x + 1 < width {
                                value += *top[x + 1].get().unwrap();
                            }
                        }
                        if address % 3 == 0 {
                            std::thread::yield_now();
                        }
                        calls.fetch_add(1, Ordering::Relaxed);
                        Ok(value % 1000)
                    },
                    |row, current, _| {
                        assert_eq!(std::thread::current().id(), caller);
                        assert_eq!(actual.len(), row * width);
                        actual.extend(current.iter().map(|slot| *slot.get().unwrap()));
                        std::thread::yield_now();
                        Ok(())
                    },
                )
                .unwrap();
                assert_eq!(calls.load(Ordering::Relaxed), width * height);
                let mut expected = vec![0; width * height];
                for address in 0..expected.len() {
                    let (x, y) = (address % width, address / width);
                    let mut value = address;
                    if x != 0 {
                        value += expected[address - 1];
                    }
                    if y != 0 {
                        value += expected[address - width];
                        if x + 1 < width {
                            value += expected[address - width + 1];
                        }
                    }
                    expected[address] = value % 1000;
                }
                assert_eq!(actual, expected);
            }
        }
    }

    #[test]
    fn earlier_rows_finish_before_selecting_the_first_raster_failure() {
        for threads in [1, 2, 3, 8] {
            let result = run(
                12,
                8,
                threads,
                |address, _, _| {
                    if address == 11 {
                        return Err(Error::Coefficient);
                    }
                    if address == 12 {
                        return Err(Error::MacroblockLimit);
                    }
                    if address < 11 {
                        std::thread::yield_now();
                    }
                    Ok(address)
                },
                |_, _, _| Ok(()),
            );
            assert!(matches!(result, Err(Error::Coefficient)));
        }
    }

    #[test]
    fn worker_unwind_is_reported_without_stranding_other_workers() {
        for threads in [1, 2, 3, 8] {
            let result = run(
                12,
                8,
                threads,
                |address, _, _| {
                    if address == 15 {
                        panic!("injected worker unwind");
                    }
                    Ok(address)
                },
                |_, _, _| Ok(()),
            );
            assert!(matches!(result, Err(Error::Internal)));
        }
        assert!(run(0, 8, 2, |_, _, _| Ok(()), |_, _, _| Ok(())).is_err());
        assert!(run(8, 0, 2, |_, _, _| Ok(()), |_, _, _| Ok(())).is_err());
        assert!(run(8, 8, 0, |_, _, _| Ok(()), |_, _, _| Ok(())).is_err());
        assert!(run(usize::MAX, 2, 1, |_, _, _| Ok(()), |_, _, _| Ok(())).is_err());
    }

    #[test]
    fn writer_failure_drains_decisions_and_preserves_decision_error_precedence() {
        for threads in [1, 2, 3, 8] {
            for late_failure in [false, true] {
                let calls = AtomicUsize::new(0);
                let mut emissions = 0;
                let result = run(
                    12,
                    40,
                    threads,
                    |address, _, _| {
                        calls.fetch_add(1, Ordering::Relaxed);
                        if late_failure && address == 12 * 39 {
                            return Err(Error::Coefficient);
                        }
                        Ok(address)
                    },
                    |_, _, _| {
                        emissions += 1;
                        Err(Error::Limit)
                    },
                );
                assert_eq!(
                    result,
                    Err(if late_failure {
                        Error::Coefficient
                    } else {
                        Error::Limit
                    })
                );
                assert_eq!(emissions, 1);
                if !late_failure {
                    assert_eq!(calls.load(Ordering::Relaxed), 12 * 40);
                }
            }
        }
    }

    #[test]
    fn caller_emission_overlaps_later_row_decisions() {
        for threads in [2, 4, 8] {
            let emitting = std::sync::atomic::AtomicBool::new(false);
            let (sender, receiver) = mpsc::sync_channel(1);
            run(
                24,
                20,
                threads,
                |address, _, _| {
                    if address == 24 {
                        while !emitting.load(Ordering::Acquire) {
                            std::thread::yield_now();
                        }
                    }
                    if address == 47 {
                        sender.send(()).unwrap();
                    }
                    Ok(address)
                },
                |row, _, _| {
                    if row == 0 {
                        emitting.store(true, Ordering::Release);
                        receiver
                            .recv_timeout(std::time::Duration::from_secs(5))
                            .unwrap();
                    }
                    Ok(())
                },
            )
            .unwrap();
        }
    }

    #[test]
    fn worker_start_refusals_keep_completed_rows_or_fall_back_to_the_caller() {
        for limit in [0, 1, 2] {
            SPAWN_LIMIT.with(|value| value.set(limit));
            let mut actual = Vec::new();
            let result = run(
                12,
                40,
                8,
                |address, _, _| Ok(address),
                |row, current, _| {
                    assert_eq!(actual.len(), row * 12);
                    actual.extend(current.iter().map(|slot| *slot.get().unwrap()));
                    Ok(())
                },
            );
            SPAWN_LIMIT.with(|value| value.set(usize::MAX));
            result.unwrap();
            assert_eq!(actual, (0..480).collect::<Vec<_>>());
        }
    }

    #[test]
    fn recycled_candidates_stay_inside_the_window_under_a_slow_caller() {
        struct Live<'a>(&'a AtomicUsize);
        impl Drop for Live<'_> {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::Relaxed);
            }
        }
        for threads in [1, 2, 3, 8] {
            let live = AtomicUsize::new(0);
            let peak = AtomicUsize::new(0);
            run(
                24,
                50,
                threads,
                |_, _, _| {
                    let count = live.fetch_add(1, Ordering::Relaxed) + 1;
                    peak.fetch_max(count, Ordering::Relaxed);
                    Ok(Live(&live))
                },
                |_, _, _| {
                    std::thread::sleep(std::time::Duration::from_micros(100));
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!(live.load(Ordering::Relaxed), 0);
            assert!(peak.load(Ordering::Relaxed) <= 24 * (threads + 1));
        }
    }
}
