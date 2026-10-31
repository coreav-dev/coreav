//! The five writers `-of` names: `default`, `compact`, `csv`, `flat` and
//! `json`, each laid out as the pinned `ffprobe` lays it out
//! (`docs/design/compat-cli.md`, "Output fidelity"). The rules were read
//! from its output and its documentation, never from its source.
//!
//! A writer is driven by three calls: a section starts, an entry, a
//! section ends. It appends to a byte buffer, which the caller flushes.

use super::model::{Sid, Unit, Value};

/// Which writer, with its options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `default`.
    Default {
        /// `nokey`/`nk`.
        nokey: bool,
        /// `noprint_wrappers`/`nw`.
        noprint_wrappers: bool,
    },
    /// `compact`, and `csv`, which is `compact` with other defaults.
    Compact {
        /// `item_sep`/`s`.
        sep: u8,
        /// `nokey`/`nk`.
        nokey: bool,
        /// `escape`/`e`.
        escape: Escape,
        /// `print_section`/`p`.
        print_section: bool,
    },
    /// `flat`.
    Flat {
        /// `sep_char`/`s`.
        sep: u8,
        /// `hierarchical`/`h`.
        hierarchical: bool,
    },
    /// `json`.
    Json {
        /// `compact`/`c`.
        compact: bool,
    },
}

/// How `compact` escapes a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Escape {
    /// Backslash before the separator, a backslash and line breaks.
    C,
    /// Quoted, as a CSV field, when it holds the separator, a quote or a
    /// line break.
    Csv,
    /// As is.
    None,
}

/// How values are printed (`-sexagesimal`, `-pretty`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    /// `H:MM:SS.uuuuuu` times.
    pub sexagesimal: bool,
    /// Units with a prefix: `-pretty`, which also sets `sexagesimal`.
    pub units: bool,
}

struct Frame {
    sid: Sid,
    /// Entries and sections printed inside this one so far.
    items: usize,
    /// Elements printed so far, for an array.
    elements: usize,
    /// Of those, how many of each section, for an array that holds two
    /// kinds (`packets_and_frames`).
    kinds: Vec<(Sid, usize)>,
    /// What the text writers put before a key: `TAG:`, `tag:`,
    /// `streams.stream.0.`.
    prefix: String,
    /// The section stands on its own (a line in `compact`, a wrapper in
    /// `default`) rather than inside its parent's.
    element: bool,
    /// `json`: this object is printed on one line.
    inline: bool,
    /// `compact`: a typed section that printed its separator; the owner's
    /// count is given back when it ends.
    typed_open: bool,
}

/// A writer and its state.
pub struct Writer {
    kind: Kind,
    style: Style,
    stack: Vec<Frame>,
}

impl Writer {
    /// A writer of `kind`, printing values in `style`.
    pub fn new(kind: Kind, style: Style) -> Self {
        Writer {
            kind,
            style,
            stack: Vec::new(),
        }
    }

    fn level(&self) -> usize {
        self.stack.len()
    }

    /// The nearest enclosing section that stands on its own (`compact`).
    fn owner(&mut self) -> Option<&mut Frame> {
        self.stack.iter_mut().rev().find(|f| f.element)
    }

