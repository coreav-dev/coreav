//! The sections of `ffprobe`'s output and what decides which are printed.
//!
//! The tree is the one `ffprobe -sections` prints, limited to the
//! sections CoreAV fills (`docs/design/compat-cli.md`). Each section has a
//! name, which `-show_entries` may name, and a unique name, which only that
//! section has: `stream` names a stream of `streams` and a stream of a
//! program alike, `stream_tags` only the tags of the first.

/// A section of the output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sid {
    /// The whole output.
    Root,
    /// `packets`.
    Packets,
    /// A packet.
    Packet,
    /// A packet's tags.
    PacketTags,
    /// A packet's side data.
    PacketSideDataList,
    /// One side data element of a packet.
    PacketSideData,
    /// `programs`.
    Programs,
    /// A program.
    Program,
    /// A program's tags.
    ProgramTags,
    /// A program's streams.
    ProgramStreams,
    /// A stream of a program.
    ProgramStream,
    /// Its disposition.
    ProgramStreamDisposition,
    /// Its tags.
    ProgramStreamTags,
    /// `stream_groups`, always empty here.
    StreamGroups,
    /// `streams`.
    Streams,
    /// A stream.
    Stream,
    /// A stream's disposition.
    StreamDisposition,
    /// A stream's tags.
    StreamTags,
    /// A stream's side data.
    StreamSideDataList,
    /// One side data element of a stream.
    StreamSideData,
    /// `chapters`.
    Chapters,
    /// A chapter.
    Chapter,
    /// A chapter's tags.
    ChapterTags,
    /// `format`.
    Format,
    /// The format's tags.
    FormatTags,
    /// `error`, with `-show_error`.
    Error,
}

/// How many sections there are.
pub const SECTIONS: usize = 26;

/// What a section is.
#[derive(Clone, Copy, Debug)]
pub struct Def {
    /// The name `-show_entries` and the writers use.
    pub name: &'static str,
    /// The name only this section has.
    pub unique: &'static str,
    /// The name of one element, where the writers print one: `tag` for
    /// `tags`, `side_datum` for `side_data`.
    pub element: &'static str,
    /// The enclosing section.
    pub parent: Sid,
    /// An array of elements of one kind.
    pub array: bool,
    /// Its elements are told apart by a type (`side_data_type`), which
    /// the compact writer prints in place of a separate line.
    pub typed: bool,
}

const fn def(
    name: &'static str,
    unique: &'static str,
    element: &'static str,
    parent: Sid,
    array: bool,
    typed: bool,
) -> Def {
    Def {
        name,
        unique,
        element,
        parent,
        array,
        typed,
    }
}

impl Sid {
    /// Every section, in the order of [`Sid::index`].
    pub const ALL: [Sid; SECTIONS] = [
        Sid::Root,
        Sid::Packets,
        Sid::Packet,
        Sid::PacketTags,
        Sid::PacketSideDataList,
        Sid::PacketSideData,
        Sid::Programs,
        Sid::Program,
        Sid::ProgramTags,
        Sid::ProgramStreams,
        Sid::ProgramStream,
        Sid::ProgramStreamDisposition,
        Sid::ProgramStreamTags,
        Sid::StreamGroups,
        Sid::Streams,
        Sid::Stream,
        Sid::StreamDisposition,
        Sid::StreamTags,
        Sid::StreamSideDataList,
        Sid::StreamSideData,
        Sid::Chapters,
        Sid::Chapter,
        Sid::ChapterTags,
        Sid::Format,
        Sid::FormatTags,
        Sid::Error,
    ];

    /// The section's position in [`Sid::ALL`].
    pub fn index(self) -> usize {
        self as usize
    }

