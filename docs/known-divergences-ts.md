# Known divergences: `coreav-mpegts` versus `ffprobe`

Every intentional difference from the `ffprobe` oracle is documented, with
a machine-readable twin. The twin is
[`known-divergences-ts.json`](known-divergences-ts.json);
`scripts/oracle_kd_ts.py` applies it to the oracle reports of
`tools/ts-adapter`, and `tools/oracle/tests/test_kd_ts.py` keeps this page,
the twin and the predicates in sync. Each entry covers only the fields it
names; an item counts as agreement only if every gating difference is
covered. Covered items stay in the denominator and are reported apart from
direct agreement. Predicates read the two captures, never file names. The
oracle is the pinned `ffprobe` 9.0.2. The design is in [`design/mpegts.md`](design/mpegts.md).

The entries describe the default packet listing of `coreav-mpegts`, which
the oracle adapter and `coreav probe --scan-frame-sizes` use. `coreav
ffprobe` lists with `Packets::listing_as_ffprobe` instead (the design
note's "The listing as `ffprobe` gives it"), which follows `ffprobe` on
the positions of KD-TS-POSITION and on the AAC frame times of
KD-TS-FRAME-TIME.

## KD-TS-END-ESTIMATE

- Fields: stream `duration_ts`, and `format_duration_us`.
- When: a stream whose end `ffprobe` puts earlier than CoreAV does.
- What each reports: a transport stream states no duration, so both
  readers estimate a stream's end from the end of the file. CoreAV reads
  the last 4 MiB (`max_tail_bytes`) and ends the stream at the latest
  timestamp there plus the duration of the access unit it times. `ffprobe`
  reads less of the tail, and ends the stream at another of the access
  units it lists for it -- on the real transport streams of the v1-ts-audio
  corpus from one to fourteen units earlier, commonly the
  one unit of the PES packet that only the end of the file completes, and
  on one capture three ticks later, because it takes the unit's length
  from the frame rate it reports (3,003 ticks) where the file's own
  timestamps step by 3,000. It also adds none of the last unit for E-AC-3
  and Opus, and half of it for MPEG-1 video, where it reports a frame rate
  twice the sequence header's; for AAC, MPEG audio, AC-3, H.264, HEVC and
  MPEG-2 video it adds the whole unit, as CoreAV does.
- Why: a PTS times the first access unit that starts in its PES packet
  (ISO/IEC 13818-1 2.4.3.7), which plays for its whole duration; the
  stream ends after it. How much of the file to read before estimating is
  each reader's own choice, which the standard does not make.
- Coverage requires, from the file itself (`Facts` in
  `scripts/oracle_kd_ts.py`, a reader written apart from CoreAV's):
  CoreAV's duration runs from the stream's first PES timestamp to the
  latest timestamp in the last 4 MiB plus the duration of the access unit
  that timestamp times (`Facts.coreav_end`: an audio frame's from its own
  header, the smallest step between the timestamps for a video stream or
  for sound whose frames the reader does not read, and none for a
  subtitle, metadata or unnamed stream, whose units state what they last);
  and `ffprobe`'s end is the end of one of the access units `ffprobe`
  itself lists for the stream (a packet's presentation or decoding time
  plus its duration, a unit the file gives no duration ending where it
  starts), or, for the codecs above, is short by exactly that fraction of
  the last unit. The format duration is covered when every stream's agrees
  or is covered, at least one is covered by an entry the file confirms,
  CoreAV's format duration is what its own rule gives from its streams
  (their span, or the longest program's span for a file of several
  programs), and `ffprobe`'s is the span of the streams it times. Where no
  stream differs at all and the two file durations are a microsecond
  apart, that microsecond is the whole difference: CoreAV takes the span
  in 90 kHz ticks and converts it once, where `ffprobe` works in
  microseconds throughout.

## KD-TS-STREAM-TIMING

- Fields: stream `duration_ts`, and `format_duration_us`.
- When: a stream `ffprobe` does not time from its own access units: one
  whose units state no duration of their own (a presentation graphics
  subtitle, whose packets `ffprobe` gives no duration, and a stream with
  one unit), and one whose end its estimate never found (a stream whose
  PES packets carry no timestamp at all, and an MVC video sub-bitstream).
- What each reports: `ffprobe` reports no duration for the first, and for
  the second the duration it reports for the whole file. CoreAV reports
  what the stream itself gives -- from its first presentation time to the
  end of its last access unit -- or nothing when the file times the stream
  nowhere.
- Why: ISO/IEC 13818-1 states no duration for a stream or for a file, so
  both are estimates. A stream whose units state no duration has none to
  estimate from, and the file's duration is the span of the other streams,
  which is not this stream's.
- Coverage requires, from the file itself (`Facts`): CoreAV's duration is
  the one the file gives by its rule (`Facts.coreav_end`, as above); or,
  where CoreAV reports none, the file gives it nothing to end on, which
  the reader must show -- no PES packet of the stream states a time at
  all, a table declaring a stream the file never sends among those
  (`Facts.untimed`), or none of them states one in the last 4 MiB, which
  is all CoreAV reads for a stream's end (`Facts.absent_from_the_tail`).
  A reader that simply could not work the end out covers nothing. And
  `ffprobe` reports no duration, or one equal within a tick to the
  duration it reports for the whole file. The format duration is covered
  as above.

## KD-TS-AAC-PROFILE

- Fields: stream `profile`.
- When: an AAC stream, in ADTS or LOAS, whose payload carries spectral
  band replication that its configuration does not announce.
- What each reports: `ffprobe` opens a decoder while it reads the file,
  which finds the band replication, and names the stream `HE-AAC` (or
  `HE-AACv2` with parametric stereo) at twice the configured sampling
  rate. CoreAV names the profile the stream's own configuration states --
  `LC` for these files -- and does not decode.
- Why: ISO/IEC 14496-3 1.5.1.1 and 4.6.18: band replication may be
  signalled implicitly, by extension payloads inside the raw data blocks.
  Reading them means decoding the audio, which a demultiplexer does not
  do; the configuration is what the stream states about itself.
- Coverage requires, from the file itself (`Facts`): the stream is AAC in
  ADTS (stream type 0x0F) or LOAS (0x11); the audio object type its own
  configuration states (an ADTS header's `profile` plus one, a
  `StreamMuxConfig`'s `audioObjectType`) is the profile CoreAV reports;
  and `ffprobe` reports `HE-AAC` or `HE-AACv2`. The channel count is an
  informational field, which the oracle does not gate. The sampling rate
  is one too, but `--ffprobe-compat` prints it and
  `scripts/compat_check.py` gates every key it prints, so the entry
  covers it there as well: CoreAV reports the rate the file's own
  configuration states (`Facts.aac_rate`) and `ffprobe` twice it, which
  band replication doubles.

## KD-TS-DUPLICATE

- Fields: packet `size`, `pts`, `dts`, `pos` and `keyframe`, and packets
  one side lists and the other does not; in `coreav ffprobe`, also the
  stream's `bit_rate`, which both tools average over the frames they list
  (`docs/design/compat-cli.md`).
- When: a transport packet of an ADTS AAC stream repeats the previous
  packet of its PID: the same continuity counter and the same bytes, its
  PCR aside. The same counter on other bytes is a lost packet, which
  CoreAV does not discard.
- What each reports: CoreAV discards the repeat, and the stream's frames
  are the ones encoded. `ffprobe` reads the repeat as more payload, which
  shifts the audio frames after it.
- Why: ISO/IEC 13818-1 2.4.3.3: a duplicate packet carries the same data
  as the one before it, and a decoder discards it.
- Coverage requires, from the file itself (`Facts`): a duplicate packet
  of the stream's PID, in or before the PES packet of the differing
  packet; an ADTS AAC stream; and every packet CoreAV lists for the
  stream (position, size, timestamps, keyframe flag) equal to the one the
  file gives without the duplicate, by the reader's own reassembly of the
  PES packets and their ADTS frames. Other codecs are not covered.

## KD-TS-FRAME-TIME

- Fields: packet `pts` and `dts`.
- When: an audio frame the file states no time for, that is, not the first
  frame that starts in its PES packet. A PES packet of ADTS AAC commonly
  holds several frames and states one time.
- What each reports: CoreAV places such a frame the samples since the last
  stated time after it, rounded once, so a run does not drift from the
  sample rate. `ffprobe` places it within one 90 kHz tick of that; on a
  44.1 or 22.05 kHz stream, where a frame of 1,024 samples lasts a
  fractional number of ticks, about one frame in five differs by one tick.
  Where the PES packet states a decoding time of its own, earlier than its
  presentation time, the two sides differ by that distance as well: CoreAV
  keeps it, so that every frame of the stream is the same distance ahead
  of its decoding time, and `ffprobe` reports the decoding time as the
  presentation time of every frame it times itself.
- Why: ISO/IEC 13818-1 2.4.3.7 states a time only for the first access
  unit that commences in a PES packet. The times of the others are
  inferred, and the standard defines no arithmetic for inferring them;
  CoreAV's is the exact one (the samples since the anchor, rounded once),
  which neither drifts nor depends on how many frames have passed.
- Coverage requires, from the file itself (`Facts` in
  `scripts/oracle_kd_ts.py`, a reader written apart from CoreAV's): an
  ADTS AAC stream; every packet CoreAV lists for the stream (position,
  size, timestamps, keyframe flag) equal to the one the file gives, by the
  reader's own reassembly of the PES packets and their ADTS frames; the
  differing packet one the file gives no position, that is, not the first
  frame starting in its PES packet; every difference of the packet on
  `pts` or `dts` alone; and the two sides within one tick of each other,
  or, on `pts` alone, `ffprobe` reporting the decoding time CoreAV gives
  the packet -- which the file's own units give it, being the presentation
  time less the distance the PES packet that times the run states between
  the two. Other codecs are not covered.

## KD-TS-DECODE-TIME

- Fields: packet `dts`.
- When: a coded picture of an H.264 or HEVC stream whose PES packet
  states no decoding time of its own -- none at all, or one equal to the
  presentation time.
- What each reports: CoreAV reports the decoding time the file states,
  which is the presentation time. `ffprobe` replaces it with one of its
  own, reconstructed from the order the pictures are coded in: one
  picture interval earlier for each picture the stream reorders, and no
  decoding time at all for the first picture of the stream, before it has
  seen enough of them to reconstruct one. On the real transport streams
  of the v1-ts-audio corpus this is the pictures at the start of a stream
  and at the start of each group of pictures, where the muxer stops
  stating a decoding time because the picture it codes next is not
  reordered.
- Why: ISO/IEC 13818-1 2.7.5 encodes a decoding time only when it differs
  from the presentation time, so a PES packet that states none -- or one
  equal to its PTS -- states that the two are the same; 2.4.3.7 gives
  those times to the first access unit that commences in the packet.
  Reconstructing another means reading the slice headers of every picture
  and the reordering the sequence parameter set allows, which is decoding
  the stream's syntax rather than demultiplexing it, and the result is
  not the file's: on these files it puts a picture's decoding time after
  its presentation time, and gives two pictures the same one.
- Coverage requires, from the file itself (`Facts` in
  `scripts/oracle_kd_ts.py`, a reader written apart from CoreAV's): the
  stream's codec, from the file's tables, is H.264 or HEVC; the packet
  carries a position, which is the start of a PES packet of that PID, and
  that PES header states a presentation time and either no decoding time
  or one equal to it (`Facts.stated`); CoreAV's `pts` and `dts` are both
  that stated time, on the program's timeline; the two sides differ on
  `dts` alone; and `ffprobe` reports no decoding time for the packet, or
  one earlier than that time. A unit the file gives no position -- one
  after the first in its PES packet, whose time this reader infers -- is
  not covered.

## KD-TS-STREAM-ORDER

- Fields: every gating field of a stream, and every field of a packet.
- When: a file with more than one program, whose streams the two readers
  list in a different order.
- What each reports: CoreAV listed the elementary streams of each Program
  Map Table in the order that table gives them, the tables in the order
  they are read. It now lists them in `ffprobe`'s order
  (`docs/schema/probe-mpegts-v2.md`), so the predicate finds no
  reordering in its documents; the entry stays until the overlay drops
  it. `ffprobe` lists the streams of the first Program Map Table completed
  after the PAT in table order
  and the others where the file first shows them from its start, a
  table or a PES packet that is not padding
  (`docs/design/compat-cli.md`, "MPEG-TS"). The two then pair stream 6 of one with
  stream 6 of the other, and every field of nearly every stream, and
  every packet of them, disagrees.
- Why: ISO/IEC 13818-1 2.4.4 makes a transport stream a set of programs,
  each with a Program Map Table of its own; the standard gives no order
  for the streams of a file, and neither reader's is wrong. `--map` by
  index therefore names a different stream on each side for such a file,
  which `docs/ffprobe-compat.md` states.
- Coverage requires, from the file itself (`Facts`): the file declares
  more than one program; CoreAV lists the PIDs the Program Map Tables
  declare in the order they declare them, and nothing else but PIDs no
  table declares -- the Event Information Table, and PIDs the file
  carries PES packets on (`Facts.pes_offsets`), which both readers list
  and no standard places; **every packet CoreAV lists for a
  stream starts in a PES packet of the PID it gives that stream**
  (`Facts.pes_offsets`), so that packets CoreAV routed to the wrong stream
  cannot themselves produce the permutation that would hide them; and
  there is a permutation of `ffprobe`'s streams -- found from the packets
  each side lists for them, the position and size of a stream's packets
  being a fingerprint no other stream of the file shares, a stream
  neither side lists a packet for pairing only with another such stream,
  and the Event Information Table pairing by the name both readers give
  it (`epg`), since it is one table at one fixed PID and the two readers
  list different numbers of its sections --
  under which every gating field of every stream agrees or is covered by
  another entry, and the two file durations agree or the file's follows
  from the streams' as `KD-TS-END-ESTIMATE` requires, with the streams
  paired as the permutation pairs them. At the packets level the permutation covers every
  disagreement but the packets that still differ under it, which stay
  uncovered. `scripts/compat_check.py` pairs the streams of such a file by
  the stream id both readers print, which is the PID, so that each key is
  compared on the stream it belongs to; `index` then differs, by this
  entry.

## KD-TS-TELETEXT-SIZE

- Fields: stream `width` and `height`.
- When: a DVB teletext stream.
- What each reports: `ffprobe` gives it 492 by 250, the page its own
  teletext decoder lays out. CoreAV gives it no size: the file states
  none.
- Why: ETSI EN 300 472 carries teletext as the pages of characters of
  EN 300 706, which have no picture size; 492 by 250 is one renderer's
  choice, not a fact of the stream.
- Coverage requires, from the file itself (`Facts`): the stream's
  descriptors make it DVB teletext; CoreAV reports no size; and
  `ffprobe` reports exactly 492 by 250.

## KD-TS-POSITION

- Fields: packet `pos`, and the fields of the packets that pairing by it
  leaves unpaired (`size`, `pts`, `dts`, `keyframe`).
- When: an H.264 or HEVC access unit (the codec the file's tables give
  the stream) that starts more than 200 KiB into its PES packet's
  payload, which happens after a keyframe of that size.
- What each reports: a PTS and a position belong to the first access unit
  that commences in a PES packet (ISO/IEC 13818-1 2.4.3.7), so CoreAV
  gives the later units of a PES packet no position. `ffprobe` reads a
  PES packet in pieces of at most 200 KiB -- measured on the real
  transport streams of the v1-ts-audio corpus: the
  first unit past 206,134
  bytes of a payload carries a position, the last one short of it, at
  203,665 bytes, does not -- and gives the first unit of each piece the
  position of the piece. The oracle pairs packets by position
  (`tools/oracle/SPEC.md`), so one such unit leaves every later packet of
  the stream paired with the wrong one.
- Why: how much of a PES packet a reader holds at once is its own affair,
  and the standard gives a position to the first access unit of the
  packet only. CoreAV reads a PES packet whole.
- Coverage requires, from the file itself (`Facts`): the PES packets of
  the stream's PID, whose payloads the units CoreAV lists tile exactly,
  one after another; the two sides list the same number of packets for
  the stream and, paired in order, agree on every field but `pos`; every
  position CoreAV gives is the start of the PES packet a unit begins in,
  and is given exactly to the first unit that starts there; and every
  position `ffprobe` gives that CoreAV does not is that PES packet's, for
  a unit starting past 200 KiB of its payload.
