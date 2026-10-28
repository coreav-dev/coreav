//! Pretty-printed JSON documents written a part at a time: the text that
//! `serde_json::to_string_pretty` gives for the whole document, without
//! building it whole. A document with one value per segment or input held
//! some 17 KB per entry, which made the memory of a split grow with its
//! number of segments (`docs/spikes/split-bench/`).

use serde_json::Value;
use std::io::{self, Write};

/// Writes `value` pretty-printed, every line but its first indented by
/// `indent` (`serde_json` escapes every newline inside a string).
pub(crate) fn write_indented<W: Write + ?Sized>(
    out: &mut W,
    value: &Value,
    indent: &str,
) -> io::Result<()> {
    let text = serde_json::to_string_pretty(value).map_err(io::Error::other)?;
    for (n, line) in text.split('\n').enumerate() {
        if n > 0 {
            out.write_all(b"\n")?;
            out.write_all(indent.as_bytes())?;
        }
        out.write_all(line.as_bytes())?;
    }
    Ok(())
}

/// Writes `items` as the array value of a key of the top-level object,
/// building one item at a time.
pub(crate) fn write_array<W: Write + ?Sized>(
    out: &mut W,
    items: impl Iterator<Item = Value>,
) -> io::Result<()> {
    let mut empty = true;
    for item in items {
        out.write_all(if empty { b"[\n    " } else { b",\n    " })?;
        write_indented(out, &item, "    ")?;
        empty = false;
    }
    out.write_all(if empty { b"[]" } else { b"\n  ]" })
}
