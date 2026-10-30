//! State kept per PID, bounded by a limit fixed when the map is built
//! (`docs/design/mpegts.md`, "Per-PID state").
//!
//! Every per-PID map of the crate is a [`PidMap`]: it is built only from a
//! limit of [`Limits`] (or the 8,192 PIDs a packet can name), and an
//! insertion of a PID it does not hold yet is refused once it holds that
//! many. The crate's `clippy.toml` forbids the standard maps outside this
//! module, so a new per-PID fact cannot be held without a bound.
use crate::Limits;

/// PIDs a transport packet can name: 13 bits (ISO/IEC 13818-1 2.4.3.2).
const PIDS: usize = 1 << 13;

/// An insertion a [`PidMap`] refused: it holds as many PIDs as its limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Full;

/// A map from PID to `T` that holds at most as many PIDs as the limit it
/// was built with.
///
/// The values are kept in PID order, and a table over the PID space says
/// where each PID's value is, so that the lookups made for every transport
/// packet cost one index rather than a search. The table is made on the
/// first insertion: 16 KiB, whatever the limit.
#[derive(Clone, Debug)]
pub(crate) struct PidMap<T> {
    /// Per PID, the place of its entry plus one; 0 for a PID not held.
    places: Box<[u16]>,
    entries: Vec<(u16, T)>,
    limit: usize,
}

/// Two maps are equal when they hold the same PIDs and values, whatever
/// their limits.
impl<T: PartialEq> PartialEq for PidMap<T> {
    fn eq(&self, other: &Self) -> bool {
        self.entries == other.entries
    }
}

impl<T> PidMap<T> {
    /// At most `max_streams` PIDs: a fact of each stream.
    pub(crate) fn streams(limits: &Limits) -> Self {
        Self::bounded(usize::try_from(limits.max_streams).unwrap_or(usize::MAX))
    }

    /// At most `max_programs` PIDs: a fact of each program.
    pub(crate) fn programs(limits: &Limits) -> Self {
        Self::bounded(usize::try_from(limits.max_programs).unwrap_or(usize::MAX))
    }

    /// At most every PID a packet can name (8,192): a fact of each PID the
    /// file carries, whatever the tables declare, whose entry is small or
    /// whose bytes another limit bounds.
    pub(crate) fn pids() -> Self {
        Self::bounded(PIDS)
    }

    fn bounded(limit: usize) -> Self {
        Self {
            places: Box::default(),
            entries: Vec::new(),
            limit: limit.min(PIDS),
        }
    }

    /// An empty map with the same limit.
    pub(crate) fn cleared(&self) -> Self {
        Self::bounded(self.limit)
    }

    /// Whether it holds as many PIDs as its limit: a PID it does not hold
    /// yet would be refused.
    #[inline]
    pub(crate) fn is_full(&self) -> bool {
        self.entries.len() >= self.limit
    }

    /// The place of `pid`'s entry, if it holds one.
    #[inline]
    fn place(&self, pid: u16) -> Option<usize> {
        let place = *self.places.get(usize::from(pid))?;
        usize::from(place).checked_sub(1)
    }

    /// Gives the entries from `from` on their places in the table.
    fn renumber(&mut self, from: usize) {
        for (place, (pid, _)) in self.entries.iter().enumerate().skip(from) {
            if let (Some(slot), Ok(place)) = (
                self.places.get_mut(usize::from(*pid)),
                u16::try_from(place + 1),
            ) {
                *slot = place;
            }
        }
    }

    /// The place of `pid`'s entry, made with `value()` first if it has
    /// none; refused for a PID it does not hold yet once it is full, and
    /// for a value past the 13 bits of a PID.
    fn place_or_insert_with(&mut self, pid: u16, value: impl FnOnce() -> T) -> Result<usize, Full> {
        if let Some(place) = self.place(pid) {
            return Ok(place);
        }
        if self.is_full() || usize::from(pid) >= PIDS {
            return Err(Full);
        }
        if self.places.is_empty() {
            self.places = vec![0; PIDS].into_boxed_slice();
        }
        let place = self.entries.partition_point(|(held, _)| *held < pid);
        self.entries.insert(place, (pid, value()));
        self.renumber(place);
        Ok(place)
    }

    /// Sets the value of `pid` and returns the one it replaces; refused
    /// for a PID it does not hold yet once it is full.
    #[inline]
    pub(crate) fn insert(&mut self, pid: u16, value: T) -> Result<Option<T>, Full> {
        if let Some(held) = self.get_mut(&pid) {
            return Ok(Some(std::mem::replace(held, value)));
        }
        self.place_or_insert_with(pid, || value).map(|_| None)
    }

