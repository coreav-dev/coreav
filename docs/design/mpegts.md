# MPEG transport stream demux (`coreav-mpegts`)

Status: Accepted. Scope: MPEG-TS demux, a job of roadmap step 1. The input of HLS
ingest and of broadcast captures, and, by FFmpeg's vulnerability history, a demuxer worth moving out of C first.

## What it does

- `probe(source, limits, allowlist)` finds the packet size (188, 192 or
  204 bytes), reads the Program Association Table and every Program Map
  Table it names, and returns a `Stream` per elementary stream with its
  stream type, codec, descriptors and the codec parameters its first
  access units carry (picture size, sample rate, channels). It reads the
  head of the file until every stream of every program has shown a
  decodable start or a limit is reached, and the tail for the last
  timestamps, so durations need no full pass. A PES packet only the end
  of the file completes is read too: a video stream whose PES packets
  state no length and whose first one runs to the end of a short file
  carries the parameter sets, and leaving it in the reassembler lost
  them. A PES packet the budget cuts is not: its payload would describe
  the stream from bytes the file does not end at.
- `TransportStream::packets(source, limits)` lists every access unit of
  every kept stream in storage order (an E-AC-3 access unit once the next
  independent frame shows its dependent substreams are all there): one packet per PES packet for video,
  one per frame for audio whose frames can be delimited (ADTS and LOAS
  AAC, MPEG audio, AC-3, E-AC-3, Opus, DTS), with the byte position of the
  transport packet that starts it, timestamps unwrapped past the 33-bit
  wrap, and the keyframe flag.
- `coreav probe` prints the result as `coreav.probe-mpegts/1`
  (`docs/schema/probe-mpegts-v1.md`), and as the `ffprobe` shape with
  `--ffprobe-compat`. With `--scan-frame-sizes` it also lists every access
  unit and reports what the listing counted per stream (`scan`: packets,
  bytes, the first and last presentation times, their span and the bit rate
  over it). That is the listing's own count, not a second duration
  estimate: a stream's `duration` stays the probe's, which is the value
  `ffprobe` reports.

## Specifications

Clean room. Written from ISO/IEC 13818-1 (transport stream, PSI, PES,
stream types, descriptors), ITU-T H.222.0 (the same text), ETSI EN 300 468
(DVB service information descriptors: AC-3, E-AC-3, subtitling, teletext,
language), ATSC A/52 (AC-3 frame headers and the ATSC stream types), ISO/IEC
13818-7 and 14496-3 (ADTS), ISO/IEC 11172-3 and 13818-3 (MPEG audio frame
headers), the HLS specification (RFC 8216: transport stream segments, ID3
timed metadata, packed audio) and the BDAV MPEG-2 transport stream layout
of the Blu-ray format for 192-byte packets (the 4-byte arrival timestamp
prefix), ISO/IEC 14496-15 (the `avcC` and `hvcC` records the in-band
parameter sets make), the Opus transport multiplexing draft (ETSI TS
1xx xxx V0.1.3, the latest the Opus project publishes) for Opus access
units, and ETSI TS 102 114 (DTS core frames, the extension substream and
its asset descriptors). H.264 frames are walked for keyframes by `coreav-isobmff`'s
reader, and the configuration records are described by it; the fields a
record repeats from its SPS are read here. `ffprobe`
and `ffmpeg` are black-box oracles only; never FFmpeg's `mpegts.c` or any
other implementation.

## Design

### Layers

- `packet`: sans-IO transport packet header and adaptation field (PCR,
  discontinuity and random access indicators).
- `section`: PSI section reassembly per PID (pointer field, sections that
  span packets, several sections in one packet), CRC-32 check (eight
  bytes a step, from a table made at compile time), and the PAT
  and PMT with their descriptor loops.
- `pes`: PES header (stream id, PTS, DTS, header length) and the
  reassembly of a PES packet's payload from its transport packets. The
  two fixed bits `'10'` that open the optional header (ISO/IEC 13818-1
  2.4.3.6) are not checked: the MPEG-1 system syntax they tell apart is
  not used in transport streams, and `ffprobe` reads the header whatever
  they hold, so a PES packet with other bits there is listed as it lists
  it, its timestamps from the flags that follow. Rejected: dropping such a
  packet with a warning, as before, which left a stream with a packet in
  `ffprobe`'s listing and none in ours.
- `frames`: audio frame delimiting inside a PES payload (ADTS, LOAS,
  MPEG audio, AC-3 and E-AC-3 sync frames, Opus access units, DTS core
  frames and extension substreams).
- `probe`, `packets`: the I/O shell over a `Source`, the trait and
  `BufferedSource` of `coreav-isobmff`, as `coreav-mkv` uses them.

### Which reader the command uses

A transport stream has no header to recognise: a stream starts wherever
its sync byte begins to repeat at a packet stride, possibly after a prefix
of other bytes. So the `coreav` command offers a file to this reader last:
Matroska first (the EBML header `1A 45 DF A3`), then ISO base media (whose
first box names one of thirteen types), and a file neither recognises goes
to `probe` here, which finds the packet size and the first packet itself.
The test is the reader's own (`coreav_mpegts::detect`), which searches for
the packet size and the first stored packet within `max_resync_bytes`, so
the command accepts exactly the files the library reads. Rejected: a sniff
of the first bytes before the ISO base media check
(`coreav_mpegts::sniff`), which is cheaper but only matches a stream that
starts at offset 0, so a capture with a prefix (`ts_prefix_junk.ts`) would
be rejected where the library reads it. A candidate is confirmed by three
packets at the stride, fewer only for a file too short to hold three: a
lone sync byte near the end of a longer file is not a transport stream,
and taking it for one would make this reader answer for files no reader
recognises (a PNG, a text file, random bytes), which must keep the ISO
base media rejection. A file no reader recognises keeps
the ISO base media rejection, the command's answer for a container it does
not read. `scripts/oracle-library-adapter.sh` picks its adapter the same
way, so the CLI capture and the library capture are comparable.

### What the reader keeps

Every elementary stream the tables name is listed, including a stream
whose type and descriptors name no codec this crate knows: it is listed
with `Codec::Other`, its stream type and its format identifier, and its
access units are its PES packets as stored. Nothing of its payload is
parsed, so listing it costs nothing in safety, and a reader that left it
out would report a file with fewer streams than it has — in broadcast
captures those PIDs (DSM-CC sections, private sections, service
information) are common. `--allow` can neither name such a codec
(`Allowlist::new` drops `Other`) nor, therefore, ask for it to be dropped.

`Allowlist::default` is every codec the crate names. The other containers
keep a narrower default, but a transport stream carries no duration of its
own: the file's duration and its stream list are derived from its streams,
so leaving one out by default would change what the file is reported to be
(an AC-3 stream dropped from a broadcast capture shortens the file's
duration by the difference between the audio and video ends). `--allow`
narrows the list explicitly and `--strict` then rejects the file instead of
dropping the stream, as for the other containers; a dropped stream keeps
its place in `stream_index`, which is `ffprobe`'s numbering, so the streams
that are kept still pair with `ffprobe`'s. The stream type tells MPEG-1
from MPEG-2 video unreliably, so a stream of either type is read when
either is allowed and judged again on the codec its sequence header gives
it; a stream dropped before its header is read is named by its type.

### Packet size and synchronisation

The packet size is the one of 188, 192 and 204 whose sync byte (0x47)
repeats at that stride over the first packets (a fixed count, bounded by
the limits). Within the stream, a packet without its sync byte is a lost
packet: the reader resynchronises at the next offset where the sync byte
repeats at the stride, records a warning and reports the bytes skipped.
A packet with the transport error indicator set is read with a warning
(below, "Damaged input"). Resynchronisation searches a window that grows
from 16 KiB, in place, so a lost packet costs a few kilobytes of
reading, up to `max_resync_bytes`.