    /// A section starts. `typ` names the element's type for a typed
    /// section (`display_matrix`).
    pub fn header(&mut self, out: &mut Vec<u8>, sid: Sid, typ: Option<&str>) {
        let def = sid.def();
        let parent = self.stack.last().map(|f| f.sid);
        let parent_array = parent.is_some_and(|p| p.def().array);
        let at_root = parent == Some(Sid::Root);
        let level = self.level();
        let mut frame = Frame {
            sid,
            items: 0,
            elements: 0,
            kinds: Vec::new(),
            prefix: String::new(),
            element: false,
            inline: false,
            typed_open: false,
        };
        match &self.kind {
            Kind::Default {
                noprint_wrappers, ..
            } => {
                if sid == Sid::Root || def.array {
                    frame.prefix = self
                        .stack
                        .last()
                        .map(|f| f.prefix.clone())
                        .unwrap_or_default();
                } else if parent_array || at_root {
                    frame.element = true;
                    if !noprint_wrappers {
                        out.push(b'[');
                        out.extend(def.name.to_ascii_uppercase().as_bytes());
                        out.extend(b"]\n");
                    }
                } else {
                    let base = self.stack.last().map(|f| f.prefix.as_str()).unwrap_or("");
                    frame.prefix = format!("{base}{}:", def.element.to_ascii_uppercase());
                }
            }
            Kind::Compact {
                sep, print_section, ..
            } => {
                let (sep, print_section) = (*sep, *print_section);
                if sid == Sid::Root || def.array {
                    frame.prefix = self
                        .stack
                        .last()
                        .map(|f| f.prefix.clone())
                        .unwrap_or_default();
                } else if (parent_array && !def.typed) || at_root {
                    frame.element = true;
                    if self.owner().is_some_and(|o| o.items > 0) {
                        out.push(sep);
                    }
                    if print_section {
                        out.extend(def.name.as_bytes());
                        out.push(sep);
                    }
                } else {
                    let base = self.stack.last().map(|f| f.prefix.as_str()).unwrap_or("");
                    frame.prefix = match typ {
                        Some(t) if def.typed => format!("{base}{}/{t}:", def.element),
                        _ => format!("{base}{}:", def.element),
                    };
                    // The first typed section of a list starts with the
                    // separator, entries or not, and its first entry adds
                    // none; a later one adds separators only before its
                    // entries (found black box on side data lists of one,
                    // two and three items).
                    let first = self.stack.last().is_none_or(|f| f.elements == 0);
                    if def.typed && first {
                        if let Some(owner) = self.owner() {
                            if owner.items > 0 {
                                out.push(sep);
                                owner.items = 0;
                                frame.typed_open = true;
                            }
                        }
                    }
                }
            }
            Kind::Flat { sep, hierarchical } => {
                let sep = char::from(*sep);
                let base = self
                    .stack
                    .last()
                    .map(|f| f.prefix.clone())
                    .unwrap_or_default();
                if sid == Sid::Root {
                } else if def.array {
                    frame.prefix = if *hierarchical {
                        format!("{base}{}{sep}", def.name)
                    } else {
                        base
                    };
                } else if parent_array {
                    let n = self.stack.last().map_or(0, |f| {
                        f.kinds
                            .iter()
                            .find(|(s, _)| *s == sid)
                            .map_or(0, |(_, n)| *n)
                    });
                    frame.prefix = format!("{base}{}{sep}{n}{sep}", def.name);
                } else {
                    frame.prefix = format!("{base}{}{sep}", def.name);
                }
            }
            Kind::Json { compact } => {
                let compact = *compact;
                if sid == Sid::Root {
                    out.extend(b"{\n");
                } else {
                    let parent_items = self.stack.last().map_or(0, |f| f.items);
                    let parent_inline = self.stack.last().is_some_and(|f| f.inline);
                    if parent_items > 0 {
                        out.extend(b",\n");
                    }
                    if compact && parent_inline && parent_items == 0 {
                        // The parent's line goes on; the indentation
                        // still comes, as `ffprobe` prints it.
                    }
                    indent(out, level);
                    if !parent_array {
                        out.push(b'"');
                        out.extend(def.name.as_bytes());
                        out.extend(b"\": ");
                    }
                    if def.array {
                        out.extend(b"[\n");
                    } else if compact {
                        out.extend(b"{ ");
                        frame.inline = true;
                    } else {
                        out.extend(b"{\n");
                    }
                }
            }
        }
        self.stack.push(frame);
        // `json` names the kind of each element of a mixed array.
        if parent == Some(Sid::PacketsAndFrames) && matches!(self.kind, Kind::Json { .. }) {
            self.entry(out, "type", &Value::Str(def.name.to_owned()));
        }
    }

