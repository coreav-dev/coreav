//! The Python module `coreav` (`docs/design/python.md`): the commands of the
//! `coreav` binary as functions, on file paths or on bytes in memory.
//!
//! Every function runs the same library code as the command
//! (`coreav-cli`'s library, with the library defaults and limits) and
//! returns the same versioned JSON document as a `dict`: `coreav.probe/1`
//! or `coreav.probe-matroska/1`, `coreav.verdict/1`, `coreav.clip/1`,
//! `coreav.extract-audio/1`, `coreav.remux/1` (`remux` and `sanitize`),
//! `coreav.extract-subtitles/1` and `coreav.chapters/1`. A command that
//! writes a file takes an `output` path, or returns the output as `bytes`
//! with its summary when `output` is `None`. Failures raise
//! `coreav.RejectedError` (the command's exit code 1),
//! `coreav.UsageError` (2) or `coreav.IoError` (3), all subclasses of
//! `coreav.CoreavError`; the first and last carry the `coreav.error/1`
//! object as `error`.
//!
//! The crate builds under the workspace's `forbid(unsafe_code)`: it has no
//! `unsafe`, and PyO3's macros need no exception (`docs/design/python.md`).
//! The interpreter is released while a command runs, so other Python
//! threads keep running.

use cli::{extract, matroska, mpegts, names, native, text};
use coreav_cli as cli;
use coreav_cli::error::Failure;
use coreav_cli::output::{self, WriteFailure};
use coreav_cli::remote::{self, RemoteSource};
use coreav_isobmff::{BufferedSource, SliceSource};
use coreav_isobmff_write::{
    AudioChoice, ClipOptions, MatroskaRemuxOptions, Metadata, OnUnsupported, RemuxOptions,
    TrackChoice, VideoChoice,
};
use coreav_policy::Policy;
use pyo3::create_exception;
use pyo3::exceptions::{PyException, PyTypeError};
use pyo3::prelude::*;
use pyo3::pybacked::PyBackedBytes;
use pyo3::types::{PyBool, PyBytes, PyDict, PyFloat, PyInt, PyList, PyString, PyTuple};
use serde_json::{Map, Number, Value};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

create_exception!(
    coreav,
    CoreavError,
    PyException,
    "Base class of the errors coreav raises."
);
create_exception!(
    coreav,
    RejectedError,
    CoreavError,
    "The file was rejected or cannot be processed as asked (the command's exit code 1). `error` holds the coreav.error/1 object."
);
create_exception!(
    coreav,
    UsageError,
    CoreavError,
    "The arguments are not valid (the command's exit code 2)."
);
create_exception!(
    coreav,
    IoError,
    CoreavError,
    "A file could not be read or written (the command's exit code 3). `error` holds the coreav.error/1 object."
);

/// Deepest nesting of a policy given as Python objects.
const MAX_POLICY_DEPTH: usize = 64;

/// Most bytes reserved up front for an output written to memory.
const MAX_RESERVED: u64 = 64 << 20;

/// Most keys [`KEYS`] holds.
const MAX_CACHED_KEYS: usize = 1024;

/// The `str` objects made for the keys of returned documents, reused from
/// call to call (`docs/design/python.md`, "Binding overhead"). A document's
/// keys are nearly all its schema's, a few hundred names, and a reused
/// `str` costs neither an allocation nor hashing (a `str` keeps its hash).
/// Keys taken from a file (tag names) can fill it; past
/// [`MAX_CACHED_KEYS`] a new key is made per use, as without the cache.
static KEYS: Mutex<Option<HashMap<String, Py<PyString>>>> = Mutex::new(None);

/// A source: bytes in memory (`bytes` or `bytearray`), a URL (a `str`
/// starting with a scheme and `://`, read by byte ranges;
/// `docs/design/remote.md`) or a file path (`str` or `os.PathLike`).
enum Input {
    Bytes(PyBackedBytes),
    Url(String),
    Path(PathBuf),
}

impl<'py> FromPyObject<'_, 'py> for Input {
    type Error = PyErr;

    fn extract(object: Borrowed<'_, 'py, PyAny>) -> Result<Self, Self::Error> {
        if let Ok(data) = object.extract::<PyBackedBytes>() {
            return Ok(Self::Bytes(data));
        }
        if let Ok(text) = object.cast::<PyString>() {
            let text = text.to_cow()?;
            if cli::remote_url(&text) {
                return Ok(Self::Url(text.into_owned()));
            }
        }
        object
            .extract::<PathBuf>()
            .map(Self::Path)
            .map_err(|_| PyTypeError::new_err("source must be a path, a URL, bytes or a bytearray"))
    }
}

impl Input {
    /// The path an output must differ from; bytes and URLs have none.
    fn path(&self) -> &Path {
        match self {
            Self::Bytes(_) | Self::Url(_) => Path::new(""),
            Self::Path(path) => path,
        }
    }
}

