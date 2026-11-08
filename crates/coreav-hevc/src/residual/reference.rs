use super::*;

pub(super) fn decode_levels(
    bins: &mut impl Bins,
    contexts: &mut Contexts,
    coefficients: &mut [Coefficient; 16],
    component: Component,
    group: usize,
    sign_data_hiding: bool,
    state: &mut LevelState,
) -> Result<[i16; 16]> {
    let set = greater_context_set(component, group, state.greater1);
    let mut greater_context = 1;
    let mut count = 0;
    let mut first = 16;
    let mut last = 0;
    let mut first_greater1 = None;
    for (position, coefficient) in coefficients.iter_mut().enumerate().rev() {
        if !coefficient.significant {
            continue;
        }
        if count == 0 {
            last = position;
        }
        first = position;
        if count < 8 {
            coefficient.greater1 = decision(
                bins,
                &mut contexts.greater1,
                component.chroma() * 16 + set * 4 + usize::from(greater_context),
            )?;
            greater_context = advance_greater_context(greater_context, coefficient.greater1);
            if coefficient.greater1 && first_greater1.is_none() {
                first_greater1 = Some(position);
            }
        }
        count += 1;
    }
    if count == 0 {
        return Ok([0; 16]);
    }
    state.greater1 = greater_context;
    if let Some(position) = first_greater1 {
        coefficients
            .get_mut(position)
            .ok_or(Error::InvalidData("residual_greater2"))?
            .greater2 = decision(bins, &mut contexts.greater2, component.chroma() * 4 + set)?;
    }
    let hidden = sign_data_hiding && last - first > 3;
    state.hidden_groups += u64::from(hidden);
    for (position, coefficient) in coefficients.iter_mut().enumerate().rev() {
        if coefficient.significant && !(hidden && position == first) {
            coefficient.negative = bins.bypass()?;
        }
    }
    let mut levels = [0; 16];
    let mut significant_count = 0;
    let mut sum = 0;
    let mut rice = 0;
    let mut previous_abs = 0;
    for (position, (coefficient, level)) in
        coefficients.iter().zip(levels.iter_mut()).enumerate().rev()
    {
        if !coefficient.significant {
            continue;
        }
        let base = 1 + u32::from(coefficient.greater1) + u32::from(coefficient.greater2);
        let threshold = if significant_count < 8 {
            if Some(position) == first_greater1 {
                3
            } else {
                2
            }
        } else {
            1
        };
        let remaining = if base == threshold {
            rice = next_rice(rice, previous_abs);
            let value = remaining_level(bins, rice)?;
            previous_abs = base + value;
            value
        } else {
            0
        };
        let magnitude = base + remaining;
        if magnitude > 32768 {
            return Err(Error::InvalidData("coefficient_level"));
        }
        sum += magnitude;
        let negative = coefficient.negative || (hidden && position == first && sum & 1 != 0);
        *level = signed_level(magnitude, negative)?;
        significant_count += 1;
    }
    Ok(levels)
}
