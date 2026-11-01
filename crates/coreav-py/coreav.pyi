"""Type stub of the Python module `coreav` (docs/design/python.md).

Every function returns the command's versioned JSON document as a dict
(docs/schema/). Writing functions return the summary when `output` is a
path, and `(output bytes, summary)` when `output` is None.

A `source` is a path, the file's bytes, or a URL (a `str` such as
"https://…", read by byte ranges: docs/design/remote.md). By default a URL
must be https:// and resolve to public addresses only;
`private_network=True` also allows private addresses and http://.
"""

import os
from typing import Any, Dict, List, Literal, Optional, Tuple, Union

__version__: str

Source = Union[str, "os.PathLike[str]", bytes, bytearray]
Output = Optional[Union[str, "os.PathLike[str]"]]
Document = Dict[str, Any]
Time = Union[str, int, float]

class CoreavError(Exception):
    """Base class of the errors coreav raises."""

class RejectedError(CoreavError):
    """The file was rejected or cannot be processed as asked (exit code 1)."""

    error: Document
    kind: str
    limit: Optional[str]
    offset: Optional[int]

class UsageError(CoreavError):
    """The arguments are not valid (exit code 2)."""

class IoError(CoreavError):
    """A file could not be read or written (exit code 3)."""

    error: Document
    kind: str
    limit: Optional[str]
    offset: Optional[int]

def probe(
    source: Source,
    *,
    allow: Optional[List[str]] = None,
    strict: bool = False,
    scan_fragments: bool = False,
    scan_frame_sizes: bool = False,
    private_network: bool = False,
) -> Document:
    """coreav.probe/1, coreav.probe-matroska/1 or coreav.probe-mpegts/2, as `coreav probe`."""

def check(
    source: Source,
    policy: Union[Dict[str, Any], str, bytes],
    *,
    private_network: bool = False,
) -> Document:
    """coreav.verdict/1, as `coreav check --policy`."""

def clip(
    source: Source,
    output: Output = None,
    *,
    start: Time,
    end: Optional[Time] = None,
    video: Optional[int] = None,
    audio: Optional[Union[int, Literal["first"]]] = "first",
    allow: Optional[List[str]] = None,
    strict: bool = False,
    force: bool = False,
    private_network: bool = False,
) -> Union[Document, Tuple[bytes, Document]]:
    """A keyframe-aligned MP4 clip; coreav.clip/1, as `coreav clip`."""

def extract_audio(
    source: Source,
    output: Output = None,
    *,
    stream: Optional[int] = None,
    allow: Optional[List[str]] = None,
    strict: bool = False,
    force: bool = False,
    private_network: bool = False,
) -> Union[Document, Tuple[bytes, Document]]:
    """The AAC track as M4A; coreav.extract-audio/1, as `coreav extract-audio`."""

def remux(
    source: Source,
    output: Output = None,
    *,
    streams: Optional[List[int]] = None,
    fail_on_unsupported: bool = False,
    drop_unsupported: bool = False,
    allow: Optional[List[str]] = None,
    strict: bool = False,
    force: bool = False,
    private_network: bool = False,
) -> Union[Document, Tuple[bytes, Document]]:
    """MP4 with the movie box first; coreav.remux/1, as `coreav remux`."""

def sanitize(
    source: Source,
    output: Output = None,
    *,
    keep: List[str] = ["rotation", "language", "gapless", "kinds"],
    streams: Optional[List[int]] = None,
    fail_on_unsupported: bool = False,
    allow: Optional[List[str]] = None,
    strict: bool = False,
    force: bool = False,
    private_network: bool = False,
) -> Union[Document, Tuple[bytes, Document]]:
    """MP4 keeping only allowlisted boxes; coreav.remux/1, as `coreav sanitize`."""

def extract_subtitles(
    source: Source,
    output: Output = None,
    *,
    stream: Optional[int] = None,
    force: bool = False,
    private_network: bool = False,
) -> Union[Document, Tuple[bytes, Document]]:
    """WebVTT; coreav.extract-subtitles/1, as `coreav extract-subtitles`."""

def chapters(source: Source, *, private_network: bool = False) -> Document:
    """coreav.chapters/1, as `coreav chapters`."""