/// A URL input, opened inside the command (the interpreter is released):
/// public `https://` addresses only, unless `private_network` allows the
/// command line's rules (`docs/design/remote.md`, choice 5).
fn open_url(url: &str, private_network: bool) -> Result<RemoteSource, Failure> {
    let options = if private_network {
        remote::Options::command()
    } else {
        remote::Options::guarded()
    };
    remote::open(url, &options)
}

/// A write's failure with the remote reason, when a read of `source`
/// failed.
fn explain_write(source: &RemoteSource, failure: WriteFailure) -> WriteFailure {
    match source.failure() {
        Some(reason) if failure.is_io() => {
            WriteFailure::Io(Failure::Remote(reason.to_owned()).to_json())
        }
        _ => failure,
    }
}

/// Why a command failed, before it is turned into a Python exception (the
/// command runs with the interpreter released).
enum Fail {
    Usage(String),
    Rejected(Value),
    Io(Value),
}

impl From<Failure> for Fail {
    fn from(failure: Failure) -> Self {
        if failure.is_io() {
            Self::Io(failure.to_json())
        } else {
            Self::Rejected(failure.to_json())
        }
    }
}

impl From<WriteFailure> for Fail {
    fn from(failure: WriteFailure) -> Self {
        if failure.is_io() {
            Self::Io(failure.to_json())
        } else {
            Self::Rejected(failure.to_json())
        }
    }
}

/// What a writing command returns: the summary of a file it wrote, or the
/// output itself with its summary.
enum Written {
    File(Value),
    Memory(Vec<u8>, Value),
}

fn into_py_err(py: Python<'_>, fail: Fail) -> PyErr {
    let (error, object) = match fail {
        Fail::Usage(message) => return UsageError::new_err(message),
        Fail::Rejected(object) => (RejectedError::new_err(message(&object)), object),
        Fail::Io(object) => (IoError::new_err(message(&object)), object),
    };
    let value = error.value(py);
    let attributes = [
        ("error", Some(&object)),
        ("kind", object.get("kind")),
        ("limit", object.get("limit")),
        ("offset", object.get("offset")),
    ];
    for (name, field) in attributes {
        let field = field.unwrap_or(&Value::Null);
        if let Err(failed) = to_py(py, field).and_then(|v| value.setattr(name, v)) {
            return failed;
        }
    }
    error
}

fn message(object: &Value) -> String {
    object
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("coreav error")
        .to_owned()
}

/// A JSON value as Python objects: `dict`, `list`, `str`, `int`, `float`,
/// `bool` or `None`.
fn to_py(py: Python<'_>, value: &Value) -> PyResult<Py<PyAny>> {
    Ok(match value {
        Value::Null => py.None(),
        Value::Bool(b) => PyBool::new(py, *b).to_owned().into_any().unbind(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.into_pyobject(py)?.into_any().unbind()
            } else if let Some(u) = n.as_u64() {
                u.into_pyobject(py)?.into_any().unbind()
            } else {
                PyFloat::new(py, n.as_f64().unwrap_or(f64::NAN))
                    .into_any()
                    .unbind()
            }
        }
        Value::String(s) => PyString::new(py, s).into_any().unbind(),
        Value::Array(items) => {
            let list = PyList::empty(py);
            for item in items {
                list.append(to_py(py, item)?)?;
            }
            list.into_any().unbind()
        }
        Value::Object(map) => {
            let dict = PyDict::new(py);
            for (key, item) in map {
                dict.set_item(key_str(py, key), to_py(py, item)?)?;
            }
            dict.into_any().unbind()
        }
    })
}

/// The `str` for a document key, from [`KEYS`] when it is there. The lock
/// is never held while Python runs, so a call made from a finalizer
/// cannot wait for itself.
fn key_str<'py>(py: Python<'py>, key: &str) -> Bound<'py, PyString> {
    let cached = KEYS
        .lock()
        .ok()
        .and_then(|keys| keys.as_ref()?.get(key).map(|s| s.clone_ref(py)));
    if let Some(cached) = cached {
        return cached.into_bound(py);
    }
    let made = PyString::new(py, key);
    if let Ok(mut keys) = KEYS.lock() {
        let keys = keys.get_or_insert_with(HashMap::new);
        if keys.len() < MAX_CACHED_KEYS {
            keys.insert(key.to_owned(), made.clone().unbind());
        }
    }
    made
}