    /// What the section is.
    pub fn def(self) -> Def {
        use Sid::*;
        match self {
            Root => def("root", "root", "root", Root, false, false),
            Packets => def("packets", "packets", "packets", Root, true, false),
            Packet => def("packet", "packet", "packet", Packets, false, false),
            PacketTags => def("tags", "packet_tags", "tag", Packet, false, false),
            PacketSideDataList => def(
                "side_data_list",
                "packet_side_data_list",
                "side_data_list",
                Packet,
                true,
                false,
            ),
            PacketSideData => def(
                "side_data",
                "packet_side_data",
                "side_datum",
                PacketSideDataList,
                false,
                true,
            ),
            Programs => def("programs", "programs", "programs", Root, true, false),
            Program => def("program", "program", "program", Programs, false, false),
            ProgramTags => def("tags", "program_tags", "tag", Program, false, false),
            ProgramStreams => def(
                "streams",
                "program_streams",
                "streams",
                Program,
                true,
                false,
            ),
            ProgramStream => def(
                "stream",
                "program_stream",
                "stream",
                ProgramStreams,
                false,
                false,
            ),
            ProgramStreamDisposition => def(
                "disposition",
                "program_stream_disposition",
                "disposition",
                ProgramStream,
                false,
                false,
            ),
            ProgramStreamTags => def(
                "tags",
                "program_stream_tags",
                "tag",
                ProgramStream,
                false,
                false,
            ),
            StreamGroups => def(
                "stream_groups",
                "stream_groups",
                "stream_groups",
                Root,
                true,
                false,
            ),
            Streams => def("streams", "streams", "streams", Root, true, false),
            Stream => def("stream", "stream", "stream", Streams, false, false),
            StreamDisposition => def(
                "disposition",
                "stream_disposition",
                "disposition",
                Stream,
                false,
                false,
            ),
            StreamTags => def("tags", "stream_tags", "tag", Stream, false, false),
            StreamSideDataList => def(
                "side_data_list",
                "stream_side_data_list",
                "side_data_list",
                Stream,
                true,
                false,
            ),
            StreamSideData => def(
                "side_data",
                "stream_side_data",
                "side_datum",
                StreamSideDataList,
                false,
                true,
            ),
            Chapters => def("chapters", "chapters", "chapters", Root, true, false),
            Chapter => def("chapter", "chapter", "chapter", Chapters, false, false),
            ChapterTags => def("tags", "chapter_tags", "tag", Chapter, false, false),
            Format => def("format", "format", "format", Root, false, false),
            FormatTags => def("tags", "format_tags", "tag", Format, false, false),
            Error => def("error", "error", "error", Root, false, false),
        }
    }

    /// The sections directly inside this one.
    pub fn children(self) -> impl Iterator<Item = Sid> {
        Sid::ALL
            .into_iter()
            .filter(move |s| *s != Sid::Root && s.def().parent == self)
    }
}

/// A value, as `ffprobe` types it: the writers quote strings and not
/// integers, and print what is not known their own way.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// An integer, printed bare by `json` and `flat`.
    Int(i64),
    /// A string.
    Str(String),
    /// A value that is not known: `N/A` in the text writers, left out by
    /// `json`.
    Na,
    /// An optional value that is not known, printed as the given word
    /// (`unknown`) by the text writers and left out by `json`.
    Unset(&'static str),
    /// A time in seconds: `%f`, or `H:MM:SS.uuuuuu` under `-sexagesimal`.
    Time(f64),
    /// A count with a unit, a string in `json`; `-pretty` prints it with
    /// the unit and a prefix.
    Unit(i64, Unit),
}

/// The unit of a [`Value::Unit`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    /// Bytes: binary prefixes.
    Byte,
    /// Bits per second.
    BitRate,
    /// Hertz.
    Hertz,
}

/// A value that is a string.
pub fn s(text: impl Into<String>) -> Value {
    Value::Str(text.into())
}

/// A time value from a timestamp and its time base, as `ffprobe`
/// computes it: the timestamp times the time base as a double.
pub fn time(ts: i64, num: i64, den: i64) -> Value {
    if den == 0 {
        return Value::Na;
    }
    #[allow(clippy::cast_precision_loss)]
    let seconds = ts as f64 * (num as f64 / den as f64);
    Value::Time(seconds)
}

/// What `-show_entries` and the `-show_*` options select.
#[derive(Clone, Debug, Default)]
pub struct Selection {
    /// Every entry and every section inside, per section.
    all: [bool; SECTIONS],
    /// The named entries, per section; `Some` of an empty list when the
    /// section was named with `=` and no key.
    keys: [Option<Vec<String>>; SECTIONS],
    /// Printed: selected, or holding a selected section.
    shown: [bool; SECTIONS],
}