    /// Sets the value of `pid`, dropping the one it replaces in place;
    /// refused for a PID it does not hold yet once it is full. Unlike
    /// [`PidMap::insert`] it moves nothing back out, which matters for a
    /// value as large as a transport packet set on every packet.
    #[inline]
    pub(crate) fn set(&mut self, pid: u16, value: T) -> Result<(), Full> {
        if let Some(held) = self.get_mut(&pid) {
            *held = value;
            return Ok(());
        }
        self.place_or_insert_with(pid, || value).map(|_| ())
    }

    /// The value of `pid`, set to `value()` first if it has none; refused
    /// for a PID it does not hold yet once it is full.
    #[inline]
    pub(crate) fn get_or_insert_with(
        &mut self,
        pid: u16,
        value: impl FnOnce() -> T,
    ) -> Result<&mut T, Full> {
        let place = self.place_or_insert_with(pid, value)?;
        self.entries
            .get_mut(place)
            .map(|(_, held)| held)
            .ok_or(Full)
    }

    #[inline]
    pub(crate) fn get(&self, pid: &u16) -> Option<&T> {
        let place = self.place(*pid)?;
        self.entries.get(place).map(|(_, held)| held)
    }

    #[inline]
    pub(crate) fn get_mut(&mut self, pid: &u16) -> Option<&mut T> {
        let place = self.place(*pid)?;
        self.entries.get_mut(place).map(|(_, held)| held)
    }

    #[inline]
    pub(crate) fn contains_key(&self, pid: &u16) -> bool {
        self.place(*pid).is_some()
    }

    #[inline]
    pub(crate) fn remove(&mut self, pid: &u16) -> Option<T> {
        let place = self.place(*pid)?;
        if let Some(slot) = self.places.get_mut(usize::from(*pid)) {
            *slot = 0;
        }
        let (_, value) = self.entries.remove(place);
        self.renumber(place);
        Some(value)
    }

    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The PIDs held, in increasing order.
    #[inline]
    pub(crate) fn keys(&self) -> impl Iterator<Item = &u16> {
        self.entries.iter().map(|(pid, _)| pid)
    }

    /// The PIDs held and their values, in increasing PID order.
    #[inline]
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&u16, &T)> {
        self.entries.iter().map(|(pid, value)| (pid, value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_pid_is_refused_once_full() {
        let limits = Limits {
            max_streams: 2,
            ..Limits::default()
        };
        let mut map = PidMap::streams(&limits);
        assert_eq!(map.insert(0x100, 1), Ok(None));
        assert_eq!(map.insert(0x101, 2), Ok(None));
        assert!(map.is_full());
        assert_eq!(map.insert(0x102, 3), Err(Full));
        assert_eq!(map.get_or_insert_with(0x102, || 3), Err(Full));
        assert_eq!(map.set(0x102, 3), Err(Full));
        assert_eq!(map.set(0x101, 6), Ok(()));
        assert_eq!(map.insert(0x101, 2), Ok(Some(6)));
        assert!(!map.contains_key(&0x102));
        // A PID it holds is updated all the same.
        assert_eq!(map.insert(0x100, 4), Ok(Some(1)));
        assert_eq!(map.get_or_insert_with(0x101, || 5), Ok(&mut 2));
        assert_eq!(map.remove(&0x101), Some(2));
        assert_eq!(map.insert(0x102, 3), Ok(None));
        assert_eq!(map.keys().copied().collect::<Vec<_>>(), [0x100, 0x102]);
    }

    #[test]
    fn every_pid_fits_the_pid_space() {
        let mut map = PidMap::pids();
        for pid in 0..0x2000u16 {
            assert_eq!(map.insert(pid, ()), Ok(None));
        }
        assert!(map.is_full());
        assert_eq!(map.cleared().insert(0, ()), Ok(None));
        assert_eq!(map.insert(0x2000, ()), Err(Full));
    }

    #[test]
    fn the_program_limit_bounds_a_map_of_programs() {
        let limits = Limits {
            max_programs: 1,
            ..Limits::default()
        };
        let mut map = PidMap::programs(&limits);
        assert_eq!(map.insert(0x1000, ()), Ok(None));
        assert_eq!(map.insert(0x1001, ()), Err(Full));
    }
}