/// Python objects as a JSON value, for a policy given as a `dict`.
fn from_py(object: &Bound<'_, PyAny>, depth: usize) -> Result<Value, String> {
    if depth > MAX_POLICY_DEPTH {
        return Err(format!("nested more than {MAX_POLICY_DEPTH} levels"));
    }
    if object.is_none() {
        return Ok(Value::Null);
    }
    if let Ok(b) = object.cast::<PyBool>() {
        return Ok(Value::Bool(b.is_true()));
    }
    if object.is_instance_of::<PyInt>() {
        if let Ok(i) = object.extract::<i64>() {
            return Ok(Value::from(i));
        }
        return object
            .extract::<u64>()
            .map(Value::from)
            .map_err(|_| "an integer out of range".to_owned());
    }
    if let Ok(f) = object.cast::<PyFloat>() {
        return Number::from_f64(f.value())
            .map(Value::Number)
            .ok_or_else(|| "a number that is not finite".to_owned());
    }
    if let Ok(s) = object.cast::<PyString>() {
        return s
            .to_cow()
            .map(|s| Value::from(s.as_ref()))
            .map_err(|e| e.to_string());
    }
    if let Ok(dict) = object.cast::<PyDict>() {
        let mut map = Map::new();
        for (key, item) in dict.iter() {
            let key = key
                .cast::<PyString>()
                .map_err(|_| "a key that is not a string".to_owned())?
                .to_cow()
                .map_err(|e| e.to_string())?
                .into_owned();
            map.insert(key, from_py(&item, depth.saturating_add(1))?);
        }
        return Ok(Value::Object(map));
    }
    if object.is_instance_of::<PyList>() || object.is_instance_of::<PyTuple>() {
        let mut items = Vec::new();
        for item in object.try_iter().map_err(|e| e.to_string())? {
            let item = item.map_err(|e| e.to_string())?;
            items.push(from_py(&item, depth.saturating_add(1))?);
        }
        return Ok(Value::Array(items));
    }
    Err(format!(
        "a value of type {}",
        object
            .get_type()
            .name()
            .map_or_else(|_| "?".to_owned(), |n| n.to_string())
    ))
}

