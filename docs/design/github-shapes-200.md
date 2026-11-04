# GitHub Shapes 200 registration

Registered on 2026-11-04, before execution. This is a new measurement named `github-shapes-200`, independent of V1, V2 and V3. This pull request registers the method and supplies its tools. No judged command, calibration or fixture generation runs before it merges. Results belong in a later pull request.

## Frozen population

`tools/compat-corpus/github-shapes-200/shapes.jsonl` contains ranks 1 through 200 from the public-code presence sample collected on 2026-11-03. Each row contains the normalized shape, its original identifier, rank and distinct repository count. Its SHA-256 is `0ca6bcb9b983ade0625f795bd5098cffa12ee638d5ddf94a37c1cefb782d9045`. The builder projects these four fields from the saved sample, checks its identity and ranking, and strips everything else. Repository URLs, source paths, source contents, copied commands, stars and static support labels are not published. Counts alone are sufficient for this measurement.

The sample has 1,818 public repositories, 1,981 shapes and 3,338 repository-shape pairs. A repository votes at most once for a shape. The selected 200 have 1,470 pairs, or 44.0383% of the sample. They include both ffmpeg and ffprobe. GitHub search ranking, capped first pages, language and path strata, excluded source directories, unresolved builders and unobserved private code limit inference. This is neither invocation telemetry nor a probability sample. No worldwide usage share or confidence interval follows from it.

All 200 rows remain in the denominator, including unresolved constructions, unavailable devices, invalid instantiations, refusals and missing predicates. No row, assignment, value or predicate changes after results are observed. Static support does not select inputs or determine success.

## Fixed instantiation

The tools materialize one deterministic command per shape, preserving input/output groups, option order, repeated options and stream roles. They do not search for a successful assignment. Abstract values are representatives, not recovered source literals. Success means success on this registered instantiation, not all possible values or media matching the shape.

Every fixture is generated afresh without held-out media. A generator creates a two-second, 64 by 48, 10 fps video test pattern and 48 kHz mono sine audio using the pinned oracle. The fixture roster is MP4 with H.264 and AAC, Matroska with H.264 and AAC, WAV PCM S16, MP3, PNG, JPEG, GIF, a PNG sequence, headerless S16 and BGR24, and a local two-item concat list. Input positions get separate copies with the same content. Unknown input containers use the MP4, except explicit raw demuxers, which use the corresponding raw fixture. Known containers retain their extension and generated content. Pipe inputs receive the corresponding file bytes on standard input. At most one pipe input is modeled. Devices, network inputs and input playlists have no instantiation. Output unknowns use MP4 unless an explicit output format supplies the extension. Pipe output is captured, numbered images retain their indices, and every output is inspected. HLS and null output commands can execute but have no registered output predicate.

The following values are fixed before execution. `N` becomes index zero within a stream selector. Unrecoverable `<selector>`, `<codec>`, `<format>`, `<enum>`, `<filter>`, unknown settings, missing required values and `-wrapper-unresolved` become `uninstantiable`, never a guessed command. Filter graph edges that normalization discarded cannot be recovered. `-filter_complex`, `-lavfi` and `subtitles` therefore have no instantiation. Simple retained filters use `scale=32:24`, `crop=32:24:0:0`, `fps=5`, `setpts=PTS-STARTPTS`, `volume=0.5`, `silencedetect=noise=-30dB:d=0.1`, `loudnorm`, `palettegen`, `pad=64:48:0:0` and `setsar=1`, composed in their retained order. Other filters remain uninstantiable.

| Option | Representative |
|---|---|
| `-ac`, `-threads` | `1` |
| `-ar` | `48000` |
| `-b:a` | `128k` |
| `-b:v`, `-vb` | `200k` |
| `-ss` | `0.2` |
| `-t`, `-to` | `1` |
| `-r`, `-framerate`, `-g` | `10` |
| `-frames:v` | `3` |
| `-s` | `64x48` |
| `-crf` | `23` |
| `-q`, `-q:v` | `2` |
| `-safe` | `0` |
| `-loop` | `0` |
| `-hls_time`, `-hls_list_size` | `1`, `0` |
| `-start_number` | `0` |
| `-strict` | `-2` |
| `-level` | `3.0` |
| `-auto-alt-ref`, `-lossless` | `0` |
| `-quality` | `good` |
| `-bsf:a`, `-bsf:v` | `aac_adtstoasc`, `h264_mp4toannexb` |
| Probe writer boolean settings | `1` |
| Probe writer separator `s` | `,` |

## Execution and exact predicate

The measured build is Linux x86-64, `cargo build --release --locked -p coreav-cli --features release-full`. This enables `cli,net,dav1d,libvpx,libopus,native-h264,aac`, without the FFmpeg decode-backend feature. Library versions are those of the checkout's Cargo lockfile and build scripts. The workflow records the source SHA, build command, lockfile digest, host image, CPU and kernel, and binary and fixture SHA-256 digests. The FFmpeg and ffprobe executables must pass `oracle_pin.verify_bin` against `scripts/oracle-ffmpeg.env` (FFmpeg 9.0.2). x264 must match the pinned commit there and its binary digest is retained.

