# Remote media: HTTP(S) URLs read by byte ranges

Status: Accepted. Scope: HTTP range and object-storage readers.

## What it does

Every `coreav` command that reads a media file also reads a URL:
`coreav probe https://…`, and likewise `check`, `clip`, `split`,
`extract-audio`, `remux`, `sanitize`, `extract-subtitles` and `chapters`.
The file is read by byte ranges, never downloaded whole, and the commands'
outputs, documents and exit codes are those of a local file. A read that
fails is an I/O error (exit 3) whose `coreav.error/1` message gives the
remote reason (`remote: HTTP 404`, a refused redirect, a cap).

The Python module (`coreav.probe("https://…")`) reads URLs the same way,
with the stricter network rules below.

## Choices

1. **One read plan.** The plan (which
   ranges to request, when, and how far ahead) is the crate
   `coreav-range` (cursors for scattered reads, pipelines of parallel segments for
   forward passes, advice from the library, a recent-bytes cache, pinning,
   caps, retries). It is sans-IO: a host starts requests and moves bytes,
   and the plan never blocks or allocates threads itself. Its numbers are
   parameters: `Settings::NATIVE` sets the memory and request caps for a
   process. Choices of the
   plan:
   - **A jump past a response still mostly unread keeps it open.** A read
     past the end of the latest response counts as reading on (which
     starts a pipeline and ends that response) only if at most 1 MiB of it
     is still unread. A probe that reads a small file's start, an index at
     its end and its start again otherwise ended the first response and
     paid a third round trip.
   - **The tail.** With `tail_bytes` (1 MiB natively), the
     last bytes of the object are asked for together with the first
     request, as a suffix range that needs no size. An index stored at the
     end (an MP4 movie box written last, Matroska cues) is then read
     without a second round trip, which on object storage costs about a
     quarter second (R2's time to the first byte, measured from the
     workspace). Unused, it costs one request and at most 1 MiB.
   - **The first span**, natively 32 MiB: closing a
     connection bounds what a cancelled response transfers, and a scan
     from the start then runs further before its pipeline pays a second
     round trip. Chosen on development files (not held out): the Matroska
     remux went from 1.10 to 0.90 times `ffmpeg`, while a 64 MiB span with
     a 16 MiB skip made probes slower (reading through a gap only pays
     below about the time to the first byte times the bandwidth, some
     12 MB here).
2. **The native host is blocking, one thread per request.** `ureq` (a
   small blocking HTTP client in safe Rust over `rustls`) makes each
   request on its own thread, which reads the response into a queue
   bounded by the plan's read-ahead and stops at its next read once the
   plan cancels it (at the latest when the body timeout ends it). A
   pipeline's segments are then fetched at once. TLS uses
   `rustls` with the *ring* provider and the operating system's root
   certificates (`rustls-native-certs`: the OpenSSL files on Linux, the
   keychain on macOS), so no certificate bundle is shipped. Rejected: an
   async runtime (`tokio`, `reqwest`), a much larger dependency tree for a
   handful of concurrent requests; `curl` bindings, C code outside the
   `unsafe` policy; `webpki-roots` and `rustls-platform-verifier` (which
   brings `webpki-root-certs`), whose licence (CDLA-Permissive-2.0) is not
   in `deny.toml`.
3. **A spool.** The native host writes every byte it receives to an
   unnamed temporary file (readable by its owner only, unlinked at once on
   Unix; at most 4 GiB, 2 GiB in the guarded mode) and serves any read it
   holds from there, through `Host::keep` and `Host::kept`. A Matroska
   remux reads the file twice (listing, then copying), and the listing
   pass's reads through gaps had made it fetch a 35 MB file 2.6 times;
   with the spool it fetches it once. Rejected: a larger memory cache,
   which would hold the whole object in memory.