/// Runs `command` with the interpreter released and turns its result into
/// Python objects.
fn run<T: Send>(
    py: Python<'_>,
    command: impl FnOnce() -> Result<T, Fail> + Send,
    convert: impl FnOnce(Python<'_>, T) -> PyResult<Py<PyAny>>,
) -> PyResult<Py<PyAny>> {
    match py.detach(command) {
        Ok(result) => convert(py, result),
        Err(fail) => Err(into_py_err(py, fail)),
    }
}

fn document(py: Python<'_>, value: Value) -> PyResult<Py<PyAny>> {
    to_py(py, &value)
}

fn written(py: Python<'_>, result: Written) -> PyResult<Py<PyAny>> {
    match result {
        Written::File(summary) => to_py(py, &summary),
        Written::Memory(data, summary) => {
            let pair = (PyBytes::new(py, &data), to_py(py, &summary)?);
            Ok(pair.into_pyobject(py)?.into_any().unbind())
        }
    }
}

/// Writes through `write` to `output`, as the commands do (a temporary file
/// renamed into place; an existing file needs `force`; never the input), or
/// to memory when `output` is `None`.
fn write_output(
    input: &Input,
    output: Option<&Path>,
    force: bool,
    write: impl FnOnce(&mut dyn Write) -> Result<Value, WriteFailure>,
) -> Result<Written, Fail> {
    match output {
        None => {
            let mut data = Vec::new();
            // A refused reservation (an address-space limit) only means the
            // buffer grows as it is written.
            let _ = data.try_reserve(reserved(input));
            let summary = write(&mut data)?;
            Ok(Written::Memory(data, summary))
        }
        Some(path) => {
            if let Some(message) = output::refuse(input.path(), path, force) {
                return Err(Fail::Usage(
                    message.replace("pass --force", "pass force=True"),
                ));
            }
            Ok(Written::File(output::to_file(path, force, write)?))
        }
    }
}

/// The capacity an output written to memory starts with: the source's size
/// plus a margin, up to [`MAX_RESERVED`]. Outputs are rarely much larger
/// than their source, so the buffer does not grow by doubling, which in a
/// Python process (whose heap the interpreter fragments) moves and faults
/// the buffer in again at every step; pages beyond what is written are
/// never touched. Past [`MAX_RESERVED`] the buffer grows as usual.
fn reserved(input: &Input) -> usize {
    let size = match input {
        Input::Bytes(data) => u64::try_from(data.len()).unwrap_or(u64::MAX),
        Input::Path(path) => std::fs::metadata(path).map_or(0, |m| m.len()),
        // The size is known only once the first response has come.
        Input::Url(_) => 0,
    };
    let size = size.saturating_add(size / 8).saturating_add(64 << 10);
    usize::try_from(size.min(MAX_RESERVED)).unwrap_or(0)
}

fn unknown_codec(unknown: &str, matroska_too: bool) -> Fail {
    let mut known = names::ALLOW_NAMES.to_vec();
    if matroska_too {
        for name in matroska::allow_names() {
            if !known.contains(&name) {
                known.push(name);
            }
        }
    }
    Fail::Usage(format!(
        "unknown codec '{unknown}' in allow; known: {}",
        known.join(", ")
    ))
}

fn isobmff_allowlist(
    allow: Option<&[String]>,
    strict: bool,
) -> Result<coreav_isobmff::Allowlist, Fail> {
    cli::probe::allowlist(allow, strict).map_err(|unknown| unknown_codec(&unknown, false))
}

fn both_allowlists(
    allow: Option<&[String]>,
    strict: bool,
) -> Result<(coreav_isobmff::Allowlist, coreav_mkv::Allowlist), Fail> {
    let (isobmff, mkv, _) = all_allowlists(allow, strict)?;
    Ok((isobmff, mkv))
}

/// One allowlist per container `probe` reads.
fn all_allowlists(
    allow: Option<&[String]>,
    strict: bool,
) -> Result<
    (
        coreav_isobmff::Allowlist,
        coreav_mkv::Allowlist,
        coreav_mpegts::Allowlist,
    ),
    Fail,
> {
    cli::probe::allowlists(allow, strict).map_err(|unknown| unknown_codec(&unknown, true))
}

/// Whether the ISO base media reader did not recognise the file, so that
/// another container may read it (`coreav` main.rs).
fn not_isobmff(failure: &Failure) -> bool {
    matches!(failure, Failure::Library(coreav_isobmff::Error::NotIsobmff))
}

fn is_matroska(input: &Input) -> Result<bool, Fail> {
    match input {
        Input::Bytes(data) => Ok(coreav_mkv::sniff(data.get(..4).unwrap_or(data))),
        Input::Path(path) => Ok(matroska::detect(path)?),
        // URLs are opened, and detected, inside each command.
        Input::Url(_) => Ok(false),
    }
}

/// Describes an MP4, MOV, Matroska, WebM or MPEG transport stream file as
/// `coreav.probe/1`, `coreav.probe-matroska/1` or
/// `coreav.probe-mpegts/2`, as `coreav probe` does.
#[pyfunction]
#[pyo3(signature = (source, *, allow=None, strict=false, scan_fragments=false, scan_frame_sizes=false, private_network=false))]
fn probe(
    py: Python<'_>,
    source: Input,
    allow: Option<Vec<String>>,
    strict: bool,
    scan_fragments: bool,
    scan_frame_sizes: bool,
    private_network: bool,
) -> PyResult<Py<PyAny>> {
    run(
        py,
        || {
            let (isobmff, mkv, ts) = all_allowlists(allow.as_deref(), strict)?;
            // The container as the command tells it: Matroska, then ISO
            // base media, then transport streams (`coreav` main.rs).
            if let Input::Url(url) = &source {
                let mut remote = open_url(url, private_network)?;
                let size = remote.len_known();
                let mut buffered = BufferedSource::new(&mut remote);
                let probed = (|| {
                    if matroska::detect_source(&mut buffered)? {
                        return matroska::run_source_with(buffered, size, &mkv, scan_frame_sizes)
                            .map(|p| matroska::document(&p));
                    }
                    match cli::probe::run_source_with(
                        &mut buffered,
                        size,
                        &isobmff,
                        scan_fragments,
                        scan_frame_sizes,
                    ) {
                        Err(failure)
                            if not_isobmff(&failure) && mpegts::detect_source(&mut buffered)? =>
                        {
                            mpegts::run_source_with(&mut buffered, size, &ts, scan_frame_sizes)
                                .map(|p| mpegts::document(&p))
                        }
                        other => other.map(|p| native::document(&p)),
                    }
                })();
                return probed.map_err(|f| Fail::from(remote.explain(f)));
            }
            let matroska = is_matroska(&source)?;
            Ok(match (&source, matroska) {
                (Input::Url(_), _) => Value::Null,
                (Input::Path(path), true) => {
                    matroska::document(&matroska::run_with(path, &mkv, scan_frame_sizes)?)
                }
                (Input::Path(path), false) => {
                    match cli::probe::run_with(path, &isobmff, scan_fragments, scan_frame_sizes) {
                        Err(failure) if not_isobmff(&failure) && mpegts::detect(path)? => {
                            mpegts::document(&mpegts::run_with(path, &ts, scan_frame_sizes)?)
                        }
                        other => native::document(&other?),
                    }
                }
                (Input::Bytes(data), true) => matroska::document(&matroska::run_source_with(
                    SliceSource::new(data),
                    data.len() as u64,
                    &mkv,
                    scan_frame_sizes,
                )?),
                (Input::Bytes(data), false) => {
                    let size = data.len() as u64;
                    match cli::probe::run_source_with(
                        SliceSource::new(data),
                        size,
                        &isobmff,
                        scan_fragments,
                        scan_frame_sizes,
                    ) {
                        Err(failure)
                            if not_isobmff(&failure)
                                && mpegts::detect_source(&mut SliceSource::new(data))? =>
                        {
                            mpegts::document(&mpegts::run_source_with(
                                SliceSource::new(data),
                                size,
                                &ts,
                                scan_frame_sizes,
                            )?)
                        }
                        other => native::document(&other?),
                    }
                }
            })
        },
        document,
    )
}

/// Checks a file against an upload policy (`coreav.policy/1`, as a `dict`
/// or as JSON text) and returns the `coreav.verdict/1` verdict, as
/// `coreav check` does; a rejected file is a verdict, not an exception.
#[pyfunction]
#[pyo3(signature = (source, policy, *, private_network=false))]
fn check(
    py: Python<'_>,
    source: Input,
    policy: &Bound<'_, PyAny>,
    private_network: bool,
) -> PyResult<Py<PyAny>> {
    let invalid = |problem: String| UsageError::new_err(format!("invalid policy: {problem}"));
    let parsed = if let Ok(text) = policy.cast::<PyString>() {
        let text = text
            .to_cow()
            .map_err(|_| invalid("text that is not valid Unicode".to_owned()))?;
        Policy::from_json(text.as_bytes())
    } else if let Ok(data) = policy.extract::<PyBackedBytes>() {
        Policy::from_json(&data)
    } else {
        // The same size limit as for JSON text, on the policy as JSON.
        let value = from_py(policy, 0).map_err(invalid)?;
        let size = serde_json::to_vec(&value).map_or(usize::MAX, |v| v.len());
        if size > coreav_policy::MAX_POLICY_BYTES {
            return Err(invalid(format!(
                "larger than {} bytes as JSON",
                coreav_policy::MAX_POLICY_BYTES
            )));
        }
        Policy::from_value(&value)
    };
    let policy = parsed.map_err(|error| UsageError::new_err(format!("invalid policy: {error}")))?;
    run(
        py,
        || {
            let verdict = match &source {
                Input::Url(url) => {
                    let mut remote = open_url(url, private_network)?;
                    let size = remote.len_known();
                    cli::check::run_source(BufferedSource::new(&mut remote), size, &policy)
                        .map_err(|f| remote.explain(f))?
                }
                Input::Path(path) => cli::check::run(path, &policy)?,
                Input::Bytes(data) => {
                    cli::check::run_source(SliceSource::new(data), data.len() as u64, &policy)?
                }
            };
            Ok(verdict.to_json())
        },
        document,
    )
}

/// A time in seconds as the decimal text `coreav clip` takes: `str` as it
/// is, `int`, or `float` through its shortest representation.
fn time_text(value: &Bound<'_, PyAny>) -> PyResult<String> {
    if value.is_instance_of::<PyBool>() {
        return Err(UsageError::new_err("a time must be seconds, not a bool"));
    }
    if let Ok(text) = value.cast::<PyString>() {
        return Ok(text
            .to_cow()
            .map_err(|_| UsageError::new_err("a time must be valid text"))?
            .into_owned());
    }
    if value.is_instance_of::<PyInt>() {
        let seconds: u64 = value
            .extract()
            .map_err(|_| UsageError::new_err("a time must be at least 0"))?;
        return Ok(seconds.to_string());
    }
    // Floats, subclasses (NumPy's) included, rounded to nanoseconds, the
    // finest time `clip` takes: a computed time such as `frame / fps` has
    // more digits than its exact decimal value needs.
    if let Ok(seconds) = value.extract::<f64>() {
        let text = format!("{seconds:.9}");
        let text = text.trim_end_matches('0').trim_end_matches('.');
        return Ok(text.to_owned());
    }
    Err(UsageError::new_err(
        "a time must be decimal seconds: str, int or float",
    ))
}

fn parse_time(value: &Bound<'_, PyAny>, name: &str) -> PyResult<coreav_isobmff::Rational> {
    cli::clip::parse_time(&time_text(value)?).ok_or_else(|| {
        UsageError::new_err(format!(
            "{name} must be decimal seconds, such as 12 or 12.5"
        ))
    })
}

/// Cuts a keyframe-aligned clip of an MP4/MOV file to MP4 by stream copy,
/// as `coreav clip` does, and returns its `coreav.clip/1` summary (with the
/// clip's bytes when `output` is `None`). `audio` is a stream index,
/// `"first"` or `None` for no audio.
#[pyfunction]
#[pyo3(signature = (source, output=None, *, start, end=None, video=None, audio=Some(AudioArg::First), allow=None, strict=false, force=false, private_network=false))]
#[allow(clippy::too_many_arguments)]
fn clip(
    py: Python<'_>,
    source: Input,
    output: Option<PathBuf>,
    start: &Bound<'_, PyAny>,
    end: Option<&Bound<'_, PyAny>>,
    video: Option<Index>,
    audio: Option<AudioArg>,
    allow: Option<Vec<String>>,
    strict: bool,
    force: bool,
    private_network: bool,
) -> PyResult<Py<PyAny>> {
    let options = ClipOptions {
        video: video.map_or(VideoChoice::First, |v| VideoChoice::Stream(v.0)),
        audio: match audio {
            None => AudioChoice::None,
            Some(AudioArg::First) => AudioChoice::First,
            Some(AudioArg::Stream(index)) => AudioChoice::Stream(index),
        },
        start: parse_time(start, "start")?,
        end: end.map(|end| parse_time(end, "end")).transpose()?,
    };
    run(
        py,
        || {
            let allowlist = isobmff_allowlist(allow.as_deref(), strict)?;
            write_output(&source, output.as_deref(), force, |out| {
                let summary = match &source {
                    Input::Url(url) => {
                        let mut remote =
                            open_url(url, private_network).map_err(WriteFailure::Rejected)?;
                        cli::clip::run_source(
                            BufferedSource::new(&mut remote),
                            &options,
                            &allowlist,
                            out,
                        )
                        .map_err(|f| explain_write(&remote, f))?
                    }
                    Input::Path(path) => cli::clip::run(path, &options, &allowlist, out)?,
                    Input::Bytes(data) => cli::clip::run_slice(data, &options, &allowlist, out)?,
                };
                Ok(cli::clip::document(&summary))
            })
        },
        written,
    )
}

/// A stream index: a non-negative `int`, not a `bool`.
#[derive(Clone, Copy)]
struct Index(usize);

impl<'py> FromPyObject<'_, 'py> for Index {
    type Error = PyErr;

    fn extract(object: Borrowed<'_, 'py, PyAny>) -> Result<Self, Self::Error> {
        if object.is_instance_of::<PyBool>() || !object.is_instance_of::<PyInt>() {
            return Err(UsageError::new_err("a stream index must be an int"));
        }
        object
            .extract::<usize>()
            .map(Self)
            .map_err(|_| UsageError::new_err("a stream index must be at least 0"))
    }
}

