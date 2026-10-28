//! The policy document, schema `coreav.policy/1` (`docs/schema/policy-v1.md`).

use crate::exact::{Frac, parse_decimal};
use core::fmt;
use serde_json::{Map, Number, Value};

/// The schema identifier of a policy.
pub const SCHEMA: &str = "coreav.policy/1";

/// Largest policy document [`Policy::from_json`] reads, in bytes.
pub const MAX_POLICY_BYTES: usize = 1 << 20;

/// Codec names a policy may list: the names `coreav probe` prints in
/// `codec` for ISO base media and Matroska files, except `other` (streams
/// with that codec are dropped by the probe and governed by
/// [`StreamRules::unidentified`]).
pub const CODECS: [&str; 32] = [
    "h264",
    "hevc",
    "av1",
    "vp8",
    "vp9",
    "prores",
    "mpeg1video",
    "mpeg2video",
    "mpeg4",
    "theora",
    "mjpeg",
    "ffv1",
    "aac",
    "mp3",
    "mp2",
    "opus",
    "vorbis",
    "flac",
    "ac3",
    "eac3",
    "dts",
    "truehd",
    "alac",
    "pcm",
    "timed_text",
    "subrip",
    "ass",
    "webvtt",
    "pgs",
    "vobsub",
    "dvbsub",
    "timecode",
];

/// Longest `CodecID` a `streams.unidentified` list may name.
const MAX_CODEC_ID: usize = 64;

/// Stream kinds, in the order the verdict reports them: the names `coreav
/// probe` prints in `kind`.
pub const KINDS: [&str; 5] = ["video", "audio", "subtitle", "timecode", "data"];

/// Clockwise rotations a policy may allow, in degrees.
pub const ROTATIONS: [u64; 4] = [0, 90, 180, 270];

/// A container format: for ISO base media files, classified from the
/// `ftyp` major brand; for Matroska files, the EBML `DocType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// ISO base media (MP4, M4A, M4V and others): any major brand that is
    /// not QuickTime or 3GPP.
    Mp4,
    /// QuickTime: major brand `qt  `, or no `ftyp` box.
    Mov,
    /// 3GPP and 3GPP2: a major brand starting with `3g`.
    ThreeGp,
    /// WebM: `DocType` `webm`.
    Webm,
    /// Matroska: `DocType` `matroska`.
    Matroska,
}

impl Format {
    /// Every format, in the order of the policy reference.
    pub const ALL: [Self; 5] = [
        Self::Mp4,
        Self::Mov,
        Self::ThreeGp,
        Self::Webm,
        Self::Matroska,
    ];

    /// The policy's name for the format.
    pub fn name(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Mov => "mov",
            Self::ThreeGp => "3gp",
            Self::Webm => "webm",
            Self::Matroska => "matroska",
        }
    }

    /// The format of a file with this major brand (`None`: no `ftyp`).
    pub fn of_major_brand(brand: Option<&str>) -> Self {
        match brand {
            None | Some("qt  ") => Self::Mov,
            Some(brand) if brand.starts_with("3g") => Self::ThreeGp,
            Some(_) => Self::Mp4,
        }
    }

    /// The format of a Matroska file with this `DocType`, as `coreav probe`
    /// prints it; `None` for any other.
    pub fn of_doc_type(doc_type: &str) -> Option<Self> {
        match doc_type {
            "webm" => Some(Self::Webm),
            "matroska" => Some(Self::Matroska),
            _ => None,
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.name() == name)
    }
}

/// What a check does with a stream that has a problem the policy names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// The file is rejected.
    Reject,
    /// The stream is accepted as it is.
    Ignore,
}

/// Which verified picture size the resolution rules use next to the
/// declared one (`docs/design/frame-sizes.md`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SizeSource {
    /// The bound over the codec configuration and every parameter set and
    /// frame header in the samples (`video.scanned_size`); the check scans
    /// the samples.
    #[default]
    Scan,
    /// The parameter sets of the codec configuration only
    /// (`video.parameter_set_size`); no sample is read.
    Configuration,
    /// The size the sample entry declares, unverified.
    Declared,
}

/// What a check does with streams whose codec CoreAV does not identify.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unidentified {
    /// Every such stream rejects the file.
    Reject,
    /// Every such stream is accepted unchecked, whatever it carries.
    Ignore,
    /// Streams with one of these sample entry types (ISO base media, as
    /// `coreav probe` prints `fourcc`) or `CodecID`s (Matroska, as it
    /// prints `codec_id`) are accepted unchecked; any other rejects the
    /// file.
    Allow(Vec<String>),
}

