use crate::{Error, LimitKind, Warning};
use coreav_isobmff::PlanningBudget;

pub(crate) fn limit(offset: u64) -> Error {
    Error::LimitExceeded {
        limit: LimitKind::PlanningBytes,
        offset,
    }
}

pub(crate) fn check(planning: Option<&PlanningBudget>, offset: u64) -> Result<(), Error> {
    if let Some(planning) = planning {
        planning.charge(0).map_err(|_| limit(offset))?;
    }
    Ok(())
}

pub(crate) fn reserve<T>(
    planning: Option<&PlanningBudget>,
    vector: &mut Vec<T>,
    additional: usize,
    offset: u64,
) -> Result<(), Error> {
    match planning {
        Some(planning) => planning
            .reserve(vector, additional)
            .map_err(|_| limit(offset)),
        None => vector.try_reserve(additional).map_err(|_| limit(offset)),
    }
}

pub(crate) fn exact<T>(
    planning: Option<&PlanningBudget>,
    vector: &mut Vec<T>,
    additional: usize,
    offset: u64,
) -> Result<(), Error> {
    match planning {
        Some(planning) => planning
            .reserve_exact(vector, additional)
            .map_err(|_| limit(offset)),
        None => vector
            .try_reserve_exact(additional)
            .map_err(|_| limit(offset)),
    }
}

pub(crate) fn copy(
    planning: Option<&PlanningBudget>,
    bytes: &[u8],
    offset: u64,
) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    exact(planning, &mut out, bytes.len(), offset)?;
    out.extend_from_slice(bytes);
    Ok(out)
}

pub(crate) fn string(
    planning: Option<&PlanningBudget>,
    bytes: &[u8],
    offset: u64,
) -> Result<String, Error> {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let bytes = bytes.get(..end).unwrap_or_default();
    let length = bytes
        .utf8_chunks()
        .try_fold(0usize, |length, chunk| {
            length
                .checked_add(chunk.valid().len())?
                .checked_add(if chunk.invalid().is_empty() { 0 } else { 3 })
        })
        .ok_or_else(|| limit(offset))?;
    let mut out = Vec::new();
    exact(planning, &mut out, length, offset)?;
    for chunk in bytes.utf8_chunks() {
        out.extend_from_slice(chunk.valid().as_bytes());
        if !chunk.invalid().is_empty() {
            out.extend_from_slice("\u{fffd}".as_bytes());
        }
    }
    String::from_utf8(out).map_err(|_| limit(offset))
}

pub(crate) fn warn(
    planning: Option<&PlanningBudget>,
    warnings: &mut Vec<Warning>,
    warning: Warning,
    offset: u64,
) -> Result<(), Error> {
    check(planning, offset)?;
    if !warnings.contains(&warning) {
        reserve(planning, warnings, 1, offset)?;
        warnings.push(warning);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusal_leaves_capacity_unallocated_and_preserves_prior_ownership() {
        let planning = PlanningBudget::new(3);
        let mut bytes = Vec::new();
        exact(Some(&planning), &mut bytes, 2, 7).unwrap();
        bytes.extend_from_slice(&[1u8, 2]);
        assert_eq!(bytes.capacity(), 2);
        assert_eq!(planning.used(), 2);
        assert_eq!(reserve(Some(&planning), &mut bytes, 1, 7), Err(limit(7)));
        assert_eq!(bytes.capacity(), 2);
        assert_eq!(bytes, [1, 2]);
        assert_eq!(planning.used(), 2);
        let mut untouched = Vec::<u64>::new();
        assert_eq!(exact(Some(&planning), &mut untouched, 1, 9), Err(limit(9)));
        assert_eq!(untouched.capacity(), 0);
    }

    #[test]
    fn lossy_string_capacity_is_charged_at_its_decoded_width() {
        let planning = PlanningBudget::new(2);
        assert_eq!(string(Some(&planning), &[0xff], 5), Err(limit(5)));
        assert_eq!(planning.used(), 0);
        let planning = PlanningBudget::new(3);
        let value = string(Some(&planning), &[0xff, 0, b'x'], 5).unwrap();
        assert_eq!(value, "\u{fffd}");
        assert_eq!(value.capacity(), 3);
        assert_eq!(planning.used(), 3);
        for bytes in [&b"valid"[..], &[0xf0, 0x80, b'a'], &[0xe2, 0x82]] {
            assert_eq!(
                string(None, bytes, 0).unwrap(),
                String::from_utf8_lossy(bytes)
            );
        }
    }
}