/// The `audio` argument of [`clip`].
enum AudioArg {
    First,
    Stream(usize),
}

impl<'py> FromPyObject<'_, 'py> for AudioArg {
    type Error = PyErr;

    fn extract(object: Borrowed<'_, 'py, PyAny>) -> Result<Self, Self::Error> {
        if object.is_instance_of::<PyInt>() {
            return Ok(Self::Stream(object.extract::<Index>()?.0));
        }
        match object.extract::<String>().as_deref() {
            Ok("first") => Ok(Self::First),
            _ => Err(UsageError::new_err(
                "audio must be a stream index, \"first\" or None",
            )),
        }
    }
}

impl<'py> IntoPyObject<'py> for AudioArg {
    type Target = PyAny;
    type Output = Bound<'py, PyAny>;
    type Error = PyErr;

    fn into_pyobject(self, py: Python<'py>) -> Result<Self::Output, Self::Error> {
        match self {
            Self::First => Ok(PyString::new(py, "first").into_any()),
            Self::Stream(index) => Ok(index.into_pyobject(py)?.into_any()),
        }
    }
}

/// Copies the AAC audio track of an MP4/MOV file into an M4A file, as
/// `coreav extract-audio` does, and returns its `coreav.extract-audio/1`
/// summary (with the M4A bytes when `output` is `None`).
#[pyfunction]
#[pyo3(signature = (source, output=None, *, stream=None, allow=None, strict=false, force=false, private_network=false))]
#[allow(clippy::too_many_arguments)]
fn extract_audio(
    py: Python<'_>,
    source: Input,
    output: Option<PathBuf>,
    stream: Option<Index>,
    allow: Option<Vec<String>>,
    strict: bool,
    force: bool,
    private_network: bool,
) -> PyResult<Py<PyAny>> {
    let choice = stream.map_or(TrackChoice::FirstAudio, |s| TrackChoice::Stream(s.0));
    run(
        py,
        || {
            let allowlist = isobmff_allowlist(allow.as_deref(), strict)?;
            write_output(&source, output.as_deref(), force, |out| {
                let summary = match &source {
                    Input::Url(url) => {
                        let mut remote =
                            open_url(url, private_network).map_err(WriteFailure::Rejected)?;
                        extract::run_source(
                            BufferedSource::new(&mut remote),
                            choice,
                            &allowlist,
                            out,
                        )
                        .map_err(|f| explain_write(&remote, f))?
                    }
                    Input::Path(path) => extract::run(path, choice, &allowlist, out)?,
                    Input::Bytes(data) => extract::run_slice(data, choice, &allowlist, out)?,
                };
                Ok(extract::document(&summary))
            })
        },
        written,
    )
}