    /// A section ends.
    pub fn footer(&mut self, out: &mut Vec<u8>) {
        let Some(frame) = self.stack.pop() else {
            return;
        };
        let def = frame.sid.def();
        let level = self.level();
        match &self.kind {
            Kind::Default {
                noprint_wrappers, ..
            } => {
                if frame.element && !noprint_wrappers {
                    out.extend(b"[/");
                    out.extend(def.name.to_ascii_uppercase().as_bytes());
                    out.extend(b"]\n");
                }
            }
            Kind::Compact { .. } => {
                if frame.typed_open {
                    if let Some(owner) = self.owner() {
                        owner.items += 1;
                    }
                }
                if frame.element {
                    out.push(b'\n');
                    // The line ends here; the next element starts a new one.
                    if let Some(owner) = self.owner() {
                        owner.items = 0;
                    }
                }
            }
            Kind::Flat { .. } => {}
            Kind::Json { .. } => {
                if frame.sid == Sid::Root {
                    out.extend(b"\n}\n");
                } else if frame.inline {
                    out.extend(b" }");
                } else {
                    out.push(b'\n');
                    indent(out, level);
                    out.push(if def.array { b']' } else { b'}' });
                }
            }
        }
        if let Some(parent) = self.stack.last_mut() {
            parent.elements += 1;
            match parent.kinds.iter_mut().find(|(s, _)| *s == frame.sid) {
                Some((_, n)) => *n += 1,
                None => parent.kinds.push((frame.sid, 1)),
            }
            if !matches!(self.kind, Kind::Compact { .. }) {
                parent.items += 1;
            }
        }
    }

    /// An entry of the current section.
    pub fn entry(&mut self, out: &mut Vec<u8>, key: &str, value: &Value) {
        let style = self.style;
        let level = self.level();
        let Some(frame) = self.stack.last() else {
            return;
        };
        let inline = frame.inline;
        let items = frame.items;
        let prefix = frame.prefix.clone();
        match &self.kind {
            Kind::Default { nokey, .. } => {
                if !nokey {
                    out.extend(prefix.as_bytes());
                    out.extend(key.as_bytes());
                    out.push(b'=');
                }
                text(out, value, style);
                out.push(b'\n');
            }
            Kind::Compact {
                sep, nokey, escape, ..
            } => {
                let (sep, nokey, escape) = (*sep, *nokey, *escape);
                let element = frame.element;
                let owner_items = if element {
                    items
                } else {
                    self.stack
                        .iter()
                        .rev()
                        .find(|f| f.element)
                        .map_or(0, |f| f.items)
                };
                if owner_items > 0 {
                    out.push(sep);
                }
                if !nokey {
                    out.extend(prefix.as_bytes());
                    out.extend(key.as_bytes());
                    out.push(b'=');
                }
                let mut buf = Vec::new();
                text(&mut buf, value, style);
                escaped(out, &buf, sep, escape);
                if let Some(owner) = self.owner() {
                    owner.items += 1;
                }
                return;
            }
            Kind::Flat { .. } => {
                out.extend(prefix.as_bytes());
                flat_key(out, key);
                out.push(b'=');
                match value {
                    Value::Int(n) => out.extend(n.to_string().as_bytes()),
                    other => {
                        let mut buf = Vec::new();
                        text(&mut buf, other, style);
                        out.push(b'"');
                        for &b in &buf {
                            match b {
                                b'"' | b'\\' | b'`' | b'$' => {
                                    out.push(b'\\');
                                    out.push(b);
                                }
                                b'\n' => out.extend(b"\\n"),
                                _ => out.push(b),
                            }
                        }
                        out.push(b'"');
                    }
                }
                out.push(b'\n');
            }
            Kind::Json { .. } => {
                if matches!(value, Value::Na | Value::Unset(_)) {
                    return;
                }
                if items > 0 {
                    out.extend(if inline { &b", "[..] } else { &b",\n"[..] });
                }
                if !inline {
                    indent(out, level);
                }
                json_string(out, key.as_bytes());
                out.extend(b": ");
                match value {
                    Value::Int(n) => out.extend(n.to_string().as_bytes()),
                    other => {
                        let mut buf = Vec::new();
                        text(&mut buf, other, style);
                        json_string(out, &buf);
                    }
                }
            }
        }
        if let Some(frame) = self.stack.last_mut() {
            frame.items += 1;
        }
    }
}

fn indent(out: &mut Vec<u8>, level: usize) {
    for _ in 0..level {
        out.extend(b"    ");
    }
}