GitHub-hosted `ubuntu-24.04` is the only execution host. Nothing is timed for a performance claim. There is no paid-host fallback. Every subprocess has a finite timeout. A reference command has 120 seconds and CoreAV has 300 seconds. The job has a fixed outer timeout. Incomplete records cannot produce a headline.

The runner reuses `encode_breadth`'s process-group deadlines, oracle verification, program-start log and encoder-only invocation classification. Whole-command fallback is disabled by an unusable `COREAV_FALLBACK`, with a precreated fallback log. Every fallback log is required and parsed strictly. Any non-null `ran` voids the acquisition. Unexpected external program starts are misses. Operator-installed x264 and narrowly classified AAC or MP3 encoder-only FFmpeg invocations are allowed and disclosed, not described as end-to-end native execution. Both arms use an empty `GCONV_PATH` and sanitized environment. Standard output is captured, never discarded as evidence.

The same concrete command runs once on the reference and once on CoreAV, with identical relative input names. A reference failure is `reference_failed`, not an exclusion. CoreAV refusal, timeout, error or missing output is a miss. An empty probe response with no requested section is `no_oracle`. Other probes require exact standard-output bytes, including the requested writer's formatting and values. Probe diagnostics on standard error are not equality evidence.

For media outputs the exact decoded-output rules in `docs/measurements/compat-0.1.0/codec-coverage-plan.md` apply, using `decoded_output.py`. Stream order, codecs, required format fields, optional known fields and exact rational timing must agree. All presented samples and pixels agree, with no tolerance, prefix comparison, alignment search, resampling or precision reduction. Nonfinite samples fail. Every numbered image and output must be present. Raw descriptions must follow from the registered command and source, never candidate length. Unsupported output descriptions are `no_oracle`. PNG or BMP requiring YUV-to-RGB conversion remain `no_oracle` under the existing registration.

Unlike V2's lossy quality predicates, this measurement requires exact decoded equality for H.264, VP9, AAC and Opus too. Lossy encodes do not count merely because they meet B4, O4 or A4. This is a deliberately stronger, separate predicate and changes no V2 rule. Encoded bytes and container sizes are not equality predicates. Copied streams additionally require identical packet payload hashes, packet counts and rational PTS, DTS and durations against the same-command reference. No known-divergence exclusions or N/A values turn a miss into a pass.

## Accounting and replay

The headline is `counted shapes / 200`. The weighted headline is the sum of repository counts for counted shapes divided by 1,470. It is the covered share of the top-200 repository-shape pairs, not 200 unique repositories and not 44% of real executions. Also report the numerator over all 3,338 sample pairs, explicitly as a lower-bound sample presence share. There is no bar, required count, release verdict or performance claim.

Each shape receives exactly one result in this order: `uninstantiable`, `reference_failed`, `timeout`, `refused`, `failed`, `unexpected_program`, `no_oracle`, `different`, or `counted`. Break out both shape counts and repository weights for every miss class and both tools. A malformed fallback log, wrong oracle, changed registration, changed fixture or binary, or missing/duplicate result makes the record incomplete rather than reducing a denominator.

An acquisition bundle retains generated fixtures, commands, binary identities, complete stdout, output files, fallback and program logs, and per-command process outcomes. The manifest binds evidence and method files by SHA-256. Replay verifies these identities, regenerates the commands from the frozen set, checks the complete roster and reapplies the output comparisons with the pinned oracle. It does not trust stored pass booleans or summary totals. Replacing a result label cannot create coverage. Artifacts use only generated media and relative names. Summaries contain closed reason labels, not arbitrary process diagnostics or machine paths.

## Order and permitted tests

1. Commit this plan before implementation or any judged execution.
2. Implement the builder, instantiator, generator, acquisition, replay, summarizer and manual main-only Actions workflow in the same pull request.
3. Test with synthetic shape objects, fake executables and small independently constructed controls. Tests may validate the frozen roster's digest and pure instantiation but must not execute its commands or generate its judged fixtures before merge. CI on this pull request runs tests only.
4. After merge, dispatch from main at a clean merged SHA containing these exact tools. The runner independently checks main membership and clean method files before fixture generation. Run once. Retain failures as misses. Only recorded infrastructure or harness faults permit replacement acquisition, with the original bundle retained and the reason disclosed.
5. Submit results separately with the set digest, method and binary identities, host, all 200 outcomes, weighted arithmetic and replay evidence. A method change after observation requires a new registration, not relabeling this result.

No commands-v1, commands-0.1.0, judging policy, known-divergence record, held-out input, existing bar, tolerance, baseline or V1 to V3 record changes.
