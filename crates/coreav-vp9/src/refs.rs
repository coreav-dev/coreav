//! Reference selection and its probability contexts (clauses 6.4.17 and 9.3.1).

type Pair = [i8; 2];

pub(crate) struct References {
    fixed: i8,
    variable: [i8; 2],
    fixed_second: bool,
}

impl References {
    pub(crate) fn new(biases: [bool; 4]) -> Self {
        let (fixed, variable) = if biases[1] == biases[2] {
            (3, [1, 2])
        } else if biases[1] == biases[3] {
            (2, [1, 3])
        } else {
            (1, [2, 3])
        };
        Self {
            fixed,
            variable,
            fixed_second: biases.get(fixed as usize).copied().unwrap_or(false),
        }
    }

    pub(crate) fn pair(&self, choice: bool) -> Pair {
        let variable = if choice {
            self.variable[1]
        } else {
            self.variable[0]
        };
        if self.fixed_second {
            [variable, self.fixed]
        } else {
            [self.fixed, variable]
        }
    }

    pub(crate) fn mode_context(&self, above: Option<Pair>, left: Option<Pair>) -> usize {
        match (above, left) {
            (Some(a), Some(l)) => match (a[1] <= 0, l[1] <= 0) {
                (true, true) => usize::from((a[0] == self.fixed) ^ (l[0] == self.fixed)),
                (true, false) => 2 + usize::from(a[0] == self.fixed || a[0] <= 0),
                (false, true) => 2 + usize::from(l[0] == self.fixed || l[0] <= 0),
                (false, false) => 4,
            },
            (Some(v), None) | (None, Some(v)) => {
                if v[1] > 0 {
                    3
                } else {
                    usize::from(v[0] == self.fixed)
                }
            }
            _ => 1,
        }
    }

    pub(crate) fn compound_context(&self, above: Option<Pair>, left: Option<Pair>) -> usize {
        let variable = |v: Pair| {
            if v[1] <= 0 || self.fixed_second {
                v[0]
            } else {
                v[1]
            }
        };
        let other = self.variable[1];
        match (above, left) {
            (Some(a), Some(l)) if a[0] <= 0 && l[0] <= 0 => 2,
            (Some(a), Some(l)) if a[0] <= 0 || l[0] <= 0 => {
                1 + 2 * usize::from(variable(if a[0] <= 0 { l } else { a }) != other)
            }
            (Some(a), Some(l)) => {
                let va = variable(a);
                let vl = variable(l);
                if va == other && vl == other {
                    return 0;
                }
                match (a[1] <= 0, l[1] <= 0) {
                    (true, true) => {
                        if (va == self.fixed && vl == self.variable[0])
                            || (vl == self.fixed && va == self.variable[0])
                        {
                            4
                        } else if va == vl {
                            3
                        } else {
                            1
                        }
                    }
                    (false, false) => {
                        if va == vl {
                            4
                        } else {
                            2
                        }
                    }
                    _ => {
                        let (single, compound) = if a[1] <= 0 { (va, vl) } else { (vl, va) };
                        if compound == other {
                            1
                        } else if single == other {
                            2
                        } else {
                            4
                        }
                    }
                }
            }
            (Some(v), None) | (None, Some(v)) if v[0] > 0 => {
                (if v[1] <= 0 { 3 } else { 4 }) * usize::from(variable(v) != other)
            }
            _ => 2,
        }
    }
}

pub(crate) fn single_contexts(above: Option<Pair>, left: Option<Pair>) -> [usize; 2] {
    let one = |v: Pair| {
        if v[0] <= 0 {
            2
        } else if v[1] <= 0 {
            4 * usize::from(v[0] == 1)
        } else {
            1 + usize::from(v.contains(&1))
        }
    };
    let first = match (above, left) {
        (Some(a), Some(l)) if a[0] <= 0 && l[0] <= 0 => 2,
        (Some(a), Some(l)) if a[0] <= 0 => one(l),
        (Some(a), Some(l)) if l[0] <= 0 => one(a),
        (Some(a), Some(l)) => match (a[1] <= 0, l[1] <= 0) {
            (true, true) => 2 * usize::from(a[0] == 1) + 2 * usize::from(l[0] == 1),
            (false, false) => 1 + usize::from(a.contains(&1) || l.contains(&1)),
            _ => {
                let (single, compound) = if a[1] <= 0 { (a, l) } else { (l, a) };
                3 * usize::from(single[0] == 1) + usize::from(compound.contains(&1))
            }
        },
        (Some(v), None) | (None, Some(v)) => one(v),
        _ => 2,
    };
    let one = |v: Pair| {
        if v[0] <= 0 || (v[0] == 1 && v[1] <= 0) {
            2
        } else if v[1] <= 0 {
            4 * usize::from(v[0] == 2)
        } else {
            3 * usize::from(v.contains(&2))
        }
    };
    let with_intra = |v: Pair| {
        if v[1] > 0 {
            1 + 2 * usize::from(v.contains(&2))
        } else if v[0] == 1 {
            3
        } else {
            4 * usize::from(v[0] == 2)
        }
    };
    let second = match (above, left) {
        (Some(a), Some(l)) if a[0] <= 0 && l[0] <= 0 => 2,
        (Some(a), Some(l)) if a[0] <= 0 => with_intra(l),
        (Some(a), Some(l)) if l[0] <= 0 => with_intra(a),
        (Some(a), Some(l)) => match (a[1] <= 0, l[1] <= 0) {
            (true, true) => match (a[0], l[0]) {
                (1, 1) => 3,
                (1, v) | (v, 1) => 4 * usize::from(v == 2),
                (a, l) => 2 * usize::from(a == 2) + 2 * usize::from(l == 2),
            },
            (false, false) => {
                if a == l {
                    3 * usize::from(a.contains(&2))
                } else {
                    2
                }
            }
            _ => {
                let (single, compound) = if a[1] <= 0 { (a, l) } else { (l, a) };
                let golden = usize::from(compound.contains(&2));
                match single[0] {
                    2 => 3 + golden,
                    3 => golden,
                    _ => 1 + 2 * golden,
                }
            }
        },
        (Some(v), None) | (None, Some(v)) => one(v),
        _ => 2,
    };
    [first, second]
}

#[cfg(test)]
mod tests;