/// A key as `flat` prints it: what is not a letter, a digit or `_`
/// becomes `_`.
fn flat_key(out: &mut Vec<u8>, key: &str) {
    for b in key.bytes() {
        out.push(if b.is_ascii_alphanumeric() || b == b'_' {
            b
        } else {
            b'_'
        });
    }
}

fn escaped(out: &mut Vec<u8>, value: &[u8], sep: u8, escape: Escape) {
    match escape {
        Escape::None => out.extend(value),
        Escape::C => {
            for &b in value {
                match b {
                    0x08 => out.extend(b"\\b"),
                    0x0c => out.extend(b"\\f"),
                    b'\n' => out.extend(b"\\n"),
                    b'\r' => out.extend(b"\\r"),
                    b'\\' => out.extend(b"\\\\"),
                    _ if b == sep => {
                        out.push(b'\\');
                        out.push(b);
                    }
                    _ => out.push(b),
                }
            }
        }
        Escape::Csv => {
            let quote = value
                .iter()
                .any(|&b| b == b'"' || b == sep || b == b'\n' || b == b'\r');
            if quote {
                out.push(b'"');
            }
            for &b in value {
                if b == b'"' {
                    out.push(b'"');
                }
                out.push(b);
            }
            if quote {
                out.push(b'"');
            }
        }
    }
}

fn json_string(out: &mut Vec<u8>, value: &[u8]) {
    out.push(b'"');
    for &b in value {
        match b {
            b'"' => out.extend(b"\\\""),
            b'\\' => out.extend(b"\\\\"),
            0x08 => out.extend(b"\\b"),
            0x0c => out.extend(b"\\f"),
            b'\n' => out.extend(b"\\n"),
            b'\r' => out.extend(b"\\r"),
            b'\t' => out.extend(b"\\t"),
            0..0x20 => out.extend(format!("\\u{b:04x}").as_bytes()),
            _ => out.push(b),
        }
    }
    out.push(b'"');
}

/// A value as the text writers print it.
fn text(out: &mut Vec<u8>, value: &Value, style: Style) {
    match value {
        Value::Int(n) => out.extend(n.to_string().as_bytes()),
        Value::Str(s) => out.extend(s.as_bytes()),
        Value::Na => out.extend(b"N/A"),
        Value::Unset(word) => out.extend(word.as_bytes()),
        Value::Time(seconds) => {
            if style.sexagesimal || style.units {
                out.extend(sexagesimal(*seconds).as_bytes());
            } else {
                out.extend(fixed(*seconds, 6).as_bytes());
            }
        }
        Value::Unit(n, unit) => {
            if style.units {
                out.extend(with_unit(*n, *unit).as_bytes());
            } else {
                out.extend(n.to_string().as_bytes());
            }
        }
    }
}

/// `H:MM:SS.uuuuuu`, as `-sexagesimal` prints a time: hours and minutes
/// cut toward zero from the signed value, so a negative time keeps its sign
/// in every field that is not zero (`0:00:-0.339022`).
fn sexagesimal(seconds: f64) -> String {
    let mut secs = seconds;
    #[allow(clippy::cast_possible_truncation)]
    let hours = (secs / 3600.0) as i64;
    #[allow(clippy::cast_precision_loss)]
    {
        secs -= hours as f64 * 3600.0;
    }
    #[allow(clippy::cast_possible_truncation)]
    let mins = (secs / 60.0) as i64;
    #[allow(clippy::cast_precision_loss)]
    {
        secs -= mins as f64 * 60.0;
    }
    let s = fixed(secs, 6);
    format!("{hours}:{mins:02}:{s:0>9}")
}

