use crate::{Error, LimitKind, Result};

pub(crate) struct Memory {
    limit: u64,
    used: u64,
}

impl Memory {
    pub(crate) fn new(limit: u64) -> Self {
        Self { limit, used: 0 }
    }

    pub(crate) fn charge<T>(&mut self, count: usize) -> Result<()> {
        let bytes = count
            .checked_mul(core::mem::size_of::<T>())
            .and_then(|n| u64::try_from(n).ok())
            .ok_or(Error::Limit(LimitKind::MemoryBytes))?;
        let used = self
            .used
            .checked_add(bytes)
            .filter(|&used| used <= self.limit)
            .ok_or(Error::Limit(LimitKind::MemoryBytes))?;
        self.used = used;
        Ok(())
    }

    pub(crate) fn empty<T>(&mut self, capacity: usize) -> Result<Vec<T>> {
        self.charge::<T>(capacity)?;
        let mut values = Vec::new();
        values
            .try_reserve_exact(capacity)
            .map_err(|_| Error::Limit(LimitKind::MemoryBytes))?;
        self.charge::<T>(values.capacity().saturating_sub(capacity))?;
        Ok(values)
    }

    pub(crate) fn filled<T: Copy>(&mut self, len: usize, value: T) -> Result<Vec<T>> {
        let mut values = self.empty(len)?;
        values.resize(len, value);
        Ok(values)
    }

    pub(crate) fn copy<T: Copy>(&mut self, source: &[T]) -> Result<Vec<T>> {
        let mut values = self.empty(source.len())?;
        values.extend_from_slice(source);
        Ok(values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_arithmetic_and_allocator_refusal_return_limit() {
        let mut memory = Memory::new(u64::MAX);
        assert!(matches!(
            memory.empty::<u16>(usize::MAX),
            Err(Error::Limit(LimitKind::MemoryBytes))
        ));
        let mut memory = Memory::new(u64::MAX);
        assert!(matches!(
            memory.empty::<u8>(isize::MAX as usize + 1),
            Err(Error::Limit(LimitKind::MemoryBytes))
        ));
    }

    #[test]
    fn every_request_including_dropped_temporaries_remains_charged() {
        let mut memory = Memory::new(12);
        let values = memory.filled::<u32>(2, 7).unwrap();
        assert_eq!(values, [7, 7]);
        drop(values);
        assert_eq!(memory.copy(&[9u32]), Ok(vec![9]));
        assert!(matches!(
            memory.empty::<u8>(1),
            Err(Error::Limit(LimitKind::MemoryBytes))
        ));
    }
}