/// A duration limit in seconds: the decimal the policy states, compared
/// exactly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seconds {
    pub(crate) exact: Frac,
    pub(crate) number: Number,
}

impl Seconds {
    /// A limit from a JSON number; `None` when it is negative or has more
    /// than 30 significant digits or decimal places.
    pub fn from_number(number: &Number) -> Option<Self> {
        let exact = parse_decimal(&number.to_string())?;
        Some(Self {
            exact,
            number: number.clone(),
        })
    }

    /// The limit as the policy wrote it.
    pub fn number(&self) -> &Number {
        &self.number
    }
}

/// Rules on the file as a whole.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileRules {
    /// Largest file size in bytes.
    pub max_size: Option<u64>,
    /// Largest overall bit rate in bits per second: file size × 8 divided by
    /// the duration.
    pub max_bit_rate: Option<u64>,
}

/// Rules on the container.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerRules {
    /// Allowed formats; `None` allows every format the probe reads.
    pub formats: Option<Vec<Format>>,
    /// Whether fragmented files are allowed.
    pub fragmented: bool,
}

impl Default for ContainerRules {
    fn default() -> Self {
        Self {
            formats: None,
            fragmented: true,
        }
    }
}

/// Rules on the duration.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DurationRules {
    /// Longest duration of the file and of each stream, in seconds.
    pub max_seconds: Option<Seconds>,
}

/// Rules shared by every stream kind.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KindRules {
    /// Allowed codecs (names of [`CODECS`]); `None` allows every codec.
    pub codecs: Option<Vec<&'static str>>,
    /// Fewest streams of this kind.
    pub min_count: Option<u64>,
    /// Most streams of this kind.
    pub max_count: Option<u64>,
    /// Largest bit rate of each stream, in bits per second.
    pub max_bit_rate: Option<u64>,
}

/// Rules on video streams.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VideoRules {
    /// The rules every kind has.
    pub common: KindRules,
    /// Largest coded width in pixels, as stored (before rotation).
    pub max_width: Option<u64>,
    /// Largest coded height in pixels, as stored (before rotation).
    pub max_height: Option<u64>,
    /// Largest width × height.
    pub max_pixels: Option<u64>,
    /// Allowed clockwise rotations in degrees; `None` allows every display
    /// matrix, including those that are not a quarter-turn rotation.
    pub rotations: Option<Vec<u64>>,
    /// Which verified size the resolution rules compare with the declared
    /// one.
    pub size_source: SizeSource,
}

impl VideoRules {
    /// Whether a check must scan the samples: a resolution rule that uses
    /// the scanned size.
    pub fn needs_scan(&self) -> bool {
        self.size_source == SizeSource::Scan
            && (self.max_width.is_some() || self.max_height.is_some() || self.max_pixels.is_some())
    }
}

/// Rules on audio streams.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AudioRules {
    /// The rules every kind has.
    pub common: KindRules,
    /// Most channels.
    pub max_channels: Option<u64>,
    /// Highest sample rate in Hz.
    pub max_sample_rate: Option<u64>,
}

/// Rules on streams. A kind without rules is not allowed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamRules {
    /// Most streams in the file, of every kind, dropped ones included.
    pub max_count: Option<u64>,
    /// Video streams.
    pub video: Option<VideoRules>,
    /// Audio streams.
    pub audio: Option<AudioRules>,
    /// Subtitle streams.
    pub subtitle: Option<KindRules>,
    /// Timecode streams.
    pub timecode: Option<KindRules>,
    /// Data streams.
    pub data: Option<KindRules>,
    /// Streams whose codec CoreAV does not identify (the probe's
    /// `dropped_streams`).
    pub unidentified: Unidentified,
    /// Streams whose sample tables are inconsistent.
    pub invalid_tables: Action,
    /// Streams whose data is in another file.
    pub external_data: Action,
    /// Streams with more than one sample description: the probe describes
    /// only the first, so the codec and size rules cannot see the others.
    pub multiple_descriptions: Action,
}

impl Default for StreamRules {
    fn default() -> Self {
        Self {
            max_count: None,
            video: None,
            audio: None,
            subtitle: None,
            timecode: None,
            data: None,
            unidentified: Unidentified::Reject,
            invalid_tables: Action::Reject,
            external_data: Action::Reject,
            multiple_descriptions: Action::Reject,
        }
    }
}