impl Selection {
    /// Selects `sid` and every section inside it whole (`-show_streams`,
    /// `-show_entries stream`), in place of what was selected before.
    pub fn select_all(&mut self, sid: Sid) {
        if let Some(all) = self.all.get_mut(sid.index()) {
            *all = true;
        }
        if let Some(keys) = self.keys.get_mut(sid.index()) {
            *keys = None;
        }
        for child in sid.children() {
            self.select_all(child);
        }
    }

    /// Adds the entries `keys` to what is selected of `sid` (`-show_entries
    /// stream=width`). A list replaces a whole-section selection made
    /// before it (`-show_streams -show_entries stream=index` prints the
    /// index alone) and joins an earlier list; the sections inside keep
    /// their own selection. An empty list prints no entry of it.
    pub fn select_keys(&mut self, sid: Sid, keys: &[String]) {
        let was_all = self
            .all
            .get_mut(sid.index())
            .map(|all| std::mem::replace(all, false))
            .unwrap_or(false);
        if let Some(slot) = self.keys.get_mut(sid.index()) {
            if was_all {
                *slot = None;
            }
            let list = slot.get_or_insert_with(Vec::new);
            for key in keys {
                if !list.contains(key) {
                    list.push(key.clone());
                }
            }
        }
    }

    /// Works out which sections are printed, once every selection is in.
    pub fn finish(&mut self) {
        let mut shown = [false; SECTIONS];
        for sid in Sid::ALL {
            if !self.selected(sid) {
                continue;
            }
            let mut at = sid;
            loop {
                if let Some(s) = shown.get_mut(at.index()) {
                    *s = true;
                }
                if at == Sid::Root {
                    break;
                }
                at = at.def().parent;
            }
        }
        self.shown = shown;
    }

    fn selected(&self, sid: Sid) -> bool {
        self.every_entry(sid) || !self.named(sid).is_empty()
    }

    /// Whether `sid` is printed.
    pub fn shown(&self, sid: Sid) -> bool {
        self.shown.get(sid.index()).copied().unwrap_or(false)
    }

    /// Whether the entry `key` of `sid` is printed.
    pub fn entry(&self, sid: Sid, key: &str) -> bool {
        self.every_entry(sid) || self.named(sid).iter().any(|k| k == key)
    }

    /// [`Selection::entry`] with the key compared without regard to ASCII
    /// case, as for tags.
    pub fn entry_ignoring_case(&self, sid: Sid, key: &str) -> bool {
        self.every_entry(sid) || self.named(sid).iter().any(|k| k.eq_ignore_ascii_case(key))
    }

    /// Whether every entry of `sid` is printed.
    pub fn every_entry(&self, sid: Sid) -> bool {
        self.all.get(sid.index()).copied().unwrap_or(false)
    }

    /// The keys named for `sid`, when it was named with a list.
    pub fn named(&self, sid: Sid) -> &[String] {
        self.keys
            .get(sid.index())
            .and_then(Option::as_deref)
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_in_order() {
        for (i, sid) in Sid::ALL.into_iter().enumerate() {
            assert_eq!(sid.index(), i);
        }
    }

    #[test]
    fn a_nested_selection_shows_its_parents() {
        let mut sel = Selection::default();
        sel.select_keys(Sid::StreamTags, &["language".to_owned()]);
        sel.finish();
        assert!(sel.shown(Sid::Streams) && sel.shown(Sid::Stream) && sel.shown(Sid::StreamTags));
        assert!(!sel.shown(Sid::Format));
        assert!(!sel.entry(Sid::Stream, "index"));
    }

    #[test]
    fn the_last_selection_of_a_section_wins() {
        let mut sel = Selection::default();
        sel.select_all(Sid::Stream);
        sel.select_keys(Sid::Stream, &["index".to_owned()]);
        sel.finish();
        assert!(sel.entry(Sid::Stream, "index") && !sel.entry(Sid::Stream, "width"));
        assert!(sel.every_entry(Sid::StreamTags));
        sel.select_all(Sid::Stream);
        sel.finish();
        assert!(sel.entry(Sid::Stream, "width"));
    }

    #[test]
    fn an_empty_list_shows_nothing() {
        let mut sel = Selection::default();
        sel.select_keys(Sid::Stream, &[]);
        sel.finish();
        assert!(!sel.shown(Sid::Stream));
    }
}