/// A count with its unit and a prefix, as `-pretty` prints it: binary
/// prefixes for bytes, decimal for the rest, `%f` when the scaled value is
/// not whole.
fn with_unit(n: i64, unit: Unit) -> String {
    let (name, binary) = match unit {
        Unit::Byte => ("byte", true),
        Unit::BitRate => ("bit/s", false),
        Unit::Hertz => ("Hz", false),
    };
    // The largest power of the base not above the value, counted in
    // integers: what `floor(log(v) / log(base))` gives for a whole `v`,
    // without linking a maths library into every command.
    let base: u64 = if binary { 1024 } else { 1000 };
    let magnitude = n.unsigned_abs();
    let mut index = 0usize;
    let mut power = 1u64;
    while n > 0 && index + 1 < PREFIXES.len() {
        match power.checked_mul(base) {
            Some(next) if next <= magnitude => {
                power = next;
                index += 1;
            }
            _ => break,
        }
    }
    #[allow(clippy::cast_precision_loss)]
    let value = n as f64 / power as f64;
    let prefix = PREFIXES
        .get(index)
        .map_or("", |p| if binary { p.1 } else { p.0 });
    if n % i64::try_from(power).unwrap_or(1) == 0 {
        let v = n / i64::try_from(power).unwrap_or(1);
        format!("{v} {prefix}{name}")
    } else {
        format!("{} {prefix}{name}", fixed(value, 6))
    }
}

const PREFIXES: [(&str, &str); 6] = [
    ("", ""),
    ("K", "Ki"),
    ("M", "Mi"),
    ("G", "Gi"),
    ("T", "Ti"),
    ("P", "Pi"),
];

/// `value` with `places` decimals, rounded as C's `printf` rounds: from
/// the exact binary value, half to even.
pub fn fixed(value: f64, places: u32) -> String {
    if !value.is_finite() {
        return if value.is_nan() {
            "nan".to_owned()
        } else if value > 0.0 {
            "inf".to_owned()
        } else {
            "-inf".to_owned()
        };
    }
    let negative = value.is_sign_negative();
    let bits = value.abs().to_bits();
    let exponent = i32::try_from((bits >> 52) & 0x7ff).unwrap_or(0);
    let fraction = bits & ((1u64 << 52) - 1);
    let (mantissa, exp) = if exponent == 0 {
        (fraction, -1074)
    } else {
        (fraction | (1u64 << 52), exponent - 1075)
    };
    let scale = 10u128.pow(places);
    // The value times 10^places, rounded half to even.
    let scaled: Option<u128> = if exp >= 0 {
        u32::try_from(exp)
            .ok()
            .filter(|e| *e <= 64)
            .and_then(|e| u128::from(mantissa).checked_shl(e))
            .and_then(|v| v.checked_mul(scale))
    } else {
        let shift = exp.unsigned_abs();
        let numerator = u128::from(mantissa) * scale;
        if shift >= 128 {
            Some(0)
        } else {
            let q = numerator >> shift;
            let r = numerator & ((1u128 << shift) - 1);
            let half = 1u128 << (shift - 1);
            Some(if r > half || (r == half && q & 1 == 1) {
                q + 1
            } else {
                q
            })
        }
    };
    let Some(scaled) = scaled else {
        return format!("{value:.*}", places as usize);
    };
    let int = scaled / scale;
    let frac = scaled % scale;
    let sign = if negative { "-" } else { "" };
    if places == 0 {
        format!("{sign}{int}")
    } else {
        format!("{sign}{int}.{frac:0width$}", width = places as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_rounds_as_printf() {
        assert_eq!(fixed(0.007_812_5, 6), "0.007812");
        assert_eq!(fixed(0.023_437_5, 6), "0.023438");
        assert_eq!(fixed(1.000_000_5, 6), "1.000001");
        assert_eq!(fixed(2.0, 6), "2.000000");
        assert_eq!(fixed(-0.08, 6), "-0.080000");
        assert_eq!(fixed(1e-9, 6), "0.000000");
        assert_eq!(fixed(123_456_789.5, 0), "123456790");
    }

    #[test]
    fn sexagesimal_times() {
        assert_eq!(sexagesimal(2.0), "0:00:02.000000");
        assert_eq!(sexagesimal(3725.5), "1:02:05.500000");
        assert_eq!(sexagesimal(-0.339_022), "0:00:-0.339022");
    }

    #[test]
    fn units() {
        assert_eq!(with_unit(92_785, Unit::Byte), "90.610352 Kibyte");
        assert_eq!(with_unit(371_140, Unit::BitRate), "371.140000 Kbit/s");
        assert_eq!(with_unit(44_100, Unit::Hertz), "44.100000 KHz");
        assert_eq!(with_unit(48_000, Unit::Hertz), "48 KHz");
        assert_eq!(with_unit(500, Unit::Byte), "500 byte");
    }
}