impl StreamRules {
    /// The shared rules of a kind (one of [`KINDS`]), when it is allowed.
    pub fn kind(&self, kind: &str) -> Option<&KindRules> {
        match kind {
            "video" => self.video.as_ref().map(|v| &v.common),
            "audio" => self.audio.as_ref().map(|a| &a.common),
            "subtitle" => self.subtitle.as_ref(),
            "timecode" => self.timecode.as_ref(),
            "data" => self.data.as_ref(),
            _ => None,
        }
    }

    /// The allowed kinds, in the order of [`KINDS`].
    pub fn allowed_kinds(&self) -> Vec<&'static str> {
        KINDS
            .into_iter()
            .filter(|k| self.kind(k).is_some())
            .collect()
    }
}

/// An upload policy: what a file must satisfy to be accepted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Policy {
    /// A name for logs, echoed in the verdict.
    pub name: Option<String>,
    /// Rules on the file.
    pub file: FileRules,
    /// Rules on the container.
    pub container: ContainerRules,
    /// Rules on the duration.
    pub duration: DurationRules,
    /// Rules on streams.
    pub streams: StreamRules,
}

/// Why a policy document was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyError {
    /// JSON pointer of the offending value (empty for the whole document).
    pub path: String,
    /// What is wrong.
    pub message: String,
}

impl fmt::Display for PolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            f.write_str(&self.message)
        } else {
            write!(f, "{}: {}", self.path, self.message)
        }
    }
}

impl std::error::Error for PolicyError {}

fn error(path: &str, message: impl Into<String>) -> PolicyError {
    PolicyError {
        path: path.to_owned(),
        message: message.into(),
    }
}

/// `path` extended by one key, escaped as RFC 6901 requires.
pub(crate) fn pointer(path: &str, key: &str) -> String {
    format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"))
}

impl Policy {
    /// Reads a policy from JSON text of at most [`MAX_POLICY_BYTES`].
    pub fn from_json(bytes: &[u8]) -> Result<Self, PolicyError> {
        if bytes.len() > MAX_POLICY_BYTES {
            return Err(error(
                "",
                format!("the policy is larger than {MAX_POLICY_BYTES} bytes"),
            ));
        }
        let value: Value =
            serde_json::from_slice(bytes).map_err(|e| error("", format!("not JSON: {e}")))?;
        Self::from_value(&value)
    }

    /// Reads a policy from a parsed JSON document. Unknown keys, values of
    /// the wrong type and unknown names are errors, so that a misspelled
    /// rule never passes silently.
    pub fn from_value(value: &Value) -> Result<Self, PolicyError> {
        let root = Object::new(value, "")?;
        root.only(&["schema", "name", "file", "container", "duration", "streams"])?;
        match root.get("schema") {
            Some(Value::String(schema)) if schema == SCHEMA => {}
            Some(_) => return Err(error("/schema", format!("must be \"{SCHEMA}\""))),
            None => return Err(error("/schema", format!("missing; must be \"{SCHEMA}\""))),
        }
        let name = match root.get("name") {
            None => None,
            Some(Value::String(name)) => Some(name.clone()),
            Some(_) => return Err(error("/name", "must be a string")),
        };
        let file = match root.child("file")? {
            None => FileRules::default(),
            Some(file) => {
                file.only(&["max_size", "max_bit_rate"])?;
                FileRules {
                    max_size: file.count("max_size")?,
                    max_bit_rate: file.count("max_bit_rate")?,
                }
            }
        };
        let container = match root.child("container")? {
            None => ContainerRules::default(),
            Some(container) => {
                container.only(&["formats", "fragmented"])?;
                ContainerRules {
                    formats: container.names("formats", |name| {
                        Format::from_name(name).ok_or_else(|| {
                            format!(
                                "unknown format \"{name}\"; known: {}",
                                Format::ALL.map(Format::name).join(", ")
                            )
                        })
                    })?,
                    fragmented: container.boolean("fragmented")?.unwrap_or(true),
                }
            }
        };
        let duration = match root.child("duration")? {
            None => DurationRules::default(),
            Some(duration) => {
                duration.only(&["max_seconds"])?;
                DurationRules {
                    max_seconds: duration.seconds("max_seconds")?,
                }
            }
        };
        let Some(streams) = root.child("streams")? else {
            return Err(error(
                "/streams",
                "missing; list the stream kinds the policy allows",
            ));
        };
        let streams = stream_rules(&streams)?;
        Ok(Self {
            name,
            file,
            container,
            duration,
            streams,
        })
    }
}