/// The options `remux` and `sanitize` share.
struct RemuxArgs {
    streams: Option<Vec<Index>>,
    fail_on_unsupported: bool,
    drop_unsupported: bool,
    allow: Option<Vec<String>>,
    strict: bool,
    force: bool,
    private_network: bool,
}

/// `coreav remux` with `metadata` copied or sanitized, as the binary's
/// `run_remux` does.
fn run_remux(
    py: Python<'_>,
    source: Input,
    output: Option<PathBuf>,
    args: RemuxArgs,
    metadata: Metadata,
) -> PyResult<Py<PyAny>> {
    if args.fail_on_unsupported && args.drop_unsupported {
        return Err(UsageError::new_err(
            "fail_on_unsupported and drop_unsupported cannot both be set",
        ));
    }
    let on_unsupported = if args.fail_on_unsupported {
        OnUnsupported::Fail
    } else {
        OnUnsupported::Drop
    };
    let streams: Option<Vec<usize>> = args.streams.map(|s| s.into_iter().map(|i| i.0).collect());
    let options = RemuxOptions {
        streams: streams.clone(),
        on_unsupported,
        metadata,
        left_out_covers: Vec::new(),
    };
    let mkv_options = MatroskaRemuxOptions {
        streams,
        on_unsupported,
        drop_video: args.drop_unsupported,
    };
    let (allow, strict, force, private_network) =
        (args.allow, args.strict, args.force, args.private_network);
    run(
        py,
        || {
            let (isobmff, mkv) = both_allowlists(allow.as_deref(), strict)?;
            let mut remote = match &source {
                Input::Url(url) => Some(open_url(url, private_network)?),
                _ => None,
            };
            let matroska = match remote.as_mut() {
                Some(r) => matroska::detect_source(r).map_err(|f| Fail::from(r.explain(f)))?,
                None => is_matroska(&source)?,
            };
            if matroska && matches!(metadata, Metadata::Sanitize(_)) {
                return Err(Failure::library(coreav_isobmff::Error::Unsupported {
                    reason: "sanitizing reads MP4/MOV files; Matroska and WebM are not supported",
                })
                .into());
            }
            write_output(&source, output.as_deref(), force, |out| {
                if let Some(r) = remote.as_mut() {
                    return if matroska {
                        cli::remux::run_matroska_source(
                            BufferedSource::new(&mut *r),
                            &mkv_options,
                            &mkv,
                            out,
                        )
                        .map(|s| cli::remux::matroska_document(&s))
                        .map_err(|f| explain_write(r, f))
                    } else {
                        cli::remux::run_source(
                            BufferedSource::new(&mut *r),
                            &options,
                            &isobmff,
                            out,
                        )
                        .map(|s| cli::remux::document(&s, &options))
                        .map_err(|f| explain_write(r, f))
                    };
                }
                Ok(match (&source, matroska) {
                    (Input::Url(_), _) => Value::Null,
                    (Input::Path(path), true) => cli::remux::matroska_document(
                        &cli::remux::run_matroska(path, &mkv_options, &mkv, out)?,
                    ),
                    (Input::Bytes(data), true) => cli::remux::matroska_document(
                        &cli::remux::run_matroska_slice(data, &mkv_options, &mkv, out)?,
                    ),
                    (Input::Path(path), false) => cli::remux::document(
                        &cli::remux::run(path, &options, &isobmff, out)?,
                        &options,
                    ),
                    (Input::Bytes(data), false) => cli::remux::document(
                        &cli::remux::run_slice(data, &options, &isobmff, out)?,
                        &options,
                    ),
                })
            })
        },
        written,
    )
}