### Programs and streams

Every program of the PAT is read, and a PID that two programs share is
listed once. ISO/IEC 13818-1 2.4.4 makes a transport stream a set of
programs, not a list of streams, so the order is a reader's choice; the
streams are listed in `ffprobe`'s: those of the first Program Map Table
completed after the PAT in table order, then the others where the file
first shows them from its start, a table or a PES packet that is not
padding (`Stream::found_at`), the tables' order breaking ties. Both are in
the file's bytes, so `stream_index` is `ffprobe`'s index and `-map 0:N`
names the same stream on both sides for a file of several programs too.
The probe numbers the streams so before it builds the list, and the packet
listing, `coreav probe` and `coreav ffprobe` all follow it. Each stream
carries `pmt_index`, its place in its own table's loop, so that the
table's order is in the document too.

A PID that carries PES packets and that no table declares is listed too,
after the declared streams and with no stream type of its own: its packets
are the file's, and `ffprobe` lists them (not one whose PES packets are all
padding, `stream_id` 0xBE, which `ffprobe` does not list either), named by their payload ("Codecs
that the PMT leaves unnamed"). One broadcast capture of
the v1-ts-audio top-up has such a PID, and it is the only one of the forty
development files that does. A private stream (type 0x06) whose data
component descriptor (ARIB STD-B10 part 2 6.2.20) names a caption
component -- 0x0008 for a broadcast service, 0x0012 for the one-segment
service of STD-B24 that a Japanese capture carries beside it -- and whose
stream identifier's `component_tag` is a
caption one (table 6-5: 0x30 to 0x37, or 0x87 for one segment) is ARIB
closed captions, `arib_caption`, whose profile is STD-B24's A or C; the
superimpose tags (0x38 to 0x3F) are not, which is the reading `ffprobe`
has too. Reading 0x0008 alone left the one-segment caption stream of a
four-program capture as binary data.

The Event Information Table (ETSI EN 300 468 5.2.4, PID 0x12) is listed as
a data stream of codec `epg`, in the place where its first section is read.
It is a table, not an elementary stream, but it is the one piece of service
information `ffprobe` lists as a stream, and without it the two readers
disagree on how many streams a broadcast capture holds. `--allow` can name it like any other
codec this reader names, so a narrowed list drops it and `--strict`
rejects a file that carries one; a PID no table declares cannot be named,
as `Codec::Other` cannot. Only its
`table_id` is read, and a section is reassembled whole first (up to the
4,093 bytes EN 300 468 allows, where the tables of 13818-1 are bounded at
1,021) so that its CRC-32 vouches for that byte. Rejected: taking the
`table_id` from the first packet of a section, which a damaged byte could
turn into a stream the file does not have. The codec comes from the stream type, refined by the
registration descriptor (`HEVC`, `AC-3`, `EAC3`, `Opus`, `ID3 `) and the
DVB descriptors of private streams (type 0x06). Streams whose codec is not
allowed, and data streams (SCTE-35, ID3, teletext, DVB subtitles until
they have a reader), are listed as dropped. A PAT is complete once every
section of its version (`last_section_number`) has arrived. A PMT that
changes mid-stream (a version change) is read and the change reported as
a warning; streams it adds are listed by the probe when the tables of
the tail it reads name them, and by the packet listing from where they
appear (`Packets::streams`). Programs and streams a later version leaves
out stay listed: their packets before the change are the file's.

`ffprobe` prints a picture size for every video stream, 0 by 0 for one
whose parameters it never learnt (a scrambled stream, or the MVC video
sub-bitstream of a Blu-ray 3D capture, stream type 0x20, whose subset
sequence parameter sets give no size), so `--ffprobe-compat` does too; the
native document leaves the size out, since it is not known. A presentation
graphics subtitle (stream type 0x90) states the picture it is composed on
in its presentation composition segment (Blu-ray BD-ROM part 3-1
9.14.4.2.2), which both documents report as that stream's size, as
`ffprobe` does; it is the stream's own, not the video stream's.

A transport stream states no picture size of its own, so a video stream's
size is what its first sequence parameter set describes: the coded size
less the whole conformance crop (ITU-T H.264 7.4.2.1.1, H.265 7.4.3.2.1),
which is `parameter_set_display_size` of `coreav-isobmff`. Broadcast H.264
commonly codes 720p as 1920x1088 and crops it, and `parameter_set_size`,
the other size that crate gives, is a bound for framing samples that keeps
a crop larger than padding (it would report 1920x1088). A crop that leaves
nothing is ignored whole, as a decoder ignores it.

### Packets

- **Units.** One packet per access unit, which is what a decoder takes
  and what the MP4 sample model and the audio decoders of an earlier change take (the
  codec configuration in the MP4 and Matroska form: an `avcC` or `hvcC`
  record made from the in-band parameter sets, an AudioSpecificConfig
  made from the ADTS header or the LATM `StreamMuxConfig`).
  - H.264 and HEVC: the byte stream is split at the access unit
    boundaries of ITU-T H.264 7.4.1.2.3 and H.265 7.4.2.4.4 (an access
    unit delimiter, a parameter set or a prefix SEI message after a coded
    picture, or the first slice of the next picture). The split runs over
    the PES packet boundaries, not within each payload: ISO/IEC 13818-1
    2.4.3.7 says a PES packet starts at an access unit, but the real files
    of the v1-ts-audio top-up do not -- one starts each PES packet some tens
    of bytes before the delimiter of the unit it carries, so the last
    bytes of the unit before it are at the head of the next PES packet.
    Reading each payload on its own cut the units there, which is most of
    what the packet sizes of the H.264 files disagreed on. Rejected: one
    packet per PES packet, the rule until those files showed how often a
    PES packet holds both fields of an interlaced frame, that is two
    access units: a decoder would be handed two pictures at once and a
    remux to MP4 would write one sample for two. A unit held for want of
    its last bytes counts against the buffered-bytes limit.
  - Audio whose frames carry their length (ADTS and LOAS AAC, MPEG audio,
    AC-3 and E-AC-3, and Opus access units, whose control header is not
    part of the packet): one packet per frame. An E-AC-3 dependent
    substream (`strmtyp` 1) joins the access unit of the independent frame
    before it, up to eight (`substreamid` 0 to 7), whichever PES packet it
    is in. An AC-3 `bsid` of 9 or 10 halves or quarters the sample rate
    `fscod` names. A DTS extension substream (ETSI TS 102 114 7.5) joins
    the access unit of the core frame before it in the same way, once the
    stream has shown a core frame, whichever PES packet it is in: Blu-ray
    puts the core and the substream in PES packets of their own with one
    PTS, and `ffprobe` lists the two as one packet. In a stream without a
    core (DTS Express) each substream is an access unit, lasting the
    `nuExSSFrameDuration` its static fields state at the reference clock.
    Rejected: PES packets for audio too (a PES packet
    commonly holds several frames, and frames may straddle PES packets;
    an `ffmpeg`-written DTS stream of 4,096-byte PES packets holds four
    frames in each, which `ffprobe` lists one by one).
  - MPEG-1 and MPEG-2 video, data and subtitle streams, and streams whose
    codec this crate cannot name: one packet per PES packet, the unit the
    file delivers. (The listing as `ffprobe` gives it lists MPEG-1 and
    MPEG-2 video picture by picture; see that section.)
  - The Event Information Table, listed as the `epg` stream: one packet
    per section, its header and CRC included, with no position and no
    time, since a section carries none. `ffprobe` lists them the same
    way.
- **Times and positions.** A PTS times the first access unit that starts
  in its PES packet (ISO/IEC 13818-1 2.4.3.7): that unit takes the PTS and
  the position of the PES packet's first transport packet, wherever it
  ends; the units after it take no position and a time the file does not
  state, which this reader infers:
  - an audio frame starts the samples since the last stated time after
    it, rounded once, so that a run does not drift from the sample rate
    (adding a truncated frame duration each time loses up to a tick a
    frame, and a frame of 1,024 samples at 22,050 Hz lasts 4,179.59
    ticks). Where the PES packet states a decoding time of its own, which
    13818-1 2.7.5 asks a muxer not to write for sound, the frames it times
    keep the distance it states between the two times: the stream decodes
    that far ahead of its presentation throughout, not for one frame in
    four. One packaged stream of the v1-ts-audio top-up states a decoding
    time 600 ticks before each presentation time, and reporting the
    presentation time for both lost it;
  - the coded pictures since the last stated time -- a run -- divide
    equally the step from the time the run starts at to the next time the
    stream states, which closes it. So the two fields of an interlaced
    frame, which one PES packet carries, are half a frame apart, and so
    is the picture of a PES packet that states no time at all, which real
    captures write for every other picture. A run is held until the time
    after it shows how many units it has and what they divide; it is
    listed in storage order. The run no stated time closes -- the last of
    the file, or one after a time that goes backwards -- lasts as many
    picture periods as it has units, the period being the shortest a
    closed run of the stream has shown. Rejected: dividing that shortest
    period among the units of every run, the rule until a broadcast
    capture whose runs are four and five pictures long showed that it
    places the units of the longer ones a fifth of a picture apart.

  A unit after the first takes no position either; `ffprobe` gives one to
  a unit that starts past 200 KiB of its PES payload, because it reads a
  PES packet in pieces of that size (KD-TS-POSITION).
- **Release order.** A unit is listed once the bytes after it show where
  it ends, which is the order `ffprobe` lists units in across streams
  (`coreav ffprobe` prints that listing): an H.264 or HEVC unit when the
  next one starts, an MPEG-1 or MPEG-2 picture when the next PES packet
  of its stream is read, and an AC-3 or E-AC-3 frame when the next frame
  is read whole, so the last frame of a PES packet waits for the next
  one. (An H.264 or HEVC unit after the first one of a run is listed
  with its run, once the stated time that closes the run is read; see
  above.) Each unit carries what the PES packet whose reading released it
  says of itself: `corrupt` when a transport packet of it was lost,
  repeated or flagged in error, or it ended short of its stated length.
  Every unit also keeps the `stream_id` of the PES packet where it
  starts, including later units of one packet and those completed at EOF.
  Rejected: listing a PES packet's units as soon as it is complete,
  which listed the frames of AC-3 and the pictures of MPEG-2 video one
  PES packet earlier than `ffprobe` against the other streams; a stream's
  own units are in the same order either way.
- **Service names.** The Service Description Table of the transport stream
  (EN 300 468 5.2.3, PID 0x11, `table_id` 0x42) gives each program its
  service and provider names, kept as stored (`TransportStream::services`,
  and `Packets::services` for a listing that reads further), with the
  PAT's `transport_stream_id`.

  The DTS is the PTS when the header has none, which is what ISO/IEC
  13818-1 2.7.5 makes a header that states only a PTS say: a decoding
  time is encoded only when it differs from the presentation time. A
  picture whose PES packet states no decoding time of its own, or one
  equal to its presentation time, therefore decodes when it is shown;
  `ffprobe` replaces that with a time of its own, reconstructed from the
  order the pictures are coded in, which reading the slice headers of
  every picture would give (KD-TS-DECODE-TIME). A frame cut by the end of
  the file is listed with the bytes there are. `ffprobe` infers these
  times too, and within a tick of these (KD-TS-FRAME-TIME); the standard
  defines no arithmetic for them.
- **Audio parameters.** A stream's sample rate, channels and profile come
  from the first frame of a PES packet that states them, not from the
  first frame there is: a LOAS frame carries the stream's configuration
  only now and then (`useSameStreamMux` leaves it out of the others), so
  the first frame of a PES packet commonly states nothing. The search
  starts where a frame header reads, not at the start of the payload: a
  muxer that fills each PES packet to a fixed size begins nearly every
  one inside the frame the one before it started, and reading only the
  start of a payload left the one-segment AAC of a broadcast capture
  with no parameters at all. A sync word is a few bits -- eleven for
  LOAS -- so arbitrary bytes hold one every few kilobytes, and the
  search therefore prefers a place the payload confirms: the frame there
  fits in the payload and ends at another header or at the payload's
  end, and where both frames state a sample rate the two agree. That
  last clause settles a false sync whose declared length happens to
  land on the real frame: it names another rate than the stream's own
  frames name each other. Where no place in the payload is confirmed
  the search takes the first header that reads. It looks no further into
  the payload than the longest frame of the syntax, because the bytes
  before the first header are the tail of one frame and no more: 8,191
  bytes for ADTS and 8,194 for LOAS, which state their length in
  thirteen bits; 4,096 for AC-3 and E-AC-3 together, which is E-AC-3's
  `frmsiz` at its largest, AC-3's own longest frame being 3,840; 2,881
  for MPEG audio, which its bit rate table gives rather than a field;
  and for Opus, whose access unit runs to 2 MiB, a cap of 64 KiB. Rejected: searching 64 KiB
  whatever the syntax, which is what it did and which costs eight times
  the reading on the stream that never parses at all. Rejected: taking the first header that reads, which
  on a payload whose leading bytes carry a false sync gave the stream
  the rate that sync declares where the reader used to give it none; and
  requiring the confirmation, which leaves a stream whose frames are
  longer than its PES payloads -- no payload holds a whole frame, so
  none confirms anything -- with no parameters at all, while the packet
  listing lists its frames. The listing takes the first header that
  reads: a frame it cannot confirm is still a frame it has to list.
  The profile is
  the audio object type the configuration states; spectral band
  replication signalled only by extension payloads inside the raw data
  blocks is not read, because reading it means decoding
  (KD-TS-AAC-PROFILE).
- **The first packet of an audio stream.** The bytes before a stream's
  first frame header are the tail of a frame that began before the file.
  They are listed as a packet of their own, with the time and position of
  the PES packet they are in and the length of the frame that follows
  them, and the frames after them take their time from it; `ffprobe`
  lists them the same way. That holds when the end of the PES packet
  leaves at least eight bytes of the frame after them, enough for its
  header: the cut frame follows them by one frame, with no position (a
  capture of the corpus whose first AC-3 frame starts 290 bytes before
  the end of its PES packet, and AC-3 streams crafted with 100 such bytes
  and the first frame cut). With less than that, or with no frame header
  in the PES packet at all, the default listing drops those bytes and
  every later packet of the stream shifts by one against `ffprobe`'s. The
  listing as `ffprobe` gives it reads them by the stream's syntax, however
  the PES packets cut them (below). Rejected: dropping them, which
  shifted every later frame's time by one frame on the files that start
  mid-frame; and reading them only when the frame after them is whole, the
  rule until that capture, which dropped them and gave the cut frame the
  PES packet's time and position.
- **Keyframes.** Per access unit, from the coded data, by the rule
  `ffprobe` follows
  (`tools/oracle/SPEC.md`, "Keyframes"); a stream whose codec this
  reader cannot name has no keyframes at all, since nothing reads its
  bytes, while a private stream (type 0x06) read as binary data keeps the
  flag, as `ffprobe` reports both: for H.264 an IDR picture, a
  recovery point SEI message, or only I slices when the SPS keeps at most
  one reference frame; for HEVC an IRAP picture; for MPEG-1 and MPEG-2
  video an I picture; every audio frame. For H.264 only the NAL units up
  to the first coded slice, and that slice's first 32 bytes or up to the
  next unit when it starts sooner, are read unless the SPS keeps at most one reference frame: those units decide an
  IDR picture (`nal_unit_type` 5, ITU-T H.264 7.4.1) and a recovery point (its SEI message, Annex D), since an access unit's SEI NAL units come before its first coded slice (7.4.1.2.3), and the
  slices after them, nearly all of an access unit's bytes, matter only to
  the intra rule. Rejected: the
  `random_access_indicator` (optional, and set inconsistently by muxers);
  it is the fallback when the NAL units cannot be read.
- **Damaged input.** A packet with `transport_error_indicator` set is read
  with a warning (its payload may be damaged; the standard leaves the
  choice to the decoder, and `ffprobe` reads it). A duplicate packet (the
  same continuity counter as the one before it and the same bytes, its
  PCR aside) is discarded with a warning, as 13818-1 2.4.3.3 provides;
  `ffprobe` reads it as more payload (KD-TS-DUPLICATE). The same counter
  on other bytes is a lost packet: the packet is kept, with a warning. A continuity gap is a warning; the PES packet
  is listed with the bytes that arrived.
- **The end of the file.** An E-AC-3 dependent substream or a DTS
  extension substream the end of the file cuts is still part of the
  access unit before it, which is listed with what there is of it; a
  frame of its own is listed with the bytes there are. `ffprobe` lists a
  Blu-ray capture of the corpus cut in the extension substream of its
  last DTS-HD frame so. Rejected: listing the cut substream as a packet of
  its own, which no decoder can take apart from its core.

### The listing as `ffprobe` gives it (opt-in)

`Packets::listing_as_ffprobe` turns on a second listing, which
reproduces `ffprobe`'s where the default above follows the standard or
its own exact arithmetic. It is off by default; `coreav ffprobe`
(`crates/coreav-cli/src/ffprobe/ts.rs`) turns it on, and every other
caller -- `coreav probe --scan-frame-sizes`, the oracle adapter
(`tools/ts-adapter`), remuxing and transcoding -- gets the default
listing, which the known divergences (`docs/known-divergences-ts.md`)
describe. With it on:

- the PES packets of H.264, HEVC, MPEG-1 and MPEG-2 video are delivered
  in pieces of 204,800 payload bytes as they arrive (`Demux::pieces`),
  as `ffprobe` reads them: the first unit that starts in a later piece
  takes the PES packet's position (KD-TS-POSITION does not arise), and a
  unit finished by a piece is listed then, among the other streams' units
  that arrive while the rest of its PES packet does;
- an H.264 or HEVC unit is listed as soon as the start of the next one
  shows where it ends, with no runs: the units after the first one of a
  PES packet take no time and no position, as `ffprobe` reports them
  (placing a second field is `coreav ffprobe`'s, from the picture's own
  syntax). `Packet::stream_id` remains the identifier of the PES packet
  where the unit starts, as on the default listing;
- an AAC frame (ADTS or LOAS) a PES packet does not time falls on a grid
  of frames counted from time zero (the base taken to the nearest whole frame of
  that grid, each end of the span since it rounded to the nearest tick),
  which is where `ffprobe` puts it on the AAC streams of the v1-ts-audio
  corpus and on crafted streams at every alignment to that grid; an AC-3
  or E-AC-3 frame one truncated frame duration after the one before;
  other audio as by default (KD-TS-FRAME-TIME's one-tick differences do
  not arise for AAC). The grid is computed in `i64` and, only where a
  product would not fit, again in `i128`, with the same results;
- an AC-3 or E-AC-3 frame held for the next one is listed once the next
  frame's header is read, even when the end of its PES packet cuts that
  frame;
- at the end of the file, what each stream still holds is listed stream
  by stream rather than the video runs of every stream first;
- the last PES packet of a video stream listed whole (MPEG-4 Visual),
  which only the end of the file completes, carries no stream id, as
  `ffprobe` lists it;
- MPEG-1 and MPEG-2 video is cut into pictures across its PES packets, as
  H.264 is cut into access units, and listed as H.264 is: the first
  picture that starts in a PES packet takes its times and position, the
  others neither (the caller places them, `coreav ffprobe` one picture
  after the other). A picture begins at a sequence header, a group of
  pictures header or a picture header after the slices of the one before
  (ISO/IEC 13818-2 6.2), at the start code itself: a zero byte before it
  is stuffing of the picture before (5.3), which `ffprobe` counts there.
  The second field of a frame (6.1.1.2: fields come in pairs; a picture
  coding extension's `picture_structure` of 1 or 2 after a first field) is
  part of the frame's unit, and the time of a PES packet that starts with
  it goes to nobody. Evidence: a capture of the corpus with four pictures
  to a PES packet of 120 KB, another whose two fields are each in a PES
  packet of their own with a time, and streams crafted from `ffmpeg`'s
  with one to three pictures, and a fixed 5,000 bytes, to a PES packet,
  with and without every other PES packet's times. Rejected: one packet
  per PES packet, the rule until that capture, which listed 47 packets
  where `ffprobe` lists 177; and cutting MPEG video into pictures in the
  default listing too, which would scan every byte of it for start codes
  in the native probe (`coreav probe --scan-frame-sizes`, which the bench
  guard times) and change what remuxing takes, a change for the caller
  that needs pictures;
- the bytes before a stream's first frame header, or before the start of
  one that the end of a PES packet cuts, are gathered over as many PES
  packets as hold no header, and take the time and position of the PES
  packet they began in. Then, by the stream's syntax: AC-3 and ADTS list
  them as a packet of their own, one frame long (as the header after them
  says, else 1,536 samples for AC-3, A/52 5.3.1, and 1,024 for AAC,
  ISO/IEC 14496-3 4.5.2.1.1, at the stream's rate, and none where the
  stream states no rate), and the first frame follows them; E-AC-3 as a
  packet with no duration, its first frame starting at their time; MPEG
  audio and LOAS as the start of the first frame's packet, which takes
  their time and position; DTS not at all, its first frame taking their
  time and position. Where they began in an earlier PES packet, the time
  of the one the first header is in goes to the first frame that starts
  there. An AC-3 stream's are listed at once, and its first frame follows
  them from seven bytes of it on (the rule for a late frame below applies
  under seven); an ADTS stream's only once the header after them is
  whole: with fewer than seven bytes of it in the PES packet, the reading
  of the next one lists them, and the frame takes that PES packet's time
  and position, as the first frame that starts there does too. A file
  that ends before any header lists what it holds of them, and of a
  header after them, as one packet with no duration. Opus is read as by
  default. Evidence: two captures of the corpus (MP2 and MP3), and
  streams crafted with 100 such bytes before AC-3, E-AC-3, ADTS, LOAS, MP2,
  MP3 and DTS frames, with the first frame whole and cut at every byte
  count from one to sixteen, with the bytes alone in one and in two PES
  packets, and with nothing after them. Rejected: a packet of their own
  for every syntax, as the default listing lists them, which costs a DTS
  stream an extra packet and MPEG audio and LOAS their first frame's
  size; and listing them only where the PES packet holds the header
  after them (AC-3 and E-AC-3 apart), the rule until these streams, which
  dropped them, and the first frame after a lead packet cut at seven
  bytes, which took no time;
- an AC-3 or E-AC-3 frame of which its PES packet holds fewer than seven
  bytes (A/52 `syncinfo` and the `bsid` after it) takes the time and
  position of the next PES packet, and so does the first frame that
  starts there, unless it is the stream's first frame, which keeps the
  first PES packet's time and position however little of it that packet
  holds; with seven bytes the frame's start is known, and the frame held
  before it is listed then, though this reader needs eight to read the
  header (a capture of the corpus whose AC-3 frame starts six bytes
  before the end of its PES packet, and AC-3 and E-AC-3 streams crafted
  with every cut from one byte to sixteen, of the first frame and of
  later ones; ADTS, MP2 and MP3 streams crafted so show no such rule);
- the splice information sections of an SCTE 35 stream (ANSI/SCTE 35
  9.2, private sections of `table_id` 0xFC) are its packets. A stream of
  type 0x86 is one only where its program's descriptors register `CUEI`,
  which is how `ffprobe` reads it: a registration in the stream's own
  loop, `CUEI` or any other identifier, leaves the type unnamed, and such
  a stream is listed with no packets at all (crafted single-program
  streams of every layout: the identifier in the program loop, in the
  stream loop, in both, in neither, and another identifier in each
  place). Its sections, header and CRC included, take no position and
  the time of the last program clock reference the PCR PID of the
  stream's program carried before the section completed (90 kHz, the
  PCR's base); before the first PCR, none.
  A section whose CRC-32 fails is not listed. Evidence: a capture of the
  corpus, each of whose 183 sections `ffprobe` times by the PCR of the
  packet before it, another whose three sections all take the file's
  first PCR, and streams crafted with sections before the first PCR,
  between later ones, across two transport packets, with a spoilt CRC and
  with PCRs on a PID no program names (which time nothing). Rejected:
  reading the stream's PES packets, which it has none of, and listing it
  with no packets, the rule until those captures. `coreav ffprobe` gives
  the stream the times of its first and last timed sections as its start
  and end.

Rejected: making this the listing for every caller, which it was for a
while. Its times are `ffprobe`'s artefacts rather than the file's (a
picture with no time where the default places it exactly, a position
that depends on a reader's buffer size), and copying every piece out of
the reassembly buffer cost the native `coreav probe --scan-frame-sizes`
5.5% more instructions on the CI case `probe-packets-ts`. Rejected: a
field of `Limits`, which is shared with the probe and bounds resources;
this chooses a behaviour. The probe has one `ffprobe`-only setting of
its own, `Limits::tail_retries` (0 by default; `coreav ffprobe` sets 1
with a 250,000-byte tail): a stream the tail shows no end of is looked
for again in a tail twice as long. The probe's `services_first` and
`Program::pmt_read` are facts of the file, read at no cost, and are
always there.

A stream's `language` tag is the code its ISO 639 language descriptor
(or a teletext or subtitling descriptor) carries, `und` included: unlike
an MP4 or Matroska track, where `und` is the format's default for no
language and `ffprobe` omits it, a transport stream only has the tag when
the multiplexer wrote a descriptor, and `ffprobe` prints what it says
(17 development files that print `und`; none omits it).

An ADTS stream's `bit_rate` is the running mean of each frame's
`frame_length * 8 * sample_rate / 1024`, truncated at every frame, over
the whole PES packets of the analysis window. Two rules, each fitted on
crafted files (cuts at every byte of the header; 1 to 30 frames in the
first packet) and kept because four audio-only development files then
agree exactly and none gets worse:
- a frame whose 7-byte header ends in one PES packet and whose payload
  continues in the next is listed but not counted (`Packet::split`); a
  cut inside the header leaves the frame counted;
- with no video stream, a stream's window starts at its second PES
  packet. With video it starts at the first: the second-packet start
  breaks five audio-and-video files.
Not explained yet, so left: nine files whose video stream ends the
analysis at a point no time, byte or timestamp change moved.

### Codecs that the PMT leaves unnamed

A stream whose type and descriptors name no codec is identified from its
payload. `ffprobe` does this whatever the type: a DTS stream relabelled
with twenty-three stream types, from 0x00 and 0x05 through the user
private range, is listed as DTS for every type it does not map itself
(and as AC-3 for 0x81), so the rule applies to every unnamed type, not only to
private streams (0x06). The types `ffprobe` maps that this crate does not
are the exception (`codec::sniffed_type`): relabelling MPEG-2 video, MPEG
audio and AC-3 streams with each of the 256 types shows `ffprobe` naming
0x1C, 0x21, 0x42, 0x8A, 0xC1, 0xC2, 0xCF, 0xD1, 0xD2, 0xD4, 0xDB and 0xEA
from the type whatever the payload, and 0x13 never from the payload. Of
these, 0x8A (DTS) and the HLS sample encryption types 0xC1, 0xC2, 0xCF and
0xDB (AC-3, E-AC-3, ADTS AAC and H.264, headers in the clear; one
development file carries the last two) are mapped to their codecs; the
others stay unnamed (`unknown`), as before payloads were read. Rejected:
naming those from the payload, which reports a codec `ffprobe` does not;
mapping them, which needs parameter reading for codecs this crate does not
parse (AVS, Dirac, JPEG 2000, AAC without a transport syntax).

The PES packets of a stream named by its payload are joined, up to 16 KiB
or eight packets, until they show one of:

- at the start of the payload, an AC-3 or E-AC-3 sync frame, a DTS core
  frame or extension substream, an ADTS header, or an MPEG audio header in
  an audio stream (`stream_id` 0xC0 to 0xDF);
- in a video stream (`stream_id` 0xE0 to 0xEF), the start code of an
  MPEG video sequence header, group of pictures or picture (13818-2 table
  6-1), an H.264 access unit delimiter or sequence parameter set, or an
  H.265 access unit delimiter or video parameter set of layer 0;
- further into the payload, one of those audio headers followed by two
  more of the same syntax, kind and rate, each where the frame before it
  ends. A PES packet need not start at a frame (13818-1 2.4.3.7 asks it
  only of the unit its PTS times), and a sync word of a few bits turns up
  in other bytes: two headers in a row appear in the MPEG-H audio stream
  of a capture of the corpus (as MPEG audio layer I), which `ffprobe`
  leaves unnamed, and three do not.

A stream whose first unit is not a PES packet (sections) has nothing to
read and stays unnamed, so the probe does not wait for it. MPEG-1 and
MPEG-2 video are told apart by the sequence header: without a sequence
extension it is MPEG-1 (13818-2 6.1.1.6), whatever the stream type says.
Rejected: trusting the stream type alone (`ffmpeg` writes MPEG-1 video as
type 0x02, and Blu-ray style muxers leave AAC as a bare private stream);
reading only the first PES packet (a capture without tables starts its
AC-3 PES packets mid-frame); a single header further in (the MPEG-H stream
above).

The same rule names the PIDs no table declares. Once the tables are read,
such a PID is taken as a stream when one of its PES packets starts, after
the declared streams (those that started PES packets before the tables
first, in the order of their first ones), and named by its payload:
`ffprobe` lists AC-3 and DTS moved to a PID the PMT leaves out as `ac3`
and `dts` with tag 0. A PID first seen only in the tail is listed
unnamed, as before.

A file without a Program Association Table is read the same way: its
streams are the PIDs that carry PES packets, in the order of their first
ones, in no program, each named by its payload, and `ts_id` is 0 -- what
`ffprobe` lists for the two development captures without tables (MPEG-2
video with AC-3, and with MPEG audio) and for `ffmpeg`-written files with
their tables taken out. The probe reads the head once to find that there
is no table, and again with those PIDs as streams. Rejected: rejecting
such a file (`NoPrograms`), the rule before, which listed nothing of a
capture `ffprobe` reads whole. A PAT whose Program Map Tables never come
is still `NoPrograms`, as is a file with neither tables nor PES packets.

A frame header at the very start of a payload names the stream without a
second to confirm it, as a PES packet's first access unit is meant to start
there; bytes that only look like one (a payload of repeated `0B 77`) are
then named E-AC-3 where `ffprobe` lists them unknown. Rejected: asking for
the chain there too, which leaves a stream of one short PES packet
unnamed.

What `ffprobe` does with too little payload is not followed: a short DTS
stream of a type `ffprobe` does not map (0x82 without `HDMV`, as `ffmpeg`
writes it) is listed by `ffprobe` as `mp3` with no channels, which no
reading of the payload explains, and as `dts` once there are more frames.
How many depends on their size, not as a fixed byte count: crafted core
streams need 14 frames of 428 bytes, 8 of 1,024, 6 of 2,048 and 5 of
4,096; a corpus file cut short needs six 2,128-byte DTS-HD frames. This
reader names it DTS either way. An MPEG-H audio stream (type 0x2D) is listed unnamed, where
`ffprobe` lists one capture's as `mp3` with no channels and another's as
unknown.

### DTS

The codec is DTS for stream types 0x82, 0x85 and 0x86 and the Blu-ray
secondary audio type 0xA2 with the registration `HDMV`, for a DVB DTS
descriptor (EN 300 468, tag 0x7B), or from the payload. `coreav ffprobe`
reads the parameters from the stream's first access unit (ETSI TS 102 114
5.3.1 for the core, 7.5 for the extension substream's header and first
asset descriptor): the core's `SFREQ`, `AMODE` and `LFF` give the rate,
channels and layout (the surround pair of `AMODE` 9 is the side pair,
`5.1(side)`) and `RATE` the bit rate; an asset gives the rate
(`nuMaxSampleRate`), the channels (`nuTotalNumChs`) and the layout from
its speaker mask (Ls/Rs and Lss/Rss the side pair, Lsr/Rsr the back pair),
which is what the decoder `ffprobe` runs puts out. The profile is
`ffprobe`'s name for the components the unit carries, found by their sync
words in the substream: `DTS-HD MA` for a lossless one (decoded to `s16p`
or `s32p` of the asset's bits, which `bits_per_raw_sample` reports),
`DTS Express` for a low bit rate one, `DTS-HD HRA` for the other substream
components, `DTS-ES` and `DTS 96/24` for the core's channel and 96 kHz
extensions, `DTS` for a core alone. A stream with a substream reports no
bit rate: `ffprobe` gives none for the lossless streams of the corpus, and
the one it gives a DTS Express stream (187,872 bit/s on a capture whose
substreams are 2,048 bytes every 4,096 samples) is not the substream's
size over its duration, so it is not derived. A DTS stream ends at its
last PES packet's time, as an E-AC-3 stream does: on every DTS stream of
the corpus `ffprobe` adds none of the last unit.

### Timestamps

PTS and DTS are 33-bit values at 90 kHz, placed on one timeline for the
whole file from its first timestamp: if that is within 60 s of the wrap
(2^33), timestamps from 60 s before it on are taken as before the wrap
and come out negative; otherwise timestamps more than 60 s before it are
taken as after the wrap and come out plus 2^33. The reference is the
program's own first timestamp -- the earliest PES packet of any of its
PIDs -- because a program keeps a system time base of its own (13818-1
2.4.2.1) and the clocks of a broadcast capture's programs are commonly
hours apart: one reference for the whole file put the streams of five of
the six programs of one capture a whole wrap (2^33 ticks, 26 hours) from
where the file has them. One reference for every stream of a program
keeps those streams on one timeline. The 60 s window, and the
choice between the two cases, follow `ffprobe`, measured as a black box
(the fixtures `ts_wrap_near.ts` and `ts_wrap_far.ts`). Rejected:
unwrapping each stream against its own previous timestamp (two streams
of one program could land on different sides of the wrap). A PCR or
discontinuity indicator does not rewrite timestamps; a gap in a PID's
continuity counter is reported as a warning.

### Durations

A stream starts at the first timestamp of its PES packets, wherever they
are: those packets commonly come before the Program Map Table that names
the PID, and they are the stream's all the same. The reader therefore
records the first timestamp of every PID it sees, before any table names
it; `ffprobe` reports the same timestamp, reading the file again from the
start once it has the tables. Rejected: the first timestamp after the
tables, which reported a stream as starting up to a second late.

The probe reads the last `max_tail_bytes` (4 MiB) of the file: a stream
ends at its last PTS there plus the duration of the access unit it times.
That duration is the audio frame's, from its header; for a stream whose
frames the reader cannot read -- a video stream, Blu-ray LPCM, a LATM
stream whose configuration is out of band -- it is the smallest step
between the presentation times read, which is the picture interval or the
length of an audio unit. A subtitle, metadata or unnamed stream ends at
its last presentation time instead, because the step between its units is
not a duration: a subtitle or an identifier lasts what it says, not until
the next one. A stream's duration runs from its first timestamp to that
end; the file's, from the earliest start to the latest end of a program,
the longest program being the file's duration.

A stream the file times once shows no step, and its last access unit
still lasts: an H.264 stream whose sequence parameter set carries timing
information (ITU-T H.264 E.2.1) ends one tick of the clock it states
after that time, the smallest interval that clock can express, as the
rule above takes the smallest step the file shows. A camera's `.m2ts` of
one access unit is timed that way and no other; a stream whose parameter
sets state no clock keeps no duration at all. Rejected: two ticks, the
interval a frame picture lasts where `fixed_frame_rate_flag` is set,
which estimates what the file does not show rather than bounding it.

The tail only refines durations, so a tail the reader cannot use (sync
lost past the resynchronisation bound) leaves them unknown. A read the
source fails is not such a tail: it fails the probe with its I/O error.
Rejected: unknown durations on any tail error, which reported a
probe of a failing source as a clean result.

A file with more than one program has no one timeline, so a span across
its programs means nothing; `ffprobe` reports the longest program too.
A full pass would be
exact for streams that end before the tail, but costs a read of the whole
file in the probe; `ffprobe` estimates the same way, which keeps the two
comparable (KD-TS-END-ESTIMATE where its estimate leaves out the unit).

### Limits

`Limits` bounds the bytes read during the probe and at its tail, the
resynchronisation search, the number of programs and streams, the size
of a reassembled PES packet or frame (`max_packet_bytes`), the bytes held
at once across every PID (`max_buffered_bytes`: PES packets being
reassembled, audio frames carried to the next PES packet, and the access
units of a run the stream has shown no picture period for, which wait
for one only while what waits across every stream is under a quarter of
that bound and are listed with the time they have past it, so that
streams whose stated times never step forward are listed rather than
held to the end of the file), and the
transport packets of a listing. A PSI section is at most 1,024 bytes (the
standard's bound for the PAT and PMT; other tables are not read), which
bounds its descriptors. Frame lengths are bounded by their syntax (an
Opus access unit by 2 MiB). Parameter sets are kept up to 31 of a kind
and 64 KiB per stream. Every limit is an error, never a panic or an
unbounded allocation.

A PES packet being reassembled starts with room for its stated length
and one more transport packet or, with no stated length, for the longest
unit read on its PID so far, so that the buffer of a coded picture is not
copied each time it doubles. Either is capped at `max_buffered_bytes`
over `max_streams`, and at what `max_buffered_bytes` leaves of the room
the buffers in progress started with, so the room they reserve together
stays within `max_buffered_bytes` however many PIDs start a PES packet,
before the tables name the streams too. When the first transport packet holds the whole PES
header, the header is parsed there and only the payload is kept, so the
completed payload is handed on without moving its bytes; the header
still counts toward `max_packet_bytes` and `max_buffered_bytes`.

`max_probe_bytes` stops the head's read. It bounds every byte the head
reads: the packet reader is given the budget as the end of the file, so
a packet only partly inside it is not read, and a resynchronisation does
not search past it. Streams that showed no decodable
start by then are listed without parameters. A head that names no program
by then is refused with `LimitExceeded` and `LimitKind::ProbeBytes` when
the reader came to the budget's end before the file's (by a
resynchronisation too), and with `NoPrograms` only when the
probe read the whole file: a caller can then tell a file that names no
program from one whose tables come later than it allowed, and raise the
limit for the second. Rejected: removing the kind and keeping
`NoPrograms` for both, which told a caller that a valid file was invalid.

### Per-PID state

Every map the crate keys by PID is a `PidMap` (`src/pid_map.rs`), which is
built only from a limit of `Limits` or from the 8,192 PIDs a packet can
name, and which refuses a PID it does not hold yet once it holds that
many. It keeps its values in PID order beside a table over the PID space
(a `u16` a PID, 16 KiB, made on the first insertion) that says where each
PID's value is, so the lookups made for every transport packet are one
index, not a search; a `BTreeMap` there took 4 to 11% of the listing on
large captures. The crate's `clippy.toml` forbids `BTreeMap` and `HashMap` everywhere else in
the crate, with a reason that names `PidMap`, so a new per-PID fact
cannot be kept without choosing its bound. Each map keeps the bound and
the refusal it had before the type:

- `max_streams`: the streams listed (`Demux::by_pid`, a refusal being the
  `LimitKind::Streams` error), the first PES packet of each PID and its
  first time (`pes_pids`, `first_pes`: once full, no more PIDs are
  read), the clock of each PCR PID while splice sections are read
  (`pcrs`: a PID past the limit keeps none) and the wrap reference of
  each stream's PID (`TransportStream::program_reference`, whose PIDs are
  streams already listed).
- `max_programs`: the sections reassembled on PIDs that may carry a
  Program Map Table before the first PAT (`early_sections`: a PID past
  the limit is not read).
- The PID space: the continuity counter and the last packet of every PID
  with a payload (`counters`, `last`: 188 bytes each, 1.5 MiB at most,
  the cost of telling a duplicate or a lost packet on any PID), the
  section assemblers of the table PIDs and of the SCTE 35 streams kept
  (`sections`: a program may move its PMT to another PID any number of
  times), the PES packets being reassembled and the header of one
  delivered in pieces (`builders`, `continued`: their bytes are bounded
  by `max_buffered_bytes`), the PIDs a pass found that no table declares
  (`Demux::add_undeclared`, which then lists them under `max_streams`) and
  the listing's streams (`Packets::by_pid`: the probe's, whose limits may
  differ from the listing's, and those the listing adds). These cannot
  be full of PIDs a packet names, so they refuse nothing.

The rule came from coreav-dev/coreav#17, which added `pcrs` bounded only by the PID
space while the per-PID facts beside it were bounded by `max_streams`; a
review caught it and the bound was written by hand, as it was at each map
that had one. Rejected: those hand-written comparisons, which a new map
can leave out, as `pcrs` did; bounding every map by `max_streams`, which
would stop telling lost packets on a file with more PIDs than streams
and change which PIDs a listing reads; arrays of 8,192 entries, which
hold the whole PID space for a file of three PIDs (1.5 MiB for `last`
alone, again for each pass over the tail), where the table of places
holds two bytes a PID and the values only the PIDs read; forbidding
`BTreeSet` and `HashSet` too, since the PIDs read whatever the tables
say (`Demux::always`, the probe's streams) are a `PidMap<()>` and the
parameter sets of `video.rs` are not kept per PID. Clippy reads only the `clippy.toml` nearest a crate, so
the crate's repeats the workspace's settings, which
`tests/clippy_config.rs` checks; putting `disallowed-types` in the
workspace's file was rejected because it would forbid the maps of every
other crate.

### The oracle's environment

The pinned static `ffprobe` 7.0.2 crashes on transport streams on hosts
whose glibc differs from its build's (the Ubuntu 24.04 CI runners): it
decodes the SDT's service names with iconv, which loads the host's gconv
modules. `scripts/oracle-ffprobe.sh` sets `GCONV_PATH` to an empty path,
so iconv fails cleanly and the names are left as read; no compared field
depends on them.

### Oracle positions

`ffprobe` reports the position of a packet stored in 204 bytes 16 bytes
before its stored packet (where the previous packet's parity starts). The
library reports the stored packet's offset; the oracle adapter
(`tools/ts-adapter`) shifts it, as it is the adapter's job to speak the
oracle's vocabulary.

## Verification and bars (fixed before any run)

Inputs:

1. Fixtures: `scripts/gen-fixtures-ts.sh` makes transport
   streams with `ffmpeg` (H.264, HEVC, MPEG-2 video, AAC, MP2, AC-3,
   several programs, 192-byte packets, timestamp wrap, discontinuities,
   lost packets, PMT changes), plus crafted files for structures `ffmpeg`
   never writes: among them a PES packet that states a decoding time
   equal to its presentation time and one that states none at all, a PID
   no table declares beside two programs (and the same file with the
   second program's table held back, so that the two readers list that
   PID in different places), one video PES packet that only the end of
   the file completes, and sound whose PES packets state a decoding time
   of their own. The structures those files hold are also hand-built in
   `crates/coreav-mpegts/tests/captures.rs`, where the reader's answer is
   stated rather than compared.
2. Real files: the `real` transport streams of the private top-up
   v1-ts-audio (its README fixes the rules and the
   holdout before collection), development and holdout splits. That holdout is spent
   (below), so the bars that rest on one are judged on input 4 instead.
4. The replacement holdout: the transport stream draw of corpus
   v2-heldout, drawn for the operation `ts-probe`, whose rules
   its README fixes before collection. **At least 30 `real` transport
   streams**, every one held out, none of them a file or a structural
   signature v1-ts-audio or an earlier draw of `v2-heldout`
   already holds. Fewer than 30 is a shortfall reported with the
   result, not a bar judged on what there is: the bars below say which
   number they are read against, and that number is this one.
3. Robustness set: every transport stream of the top-up (`real`, `edge`,
   `hostile`) and the transport stream part of the hostile suite
   (`tools/hostile/generate.py`, part `mpegts`): tables that
   point at themselves or lie about their lengths, more programs and
   streams than the limits allow, PES packets that never end, frame headers
   whose lengths do not advance, damaged, duplicated and scrambled packets,
   and timestamps at the 33-bit wrap. `scripts/cli_hostile_check.py` runs
   each through `coreav probe` in both output modes and through
   `probe --scan-frame-sizes`, which lists every access unit.

Bars, against `ffprobe` 7.0.2 with the documented known divergences
(`docs/known-divergences-ts.md`, each with a predicate checked against
the file):

| Measure | Bar | Applies to |
|---|---|---|
| Stream oracle: files agreeing (direct or covered) over files `ffprobe` reads | at least 98% | input 2's development split, and input 4 (at least 30 files) |
| Packet oracle: packets agreeing over all paired and unpaired packets | at least 99% | the same |
| `--ffprobe-compat`: every key it prints equals `ffprobe`'s | 100% of files, keys | inputs 1, 2 and 4 |
| Crashes, panics, hangs (120 s per file) or non-structured failures | 0 | input 3 |
| Fixtures, streams, packets and `--ffprobe-compat` | 100% | CI, every change |
| Peak resident memory of `coreav probe` | at most 300 MiB | input 3 |

The known divergences (`docs/known-divergences-ts.md`) are built as the
ISO base media overlay's are since an earlier change: each predicate checks CoreAV's
values against the file itself, read by a reader written apart from
CoreAV's (`Facts` in `scripts/oracle_kd_ts.py`), so that a CoreAV bug that
moves a value cannot also move the evidence the entry checks; and
`tools/oracle/kd_audit.py --overlay ts` injects bug-shaped changes into
CoreAV's captures of every fixture (in CI, `ts-fixtures`), where none may
be covered. The bars above came with this design note in an earlier change
(`cdcd6e4`), before any real file was compared; every entry is dated
after them, the two that arrived in the same pull request included:
`ADDED_AFTER_THE_BARS` in `tools/stage3-report/report.py`, and every
reported number is given with every entry and with direct agreement
only.

If input 2 has fewer than 50 `real` files (the stage 3 criterion), the
bars are judged on the files there are and the shortfall is reported with
them; the count is not reached by relaxing the permission rule.

Where the bars stand is in [`measurements/ts-demux.md`](../measurements/ts-demux.md).
They were met on the development split and missed on the held-out split,
which was compared once and is spent; the causes that split showed are
fixed or documented here. The development split's direct agreement did
not move with them -- the files it holds show none of those causes --
and the packets it leaves uncovered fell from 919 to 2, every one of
them to the new divergence entry, not to a fix. **No bar is judged again
until a holdout the reader has not seen is drawn**: a later run of the
spent one, after changes made in answer to what it showed, is no
estimate of unseen files.

Fuzzing: a fuzz project `crates/coreav-mpegts/fuzz` with targets for the
probe, the packet listing, the PSI sections and the PES reassembly, run
in CI as every fuzz project is, and part of the 24 CPU-hour campaign.

### Speed bars

On the `real` transport streams of input 4, the replacement holdout,
the median per-file ratio, overall and for each class (`group-g`, `group-b`
and `group-h`, whichever has at least 5 files; smaller classes are
reported), with a bootstrap 95% interval:

| Operation | Against | Bar |
|---|---|---|
| `coreav probe` | `ffprobe -v error -of json -show_format -show_streams` | at most 1.00 |
| `coreav probe --ffprobe-compat` | the same | at most 1.00 |
| the packet listing, in process | libavformat in process (`docs/spikes/parity-bench/`): open, find stream info, read every packet | at most 1.00 |

Both sides run on the same host, warm, with five rounds in alternating
order. `scripts/bench_guard.py` gains three transport stream cases as the
CI regression guard: `probe-ts`, `probe-compat-ts` and `probe-packets-ts`
(`probe --scan-frame-sizes`, the packet listing over the whole file). A
claim that CoreAV is faster needs the upper bound of the interval below
1.00. Besides the median and its interval, the speed report gives the
share of held-out files on which CoreAV is faster, overall and per class.

Input 4 was collected and opened once; `docs/measurements/ts-demux.md`
holds the result. The three speed bars are met on it, overall and in
every class, and all three overall intervals are below 1.00. The three
correctness bars are missed: the oracle bars at 81.08%
of files and 73.88% of packets, which is 100% and 100.00% on `group-g` and
58.3% and 49.77% on `group-h`, and `--ffprobe-compat`, 21 of whose 26
keys differ on between 1 and 7 of the 37 files (it rejects nothing
unexplained). The reader is at parity on streaming segments
and does not yet read the large multi-programme multiplex; the largest
single item is `KD-TS-STREAM-ORDER`, whose predicate was settled
against one capture and does not fire on two multiplexes whose stream
list is an exact permutation of `ffprobe`'s.

### Speed bars, round 2: large files against FFmpeg 9.0.2

The speed bars above are met overall and in every class of input 4, but four of its files, three off-air captures of 84 to 135 MB and a 196 MB HLS segment, take 1.06 to 1.43 of libavformat 7.0.2's time on the packet listing, and the `group-h` interval reaches 1.019. Against FFmpeg 9.0.2 the gap is wider: on the same four files, timed on a development machine (not a bar), the packet listing takes 1.35 to 1.75 of libavformat 9.0.2's time. This round closes that gap and is judged on files the reader has not seen. Input 4 is spent and is a tuning pool for this round.

**What limits it** (instruction counts with Valgrind on four of input 4's large files, `main` at the commit before this section; a profile, not a bar). The listing spends its instructions in four places:

- H.264 key frames: each access unit is copied into a length-prefixed buffer and every NAL unit of it walked, to read slice types that only a stream keeping at most one reference frame needs. On the 196 MB segment that is 39% of the listing. The NAL units up to the first coded slice decide the key frame of a conforming stream (ITU-T H.264 7.4.1.2.3: SEI messages precede the first slice of the primary coded picture; the slices of a picture are all IDR slices or none is). Removing the step bounds the gain at 40% on that file.
- Copies of the PES payload: from the read chunk into a packet, into the PES packet being reassembled (grown by doubling), into the completed payload, and into the access unit. Handing the reassembled bytes on without a copy and sizing the buffer once bounds the gain at 8% to 23% across the four files.
- Per-PID lookups, several ordered-map lookups a packet: a lookup by index into the PID space bounds the gain at 4% to 11%.
- Reading packets: each packet is copied out of the read chunk before it is parsed. On a capture of many small PES packets this and the window check are 30% of the listing.

**Bars.** On the `real` transport streams of draw 7 of v2-heldout ([`docs/corpus.md`](../corpus.md#draw-7)), the median per-file ratio of CoreAV's time to libavformat's, with a bootstrap 95% interval (10,000 resamples, seed 89), overall and in each size class of at least 5 files (smaller classes are reported):

| Operation | Against | Bar |
|---|---|---|
| the packet listing in process (`tools/ts-speed/`: the probe, then every access unit of every kept stream) | libavformat of FFmpeg 9.0.2 in process (`tools/ts-speed/lav_ts.c`): open with the format given, find stream info, read every packet; libavformat reads through a 64 KiB buffer, CoreAV 512 packets a read, both with `pread` | median at most **1.00** and the upper bound of the interval at most **1.00**, overall and in `small`, `medium` and `large` |

The size classes are the draw's: `small` under 16 MB, `medium` 16 MB to under 64 MB, `large` 64 MB to 200 MB (1 MB = 10^6 bytes). The bound on the interval is the tracking issue's goal, no class whose interval reaches above 1.00; a claim that CoreAV is faster needs it below 1.00. The report also gives the share of files on which CoreAV is faster, overall and per class, and, as information and not a bar, `coreav probe` against `ffprobe -v error -of json -show_format -show_streams` of FFmpeg 9.0.2, both as commands.

**Method.** FFmpeg 9.0.2 is the BtbN build `n9.0.2-17-g2a571b6068`: its LGPL shared libraries on the in-process side (archive SHA-256 `7a1ba2af36e5beeb98151e766a903b1d115f2206a86571f17f873cd7292c424f`), and the build of `docs/design/h264.md` for the commands. `lav_ts.c` refuses another library version, or a GPL or nonfree configuration. The host is the one-thread class at a fixed frequency of `docs/design/h264.md` (`c4-standard-8`, one thread a core, `turboMode=ALL_CORE_MAX`, Xeon Platinum 8581C, every timed command on one core), through `scripts/gcp_vm_job.py` with a cap in the script. One file at a time, its bytes on a tmpfs and read once before timing; per file one warm-up run of each engine, then 7 rounds with the engines' order rotated; a file's time per engine is the median of its rounds. Each round also times a second, byte-identical copy of CoreAV's binary, the null arm.

A run is void, and is run again on a new VM, when the VM does not report the turbo mode and the 8581C, a frequency sample of the timing core is more than 3% from 3.1 GHz, the null arm's median per-file ratio is outside 0.99 to 1.01, or the job ends for a reason outside the VM. A run that misses a bar is final for that bar.

**When the code goes to the draw.** Draw 7 is opened once, with the final binary, after the last code change and review. Before that, the code is timed on input 4's 37 files (spent, a tuning pool) on the same class by the same method, and goes to draw 7 only when that gives a median of at most 0.90 overall and at most 0.95 on its 11 files of 64 MB or more. Those numbers decide whether to spend the draw and are reported beside the result, not as evidence for the bars.