fn stream_rules(streams: &Object<'_>) -> Result<StreamRules, PolicyError> {
    streams.only(&[
        "max_count",
        "video",
        "audio",
        "subtitle",
        "timecode",
        "data",
        "unidentified",
        "invalid_tables",
        "external_data",
        "multiple_descriptions",
    ])?;
    const COMMON: [&str; 4] = ["codecs", "min_count", "max_count", "max_bit_rate"];
    let video = match streams.child("video")? {
        None => None,
        Some(video) => {
            video.only(
                &[
                    &COMMON[..],
                    &[
                        "max_width",
                        "max_height",
                        "max_pixels",
                        "rotations",
                        "size_source",
                    ],
                ]
                .concat(),
            )?;
            Some(VideoRules {
                common: kind_rules(&video)?,
                max_width: video.count("max_width")?,
                max_height: video.count("max_height")?,
                max_pixels: video.count("max_pixels")?,
                rotations: video.list("rotations", |item, path| {
                    item.as_u64()
                        .filter(|r| ROTATIONS.contains(r))
                        .ok_or_else(|| error(path, "must be 0, 90, 180 or 270"))
                })?,
                size_source: match video.get("size_source") {
                    None => SizeSource::Scan,
                    Some(Value::String(v)) if v == "scan" => SizeSource::Scan,
                    Some(Value::String(v)) if v == "configuration" => SizeSource::Configuration,
                    Some(Value::String(v)) if v == "declared" => SizeSource::Declared,
                    Some(_) => {
                        return Err(error(
                            &video.at("size_source"),
                            "must be \"scan\", \"configuration\" or \"declared\"",
                        ));
                    }
                },
            })
        }
    };
    let audio = match streams.child("audio")? {
        None => None,
        Some(audio) => {
            audio.only(&[&COMMON[..], &["max_channels", "max_sample_rate"]].concat())?;
            Some(AudioRules {
                common: kind_rules(&audio)?,
                max_channels: audio.count("max_channels")?,
                max_sample_rate: audio.count("max_sample_rate")?,
            })
        }
    };
    let plain = |key: &str| -> Result<Option<KindRules>, PolicyError> {
        match streams.child(key)? {
            None => Ok(None),
            Some(kind) => {
                kind.only(&COMMON)?;
                kind_rules(&kind).map(Some)
            }
        }
    };
    Ok(StreamRules {
        max_count: streams.count("max_count")?,
        video,
        audio,
        subtitle: plain("subtitle")?,
        timecode: plain("timecode")?,
        data: plain("data")?,
        unidentified: streams.unidentified("unidentified")?,
        invalid_tables: streams.action("invalid_tables")?,
        external_data: streams.action("external_data")?,
        multiple_descriptions: streams.action("multiple_descriptions")?,
    })
}

fn kind_rules(kind: &Object<'_>) -> Result<KindRules, PolicyError> {
    let rules = KindRules {
        codecs: kind.names("codecs", |name| {
            CODECS.into_iter().find(|c| *c == name).ok_or_else(|| {
                if name == "other" {
                    "\"other\" cannot be listed; streams with a codec CoreAV does not \
                         identify are governed by streams.unidentified"
                        .to_owned()
                } else {
                    format!("unknown codec \"{name}\"; known: {}", CODECS.join(", "))
                }
            })
        })?,
        min_count: kind.count("min_count")?,
        max_count: kind.count("max_count")?,
        max_bit_rate: kind.count("max_bit_rate")?,
    };
    if let (Some(min), Some(max)) = (rules.min_count, rules.max_count) {
        if min > max {
            return Err(error(
                &pointer(&kind.path, "min_count"),
                format!("{min} is above max_count {max}"),
            ));
        }
    }
    Ok(rules)
}

/// A non-negative integer, also when JSON writes it with a fraction or an
/// exponent (`1000.0`, `1e3`), as JSON Schema's `integer` allows; integral
/// values written that way are exact up to 2^53.
fn count(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        let float = value.as_f64()?;
        let exact = float.fract() == 0.0 && (0.0..=9_007_199_254_740_992.0).contains(&float);
        // The range check makes the conversion exact.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        exact.then_some(float as u64)
    })
}