/// Rewrites an MP4/MOV, Matroska or WebM file as MP4 with the movie box
/// first, by stream copy, as `coreav remux` does, and returns its
/// `coreav.remux/1` summary (with the MP4 bytes when `output` is `None`).
#[pyfunction]
#[pyo3(signature = (source, output=None, *, streams=None, fail_on_unsupported=false, drop_unsupported=false, allow=None, strict=false, force=false, private_network=false))]
#[allow(clippy::too_many_arguments)]
fn remux(
    py: Python<'_>,
    source: Input,
    output: Option<PathBuf>,
    streams: Option<Vec<Index>>,
    fail_on_unsupported: bool,
    drop_unsupported: bool,
    allow: Option<Vec<String>>,
    strict: bool,
    force: bool,
    private_network: bool,
) -> PyResult<Py<PyAny>> {
    let args = RemuxArgs {
        streams,
        fail_on_unsupported,
        drop_unsupported,
        allow,
        strict,
        force,
        private_network,
    };
    run_remux(py, source, output, args, Metadata::Copy)
}

/// Rewrites an MP4/MOV file as MP4 keeping only allowlisted boxes (no
/// location, device or date metadata), as `coreav sanitize` does, and
/// returns its `coreav.remux/1` summary (with the MP4 bytes when `output`
/// is `None`). `keep` lists what to keep, as `--keep` does: `rotation`,
/// `language`, `dates`, `gapless`, `kinds`; an empty list keeps none.
#[pyfunction]
#[pyo3(signature = (source, output=None, *, keep=vec!["rotation".to_owned(), "language".to_owned(), "gapless".to_owned(), "kinds".to_owned()], streams=None, fail_on_unsupported=false, allow=None, strict=false, force=false, private_network=false))]
#[allow(clippy::too_many_arguments)]
fn sanitize(
    py: Python<'_>,
    source: Input,
    output: Option<PathBuf>,
    keep: Vec<String>,
    streams: Option<Vec<Index>>,
    fail_on_unsupported: bool,
    allow: Option<Vec<String>>,
    strict: bool,
    force: bool,
    private_network: bool,
) -> PyResult<Py<PyAny>> {
    let keep = cli::remux::parse_keep(&keep).map_err(|unknown| {
        UsageError::new_err(format!(
            "cannot keep '{unknown}': keep takes {}, or none",
            cli::remux::KEEP_NAMES.join(", ")
        ))
    })?;
    let args = RemuxArgs {
        streams,
        fail_on_unsupported,
        drop_unsupported: false,
        allow,
        strict,
        force,
        private_network,
    };
    run_remux(py, source, output, args, Metadata::Sanitize(keep))
}