4. **Redirects are followed only to HTTPS**, at most five per request, each
   checked like the first URL. A server that ignores ranges is read only
   from byte 0 (the plan's `whole` case). A strong ETag is pinned with
   `If-Match`; a changed object fails the read (exit 3). Response bodies
   are not decoded (`Accept-Encoding` is not sent), since ranges address
   the stored bytes.
5. **Limits.** Per object: the plan's request cap (10,000 natively), an
   optional byte cap (16 GiB in the guarded mode below) and, in the guarded
   mode, a deadline of an hour; per request: a connection timeout (15 s),
   an idle timeout (60 s) for the response head and for each wait for body
   bytes, and a body timeout (300 s) after which the request ends and the
   plan requests the rest again, so that a server sending a byte now and
   then holds no thread for long. Memory follows the bytes held, not the
   reads: received bytes are added to the last kept piece when they follow
   it (up to 256 KiB a piece), so a server that sends its body a byte at a
   time cannot make the reader keep a buffer per byte. Request threads are
   started with `std::thread::Builder`, and a refused thread is a failed
   request. Retries follow the plan (two per read, for failures a new
   request can fix).
6. **The network rules depend on who chose the URL.**
   - The command line (`Options::command`): any address, `http://` and
     `https://`, the environment's proxy variables. The user runs the
     command on their own URL, and a server on the local network is a
     normal input.
   - The Python module (`Options::guarded`): `https://`
     only, public addresses only, no proxies. A host name is resolved and
     every address it resolves to is checked (loopback, private, link-local,
     shared CGNAT, unspecified, broadcast, multicast, documentation,
     benchmarking, 6to4 relay and reserved ranges are refused; for IPv6 also
     unique-local, site-local and the local-use NAT64 prefix, and
     IPv4-mapped, IPv4-compatible, NAT64 and 6to4 addresses by their IPv4
     address). The connection is made to the checked addresses,
     so a second DNS answer cannot redirect it (DNS rebinding), and every
     redirect hop is checked again. A URL with credentials is refused.
     Python callers that read their own local servers pass
     `private_network=True`.
   Rejected: one rule for all, which either blocks local servers on the
   command line or leaves a service that reads the URLs it is given free to
   reach the cloud metadata address (169.254.169.254) or a database on
   localhost.
7. **Object storage through presigned URLs, not `s3://` with SigV4.** A
   presigned GET URL (S3, R2, GCS) is an HTTPS URL with ranges, so it needs
   nothing more; it carries its own expiry and scope, and the credentials
   stay with whoever signs it. `s3://` would add request signing
   (HMAC-SHA256 dependencies, clock skew handling), credential discovery
   from the environment and per-provider endpoints, and would put long-lived
   credentials in the process an agent drives. Rejected for now: `s3://`;
   revisit if users ask for it.

8. **Diagnostics.** `COREAV_REMOTE_STATS=1` prints the reader's requests and
   bytes on standard error when it ends, `COREAV_REMOTE_TRACE=1` each read
   the parsers make, and `COREAV_REMOTE_SETTINGS` (`name=value` pairs)
   changes the plan's numbers on the command line, for measurements; the
   guarded mode ignores it.

## Known limits

- **A server that ignores ranges has 300 s per object.** Its response is
  the whole object from byte 0, and the body timeout is a total time per
  response, so the request is cut after 300 s. The read is then tried
  again from its own offset, past byte 0, which such a server can only
  answer with the whole object again: the read fails (exit 3) with the
  message that the server does not answer range requests. Range-capable
  servers are not affected: a request cut by the timeout is requested
  again from where it stopped.
- **`hls` reads local files only.**

## Speed bars (fixed before the run)

- **Operations and baselines (the same URLs, from the same machine):**
  - `coreav probe URL` against `ffprobe -v error -of json -show_format
    -show_streams URL`;
  - `coreav remux URL out.mp4` against `ffmpeg -v error -i URL` with the
    same streams mapped, `-c copy -movflags +faststart out.mp4`;
  - `coreav extract-audio URL out.m4a` against `ffmpeg -v error -i URL
    -map 0:<the same stream> -c copy out.m4a`;
  - `coreav clip --start S --end S+10 URL out.mp4` against `ffmpeg -v error
    -ss S -i URL -t 10` with the same streams mapped `-c copy out.mp4`,
    with S a quarter of the duration (rounded down to a second).
  Both sides are command lines, timed from outside with process start-up
  included.
- **Inputs:** held-out `real` corpus v1 objects in R2, through presigned
  URLs: the 30 MP4/MOV/M4A and the 30 Matroska/WebM files with the lowest
  SHA-256 among those of at most 256 MiB that the operation accepts (clip
  and extract-audio: MP4/MOV only, with the stream the operation needs).
  Classes: ISO base media and Matroska.
- **Method:** per input, one warm-up run of each side, then three rounds
  with the order alternated; a side's time is the median of its rounds;
  the ratio is ours over the baseline's, per input.
- **Bar:** for each operation, the median of the per-input ratios is at
  most **1.00** overall and in each class. A "faster" claim needs the
  upper bound of the bootstrap 95% confidence interval below 1.00.

**The other commands (fixed before their runs).** The same inputs
rule, method and bar, with these operations and baselines:

- `coreav check --policy docs/schema/policy-examples/web-upload.json URL`
  (the default size source scans every sample's headers) against `ffprobe
  -v error -count_packets -show_streams URL`, which also reads every packet
  (the local comparison of `docs/design/frame-sizes.md`, S6). Classes: ISO
  base media and Matroska.
- `coreav split --every 10 URL DIR/%03d.mp4` against `ffmpeg -v error -i URL
  -map <the same streams> -c copy -f segment -segment_time 10
  -reset_timestamps 1 DIR/%03d.mp4` (the local comparison of
  `docs/design/split.md`). ISO base media only (MP4, MOV, M4A), one class.
- `coreav sanitize URL out.mp4` against `ffmpeg -v error -i URL -map <the
  same streams> -c copy -map_metadata -1 -map_chapters -1 -movflags
  +faststart out.mp4` (`docs/design/sanitize.md`). ISO base media only (MP4,
  MOV, M4A), one class.
- `coreav extract-subtitles URL out.vtt` against `ffmpeg -v error -i URL -map
  0:s:0 -f webvtt out.vtt`, on every held-out file of at most 256 MiB
  tagged `subtitle-track` (4: two `mp4`, two `m4b`), one class.
- `coreav chapters URL` against `ffprobe -v error -of json -show_chapters
  URL`, on every held-out file of at most 256 MiB tagged `chapters` (3:
  one `mp4`, two `m4b`), one class.

Round 1 for each of these operations over URLs. The subtitle and chapter
sets are the only held-out inputs with those tracks, so their medians
rest on 4 and 3 inputs: they can meet the bar, but no "faster" claim can
rest on them.

**Result: met for every operation** (`docs/spikes/remote-bench/`, the `dist`
build of `ecee36b`), medians of the per-input ratios:
- `check` 0.822 (ISO 0.548, Matroska 0.907);
- `split` 0.782;
- `sanitize` 0.813;
- `chapters` 0.582 (3 inputs);
- `extract-subtitles` 0.072, on 1 input: the other 3 tagged files are the
  chapter files, whose text track is a chapter list.

`check` (overall and ISO), `split` and `sanitize` are faster than FFmpeg
reading the same URLs, their upper bounds below 1.00. `check` on Matroska
is on par (upper bound 1.015).

**Result: met** (`docs/spikes/remote-bench/`, held-out, against FFmpeg's HTTP
reader on R2): probe 0.833 (ISO 0.542, Matroska 0.946), remux 0.800 (ISO
0.711, Matroska 0.909), extract-audio 0.681, clip 0.454, medians of the
per-input ratios. Clip again after the cut point check
(`docs/design/cut-points.md`): 0.419 held out (`clip-02`), 0.402 on the
final code over the same, no longer held-out selection (`clip-03`). Every
interval's upper
bound is below 1.00 except the Matroska probe's (1.092): at parity there,
faster everywhere else.