/// A JSON object being read, with its pointer for error messages.
struct Object<'a> {
    map: &'a Map<String, Value>,
    path: String,
}

impl<'a> Object<'a> {
    fn new(value: &'a Value, path: &str) -> Result<Self, PolicyError> {
        match value {
            Value::Object(map) => Ok(Self {
                map,
                path: path.to_owned(),
            }),
            _ => Err(error(path, "must be an object")),
        }
    }

    fn get(&self, key: &str) -> Option<&'a Value> {
        self.map.get(key)
    }

    fn at(&self, key: &str) -> String {
        pointer(&self.path, key)
    }

    /// Refuses keys outside `allowed`; reports the first in key order.
    fn only(&self, allowed: &[&str]) -> Result<(), PolicyError> {
        match self.map.keys().find(|k| !allowed.contains(&k.as_str())) {
            None => Ok(()),
            Some(key) => Err(error(
                &self.at(key),
                format!("unknown key; expected one of {}", allowed.join(", ")),
            )),
        }
    }

    fn child(&self, key: &str) -> Result<Option<Object<'a>>, PolicyError> {
        self.get(key)
            .map(|value| Object::new(value, &self.at(key)))
            .transpose()
    }

    fn count(&self, key: &str) -> Result<Option<u64>, PolicyError> {
        self.get(key)
            .map(|value| {
                count(value).ok_or_else(|| error(&self.at(key), "must be a non-negative integer"))
            })
            .transpose()
    }

    fn boolean(&self, key: &str) -> Result<Option<bool>, PolicyError> {
        self.get(key)
            .map(|value| {
                value
                    .as_bool()
                    .ok_or_else(|| error(&self.at(key), "must be true or false"))
            })
            .transpose()
    }

    fn seconds(&self, key: &str) -> Result<Option<Seconds>, PolicyError> {
        self.get(key)
            .map(|value| match value {
                Value::Number(number) => Seconds::from_number(number).ok_or_else(|| {
                    error(
                        &self.at(key),
                        "must be a non-negative number of seconds with at most 30 digits",
                    )
                }),
                _ => Err(error(&self.at(key), "must be a number of seconds")),
            })
            .transpose()
    }

    fn action(&self, key: &str) -> Result<Action, PolicyError> {
        match self.get(key) {
            None => Ok(Action::Reject),
            Some(Value::String(s)) if s == "reject" => Ok(Action::Reject),
            Some(Value::String(s)) if s == "ignore" => Ok(Action::Ignore),
            Some(_) => Err(error(&self.at(key), "must be \"reject\" or \"ignore\"")),
        }
    }

    fn unidentified(&self, key: &str) -> Result<Unidentified, PolicyError> {
        match self.get(key) {
            None => Ok(Unidentified::Reject),
            Some(Value::String(s)) if s == "reject" => Ok(Unidentified::Reject),
            Some(Value::String(s)) if s == "ignore" => Ok(Unidentified::Ignore),
            Some(Value::Array(_)) => {
                let fourccs = self.list(key, |item, path| match item.as_str() {
                    Some(name)
                        if (1..=MAX_CODEC_ID).contains(&name.len())
                            && name.bytes().all(|b| (0x20..0x7f).contains(&b)) =>
                    {
                        Ok(name.to_owned())
                    }
                    _ => Err(error(
                        path,
                        "must be a sample entry type or a Matroska CodecID (1 to 64 \
                         printable ASCII characters)",
                    )),
                })?;
                Ok(Unidentified::Allow(fourccs.unwrap_or_default()))
            }
            Some(_) => Err(error(
                &self.at(key),
                "must be \"reject\", \"ignore\" or a list of sample entry types and CodecIDs",
            )),
        }
    }

    fn list<T>(
        &self,
        key: &str,
        item: impl Fn(&Value, &str) -> Result<T, PolicyError>,
    ) -> Result<Option<Vec<T>>, PolicyError> {
        let Some(value) = self.get(key) else {
            return Ok(None);
        };
        let path = self.at(key);
        let Value::Array(items) = value else {
            return Err(error(&path, "must be an array"));
        };
        items
            .iter()
            .enumerate()
            .map(|(i, value)| item(value, &format!("{path}/{i}")))
            .collect::<Result<Vec<T>, PolicyError>>()
            .map(Some)
    }

    fn names<T>(
        &self,
        key: &str,
        name: impl Fn(&str) -> Result<T, String>,
    ) -> Result<Option<Vec<T>>, PolicyError> {
        self.list(key, |value, path| match value {
            Value::String(s) => name(s).map_err(|message| error(path, message)),
            _ => Err(error(path, "must be a string")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(value: Value) -> Result<Policy, PolicyError> {
        Policy::from_value(&value)
    }

    fn err(value: Value) -> String {
        parse(value).unwrap_err().to_string()
    }

    #[test]
    fn minimal_policy_and_defaults() {
        let policy = parse(json!({"schema": SCHEMA, "streams": {}})).unwrap();
        assert_eq!(policy.name, None);
        assert_eq!(policy.file, FileRules::default());
        assert!(policy.container.fragmented);
        assert_eq!(policy.container.formats, None);
        assert_eq!(policy.streams.unidentified, Unidentified::Reject);
        assert_eq!(policy.streams.invalid_tables, Action::Reject);
        assert_eq!(policy.streams.external_data, Action::Reject);
        assert!(policy.streams.allowed_kinds().is_empty());
    }

    #[test]
    fn full_policy() {
        let policy = parse(json!({
            "schema": SCHEMA,
            "name": "web",
            "file": {"max_size": 1000, "max_bit_rate": 8},
            "container": {"formats": ["mp4", "3gp"], "fragmented": false},
            "duration": {"max_seconds": 600.5},
            "streams": {
                "max_count": 3,
                "video": {"codecs": ["h264"], "min_count": 1, "max_count": 1,
                          "max_width": 1920, "max_height": 1080, "max_pixels": 2073600,
                          "max_bit_rate": 5, "rotations": [0, 180]},
                "audio": {"codecs": ["aac"], "max_channels": 2, "max_sample_rate": 48000},
                "data": {},
                "unidentified": "ignore",
                "invalid_tables": "reject",
                "external_data": "ignore"
            }
        }))
        .unwrap();
        assert_eq!(policy.name.as_deref(), Some("web"));
        assert_eq!(policy.file.max_size, Some(1000));
        assert_eq!(
            policy.container.formats,
            Some(vec![Format::Mp4, Format::ThreeGp])
        );
        assert!(!policy.container.fragmented);
        let max = policy.duration.max_seconds.unwrap();
        assert_eq!(max.exact.reduced(), Some((1201, 2)));
        let video = policy.streams.video.as_ref().unwrap();
        assert_eq!(video.common.codecs, Some(vec!["h264"]));
        assert_eq!(video.rotations, Some(vec![0, 180]));
        assert_eq!(video.size_source, SizeSource::Scan);
        assert!(video.needs_scan());
        assert_eq!(policy.streams.unidentified, Unidentified::Ignore);
        assert_eq!(policy.streams.external_data, Action::Ignore);
        assert!(policy.streams.data.is_some());
        assert_eq!(
            policy.streams.allowed_kinds(),
            vec!["video", "audio", "data"]
        );
    }

    #[test]
    fn errors_name_the_offending_value() {
        assert_eq!(
            err(json!({"streams": {}})),
            "/schema: missing; must be \"coreav.policy/1\""
        );
        assert_eq!(
            err(json!({"schema": "coreav.policy/2", "streams": {}})),
            "/schema: must be \"coreav.policy/1\""
        );
        assert_eq!(
            err(json!({"schema": SCHEMA})),
            "/streams: missing; list the stream kinds the policy allows"
        );
        assert_eq!(
            err(json!({"schema": SCHEMA, "streams": {"video": {"max_widht": 1}}})),
            "/streams/video/max_widht: unknown key; expected one of codecs, min_count, \
             max_count, max_bit_rate, max_width, max_height, max_pixels, rotations, size_source"
        );
        assert_eq!(
            err(json!({"schema": SCHEMA, "streams": {"audio": {"codecs": ["aac", "wma"]}}})),
            "/streams/audio/codecs/1: unknown codec \"wma\"; known: h264, hevc, av1, vp8, vp9, \
             prores, mpeg1video, mpeg2video, mpeg4, theora, mjpeg, ffv1, aac, mp3, mp2, opus, \
             vorbis, flac, ac3, eac3, dts, truehd, alac, pcm, timed_text, subrip, ass, webvtt, \
             pgs, vobsub, dvbsub, timecode"
        );
        assert!(
            err(json!({"schema": SCHEMA, "streams": {"data": {"codecs": ["other"]}}}))
                .starts_with("/streams/data/codecs/0: \"other\" cannot be listed")
        );
        assert_eq!(
            err(json!({"schema": SCHEMA, "file": {"max_size": -1}, "streams": {}})),
            "/file/max_size: must be a non-negative integer"
        );
        assert_eq!(
            err(json!({"schema": SCHEMA, "file": {"max_size": 1.5}, "streams": {}})),
            "/file/max_size: must be a non-negative integer"
        );
        let integral = Policy::from_json(
            br#"{"schema": "coreav.policy/1", "file": {"max_size": 1e3, "max_bit_rate": 8.0},
                 "streams": {}}"#,
        )
        .unwrap();
        assert_eq!(integral.file.max_size, Some(1000));
        assert_eq!(integral.file.max_bit_rate, Some(8));
        assert_eq!(
            err(json!({"schema": SCHEMA, "duration": {"max_seconds": -2}, "streams": {}})),
            "/duration/max_seconds: must be a non-negative number of seconds with at most 30 \
             digits"
        );
        assert_eq!(
            err(json!({"schema": SCHEMA, "streams": {"video": {"rotations": [45]}}})),
            "/streams/video/rotations/0: must be 0, 90, 180 or 270"
        );
        assert_eq!(
            err(json!({"schema": SCHEMA, "streams": {"video": {"size_source": "sps"}}})),
            "/streams/video/size_source: must be \"scan\", \"configuration\" or \"declared\""
        );
        assert_eq!(
            err(json!({"schema": SCHEMA, "streams": {"video": {"min_count": 2, "max_count": 1}}})),
            "/streams/video/min_count: 2 is above max_count 1"
        );
        assert_eq!(
            err(json!({"schema": SCHEMA, "streams": {"unidentified": "drop"}})),
            "/streams/unidentified: must be \"reject\", \"ignore\" or a list of sample entry \
             types and CodecIDs"
        );
        assert_eq!(
            err(json!({"schema": SCHEMA, "container": {"formats": ["mkv"]}, "streams": {}})),
            "/container/formats/0: unknown format \"mkv\"; known: mp4, mov, 3gp, webm, matroska"
        );
        assert_eq!(
            err(json!({"schema": SCHEMA, "streams": {"a/b~c": 1}})),
            "/streams/a~1b~0c: unknown key; expected one of max_count, video, audio, subtitle, \
             timecode, data, unidentified, invalid_tables, external_data, multiple_descriptions"
        );
        assert_eq!(err(json!([1])), "must be an object");
    }

    #[test]
    fn unidentified_lists() {
        let policy =
            parse(json!({"schema": SCHEMA, "streams": {"unidentified": ["mebx", "c608"]}}))
                .unwrap();
        assert_eq!(
            policy.streams.unidentified,
            Unidentified::Allow(vec!["mebx".to_owned(), "c608".to_owned()])
        );
        assert_eq!(
            err(json!({"schema": SCHEMA, "streams": {"unidentified": ["mebx", ""]}})),
            "/streams/unidentified/1: must be a sample entry type or a Matroska CodecID (1 to \
             64 printable ASCII characters)"
        );
        let policy =
            parse(json!({"schema": SCHEMA, "streams": {"unidentified": ["V_QUICKTIME"]}})).unwrap();
        assert_eq!(
            policy.streams.unidentified,
            Unidentified::Allow(vec!["V_QUICKTIME".to_owned()])
        );
    }

    #[test]
    fn json_text() {
        assert!(Policy::from_json(br#"{"schema": "coreav.policy/1", "streams": {}}"#).is_ok());
        assert!(
            Policy::from_json(b"{")
                .unwrap_err()
                .message
                .starts_with("not JSON")
        );
        let big = vec![b' '; MAX_POLICY_BYTES + 1];
        assert!(
            Policy::from_json(&big)
                .unwrap_err()
                .message
                .contains("larger than")
        );
    }

    #[test]
    fn formats_from_brands() {
        assert_eq!(Format::of_major_brand(None), Format::Mov);
        assert_eq!(Format::of_major_brand(Some("qt  ")), Format::Mov);
        assert_eq!(Format::of_major_brand(Some("3gp5")), Format::ThreeGp);
        assert_eq!(Format::of_major_brand(Some("3g2a")), Format::ThreeGp);
        assert_eq!(Format::of_major_brand(Some("isom")), Format::Mp4);
        assert_eq!(Format::of_major_brand(Some("M4A ")), Format::Mp4);
    }
}