/// Writes the text subtitles (3GPP timed text or QuickTime text) of an
/// MP4/MOV file as WebVTT, as `coreav extract-subtitles` does, and returns
/// its `coreav.extract-subtitles/1` summary (with the WebVTT bytes when
/// `output` is `None`).
#[pyfunction]
#[pyo3(signature = (source, output=None, *, stream=None, force=false, private_network=false))]
fn extract_subtitles(
    py: Python<'_>,
    source: Input,
    output: Option<PathBuf>,
    stream: Option<Index>,
    force: bool,
    private_network: bool,
) -> PyResult<Py<PyAny>> {
    let choice = stream.map_or(coreav_text::TrackChoice::First, |s| {
        coreav_text::TrackChoice::Stream(s.0)
    });
    run(
        py,
        || {
            write_output(&source, output.as_deref(), force, |out| {
                let summary = match &source {
                    Input::Url(url) => {
                        let mut remote =
                            open_url(url, private_network).map_err(WriteFailure::Rejected)?;
                        text::run_subtitles_source(BufferedSource::new(&mut remote), choice, out)
                            .map_err(|f| explain_write(&remote, f))?
                    }
                    Input::Path(path) => text::run_subtitles(path, choice, out)?,
                    Input::Bytes(data) => text::run_subtitles_slice(data, choice, out)?,
                };
                Ok(text::subtitles_document(&summary))
            })
        },
        written,
    )
}

/// The QuickTime chapter list of an MP4/MOV file as `coreav.chapters/1`, as
/// `coreav chapters` prints it.
#[pyfunction]
#[pyo3(signature = (source, *, private_network=false))]
fn chapters(py: Python<'_>, source: Input, private_network: bool) -> PyResult<Py<PyAny>> {
    run(
        py,
        || {
            let list = match &source {
                Input::Url(url) => {
                    let mut remote = open_url(url, private_network)?;
                    text::run_chapters_source(BufferedSource::new(&mut remote))
                        .map_err(|f| remote.explain(f))?
                }
                Input::Path(path) => text::run_chapters(path)?,
                Input::Bytes(data) => text::run_chapters_slice(data)?,
            };
            Ok(text::chapters_document(list.as_ref()))
        },
        document,
    )
}

/// The `coreav` binary's own source, so that the Python package's `coreav`
/// command is the same program, with every subcommand, its options, output
/// and exit codes.
#[allow(dead_code)]
#[path = "../../coreav-cli/src/main.rs"]
mod command;

/// The `coreav` command of the Python package (`[project.scripts]`): runs
/// the binary's command line on `sys.argv` and ends the process with its
/// exit code, as the binary would, without returning to Python. Not part of
/// the Python API.
#[pyfunction]
fn _cli(py: Python<'_>) -> PyResult<()> {
    let sys = py.import("sys")?;
    let argv: Vec<std::ffi::OsString> = sys.getattr("argv")?.extract()?;
    // The command writes to the process's standard streams directly; a
    // closed stream is None in Python.
    for stream in ["stdout", "stderr"] {
        let stream = sys.getattr(stream)?;
        if !stream.is_none() {
            stream.call_method0("flush")?;
        }
    }
    // Python's SIGINT handler only sets a flag that the command never
    // checks: restore the default, so Ctrl-C stops it as it stops the binary.
    let signal = py.import("signal")?;
    signal.call_method1(
        "signal",
        (signal.getattr("SIGINT")?, signal.getattr("SIG_DFL")?),
    )?;
    // `coreav thumbnail` and `coreav transcode` (with their features) start
    // the command again as their sandboxed workers: here the installed
    // script, which is the first argument.
    let this = argv.first().map(std::path::PathBuf::from);
    let code = py.detach(|| command::main_with(argv, this));
    // `ExitCode` holds the command's 0 to 255; there is no stable way to
    // read it but comparing.
    let status = (0..=u8::MAX)
        .find(|n| std::process::ExitCode::from(*n) == code)
        .unwrap_or(1);
    let _ = std::io::Write::flush(&mut std::io::stdout());
    std::process::exit(i32::from(status))
}

/// The module.
#[pymodule(name = "coreav")]
fn coreav_py(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let py = m.py();
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add("CoreavError", py.get_type::<CoreavError>())?;
    m.add("RejectedError", py.get_type::<RejectedError>())?;
    m.add("UsageError", py.get_type::<UsageError>())?;
    m.add("IoError", py.get_type::<IoError>())?;
    m.add_function(wrap_pyfunction!(probe, m)?)?;
    m.add_function(wrap_pyfunction!(check, m)?)?;
    m.add_function(wrap_pyfunction!(clip, m)?)?;
    m.add_function(wrap_pyfunction!(extract_audio, m)?)?;
    m.add_function(wrap_pyfunction!(remux, m)?)?;
    m.add_function(wrap_pyfunction!(sanitize, m)?)?;
    m.add_function(wrap_pyfunction!(extract_subtitles, m)?)?;
    m.add_function(wrap_pyfunction!(chapters, m)?)?;
    m.add_function(wrap_pyfunction!(_cli, m)?)?;
    Ok(())
}
