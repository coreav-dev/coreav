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

The measured build is Linux x86-64, `cargo build --release --locked -p coreav-cli --features release-full`. This enables `cli,net,dav1d,libvpx,libopus,native-h264,aac`, without the FFmpeg decode-backend feature. Cargo dependencies use the checkout's lockfile. The Actions image supplies dav1d, libvpx and libopus development packages. Their actual versions and linked-library SHA-256 digests are recorded, not assumed stable across host images. The workflow records the source SHA, build command, lockfile digest, host image, CPU and kernel, and binary and fixture SHA-256 digests. The FFmpeg and ffprobe executables must pass `oracle_pin.verify_bin` against `scripts/oracle-ffmpeg.env` (FFmpeg 9.0.2). x264 must match the pinned commit there and its binary digest is retained.

GitHub-hosted `ubuntu-24.04` is the only execution host. Nothing is timed for a performance claim. There is no paid-host fallback. Every subprocess has a finite timeout. A reference command has 120 seconds and CoreAV has 300 seconds. The job has a fixed outer timeout. Incomplete records cannot produce a headline.

The runner follows `encode_breadth`'s process-group deadline policy and reuses its oracle verification, program-start log and encoder-only invocation classification. The reference's log level is `info` for media mapping and `error` for probes. This changes no output option. Whole-command fallback is disabled by an unusable `COREAV_FALLBACK`, with a precreated fallback log. Every fallback log is required and parsed strictly. Any non-null `ran` voids the acquisition. Unexpected external program starts are misses. Operator-installed x264 and narrowly classified AAC or MP3 encoder-only FFmpeg invocations are allowed and disclosed, not described as end-to-end native execution. Both arms use an empty `GCONV_PATH` and sanitized environment. Standard output is captured, never discarded as evidence.

The same concrete command runs once on the reference and once on CoreAV, with identical relative input names. Reference inputs and later decoded-output comparisons use `oracle_audio.INPUT_ARGS` to fix audio decoder threading, without changing video threading or output options. A reference failure is `reference_failed`, not an exclusion. CoreAV refusal, timeout, error or missing output is a miss.

For a successful reference probe, the pinned oracle also emits an eligibility witness with the same inputs, selectors and requested sections, changing only its writer to JSON. This auxiliary command has the reference's 120-second limit; its invocation, exit, stdout and stderr are retained and bound during replay. A failed, timed-out or malformed witness is `no_oracle`. Empty sections, missing or unavailable explicitly requested fields, and wholly unavailable broad records are also `no_oracle`. Broad records may contain unavailable optional fields when other substantive data exists. This one rule applies to every selected writer. No default, flat, CSV, INI or compact grammar is implemented by CoreAV's measurement tools.

The original requested serialization counts only when both primary exits are zero, both stdout captures are nonempty, their complete bytes agree, and the oracle witness establishes substantive requested data. A differing candidate serialization remains a miss even when its witness would describe valid data. The witness cannot replace or relax byte equality, and a missing or substituted witness receipt invalidates the evidence. Probe diagnostics on standard error are not equality evidence.

For media outputs the exact decoded-output rules in `docs/measurements/compat-0.1.0/codec-coverage-plan.md` apply, using `decoded_output.py`. Stream order, codecs, required format fields, optional known fields and exact rational timing must agree. All presented samples and pixels agree, with no tolerance, prefix comparison, alignment search, resampling or precision reduction. Nonfinite samples fail. Every numbered image and output must be present. Raw descriptions must follow from the registered command and source, never candidate length. Unsupported output descriptions are `no_oracle`. PNG or BMP requiring YUV-to-RGB conversion remain `no_oracle` under the existing registration.

Unlike V2's lossy quality predicates, this measurement requires exact decoded equality for H.264, VP9, AAC and Opus too. Lossy encodes do not count merely because they meet B4, O4 or A4. This is a deliberately stronger, separate predicate and changes no V2 rule. The existing x264, AAC and MP3 encoder-start evidence is still required for those reference jobs. Missing encoder provenance is `no_oracle`. Encoded bytes and container sizes are not equality predicates. Copied streams additionally require identical packet payload hashes, packet counts and rational PTS, DTS and durations against the same-command reference. No known-divergence exclusions or N/A values turn a miss into a pass.

## Accounting and replay

The headline is `counted shapes / 200`. The weighted headline is the sum of repository counts for counted shapes divided by 1,470. It is the covered share of the top-200 repository-shape pairs, not 200 unique repositories and not 44% of real executions. Also report the numerator over all 3,338 sample pairs, explicitly as a lower-bound sample presence share. There is no bar, required count, release verdict or performance claim.

Each shape receives exactly one result in this order: `uninstantiable`, `reference_failed`, `timeout`, `refused`, `failed`, `unexpected_program`, `no_oracle`, `different`, `compatible_avc1`, `compatible_aac`, or `counted`. The separately registered compatibility outcomes below are misses for the exact headline. Break out both shape counts and repository weights for every miss class and both tools. A malformed fallback log, wrong oracle, changed registration, changed fixture or binary, or missing/duplicate result makes the record incomplete rather than reducing a denominator.

An acquisition bundle retains generated fixtures, commands, binary identities, complete stdout, output files, fallback and program logs, and per-command process outcomes. The manifest binds evidence and method files by SHA-256. Replay verifies these identities, regenerates the commands from the frozen set, checks the complete roster and reapplies the output comparisons with the pinned oracle. It does not trust stored pass booleans or summary totals. Replacing a result label cannot create coverage. Artifacts use only generated media and relative names. Summaries contain closed reason labels, not arbitrary process diagnostics or machine paths.

## Order and permitted tests

1. Commit this plan before implementation or any judged execution.
2. Implement the builder, instantiator, generator, acquisition, replay, summarizer and manual main-only Actions workflow together with this plan.
3. Test with synthetic shape objects, fake executables and small independently constructed controls. Tests may validate the frozen roster's digest and pure instantiation but must not execute its commands or generate its judged fixtures before merge. PR CI also parses actual pinned-oracle writer output from a separate 16-sample, 8 kHz WAV control, covering default, JSON, CSV, flat, compact, INI and key/wrapper variants. It never invokes the acquisition or judged fixture generator.
4. After merge, dispatch from main at a clean merged SHA containing these exact tools. The runner independently checks main membership and clean method files before fixture generation. Run once. Retain failures as misses. Only recorded infrastructure or harness faults permit replacement acquisition, with the original bundle retained and the reason disclosed.
5. Submit results separately with the set digest, method and binary identities, host, all 200 outcomes, weighted arithmetic and replay evidence. A method change after observation requires a new registration, not relabeling this result.

No commands-v1, commands-0.1.0, judging policy, known-divergence record, held-out input, existing bar, tolerance, baseline or V1 to V3 record changes.

## Separate AAC compatibility registration

Registered on 2026-11-06 before implementation or decoder comparison. The maintainer requested that qualified AAC decode-tolerance commands count as compatible, separately from exact output, following coreav-dev/coreav#433. The new `compatible_aac` result never increases `counted`, `counted_pairs`, `top_200_pair_share` or `whole_sample_presence_lower_bound`. PNS is never exact. Frozen results, gate records, published numbers, population, fixtures and command assignments do not change. A new measurement result requires a separate later submission.

### Predicate and scope

Reuse the complete [AAC PCM qualification](aac-pcm-eligibility.md) and its [PNS extension](aac-pns-eligibility.md), including the existing production preflight. This section adds AAC scope separately; the avc1 registration's statement that it does not admit AAC tolerance describes only that registration. There is no new tolerance. No-PNS PCM requires complete finite F32 samples within 2^-16 and actual S16 samples within one LSB. PNS requires independently checked per-band energies within 0.1 dB for both decoders, the registered deterministic-bin and non-PNS projection bars, each decoder's actual S16 packing against its own F32, and exact metadata-predicted presentation intervals. Cross-decoder pointwise comparison of complete PNS waveforms is neither required nor sufficient.

Only a single seekable MP4 input with one explicit mono or stereo AAC-LC audio track and a single audio-only output can qualify. Source video may be present but cannot be output. Retain the qualification's bounded nonfragmented timelines, complete packet scan, clean decoding, window-history rules and excluded PNS tool combinations. Output is raw F32, raw S16 or S16 WAV, with native rate and channel count. Explicit `-ar` and `-ac` must be identities. Permit only the existing default audio selection, optional input `-threads 1`, output `-vn`, `-c:a`, `-f`, `-ar` and `-ac`. The complete reference mapping must show AAC decoding to the corresponding PCM encoder, including when encoder selection is implicit. No seek, trim, filter, mixing, other encode, copied stream, multiple input or multiple output is qualified.

The candidate ranks within this scope are 5, 13, 33, 43, 53, 57, 67, 72, 77, 89, 98, 139, 168 and 174. These are candidates, not observed passes. Ranks 15 and 188 use input pipes, which the existing seekable-input qualification does not cover. Rank 44 writes PCM in MP4, whose complete framing is not qualified. F32 WAV, other containers and unsupported or specification-undecidable presentations remain out. Do not infer qualification from a rank or codec label.

The exact-credit guard follows AAC decoding in the reference mapping, not the output codec. Every non-copy output decoded from AAC needs a successful complete stored-packet diagnostic scan showing no PNS before exact credit. Unknown scans and PNS in unqualified output forms are `different`. This can only lower exact credit; it does not qualify an encoded output or alter its native path. Copied AAC keeps its existing packet and decoded predicates. The scan never reads a judged input before merge.

### Implementation and verification order

1. Commit this plan first. Implement only measurement tooling, qualification-output export, generated controls and workflow wiring. Do not change native decoding, MP3 or any frozen artifact.
2. For an eligible command, rerun the existing complete qualification with the same bound CoreAV, pinned FFmpeg 9.0.2 and opt-in diagnostic binaries. Export the qualified raw outputs for both arms without changing acceptance. Require each measured payload to equal its own arm's qualified payload byte for byte. This binds actual outputs to the numerical proof and exact count, priming and channel order. A content-admission flag or stored pass label cannot substitute for this proof.
3. Require everything outside the AAC PCM payload to be exact. Raw outputs have no framing. For S16 WAV require valid complete extents, sample fields and byte-identical framing and metadata, with payloads equal to their respective qualified raw outputs. File-output stdout must be empty, with no undeclared files or external encoder starts. A framing or payload mutation fails even if separate qualification passes. Compare WAV framing against the same-command reference output, not the qualification's metadata-discard control. Keep the exact decoded comparator first. Only a decoded-sample mismatch may use tolerance; other mismatches remain misses. Before exact credit for AAC-decoded PCM, a successful complete diagnostic scan must establish that no packet contains PNS. Unknown diagnostics fail closed. Even accidentally equal PNS samples require full qualification and receive only compatible credit. Qualification failures remain `different`, not `no_oracle`.
4. Keep existing non-AAC exact and avc1 behavior. Report the AAC category and its repository weights separately. Replay requalifies the bound input and checks actual output bytes, rather than trusting labels. Bind diagnostic build command, binary digest, qualification dependencies and method files to the acquisition manifest. Historical manifests cannot be rejudged under the new method.
5. Before merge, run only independent generated controls and fake-executable or pure-accounting tests. Cover no-PNS and separable PNS, both PCM widths, raw file and stdout, S16 WAV file and stdout, identity options, unsupported profiles or timelines or PNS tools, count drift, nonfinite samples, changed payloads, metadata and framing, and separate shape and weighted accounting. Use the verified pinned oracle and unchanged transform controls. Do not execute judged commands, the judged fixture generator, held-out inputs or retained judged outputs with this predicate before merge.

## Separate avc1 compatibility registration

Registered on 2026-11-06 before implementation or execution of this predicate. The maintainer permits a separate `compatible_avc1` outcome for copied H.264 parameter-set carriage. Like the separate [AAC decode-tolerance qualification](aac-pcm-eligibility.md), it never increases an exact-output count. The existing `counted` predicate, frozen exact results, published numbers, population, fixtures and gate records stay unchanged. This registration does not admit AAC tolerance into this measurement.

ISO/IEC 14496-15:2017 clause 5.2, Table 2, rows 7 and 8 prohibit SPS and PPS NAL units in `avc1` and `avc2` samples. The seventh-edition FDIS w23225 clause 5.2, Table 1 repeats this restriction. Its clause 5.4.4 permits dual storage for `avc3` and `avc4`, not `avc1`. The published 2024 edition's complete corresponding text has not been verified. The [published 2017 table](https://ossrs.io/lts/zh-cn/assets/files/ISO_IEC_14496-15-AVC-format-2017-e51a22786929a5fbc2f9dc9fc4761e09.pdf#page=23) and [official FDIS](https://dms.mpeg.expert/doc_end_user/documents/144_Hannover/wg11/MDS23225_WG03_N01072.zip) supply the inspected clauses.

The implementation plan is:

1. Keep exact decoded comparison and the original copied-packet hash comparison first. Only a copied-packet mismatch may attempt `compatible_avc1`.
2. Put the byte predicate in `decoded_output.py`, beside the shared decoded-output owner. Obtain complete packet bytes and configuration bytes through the pinned ffprobe binary and bind each byte string to its reported size and SHA-256. Require matching, completely parsed `avcC` records with nonempty SPS and PPS arrays and `avc1` on both arms. Parse length-prefixed packets without recovery or prefix truncation. CoreAV samples contain no SPS or PPS. Remove only reference SPS or PPS NAL units whose complete bytes equal an entry of the corresponding `avcC` array. Require at least one removal and exact remaining packet bytes, packet order, counts and rational PTS, DTS and positive durations. Reject differing configurations, `avc3`, malformed lengths, unknown parameter sets and every other payload change. Parameter-set extension and subset SPS NAL units in either arm's samples also refuse.
3. Require exact complete decoded video and audio, all existing stream fields, and identical presented frame timelines, including both copied and encoded audio. Every video and audio frame in every output needs a known presentation timestamp and a positive duration. No alignment, sample tolerance or precision reduction applies. Every other output and copied stream must still pass its original gates. A later miss overrides an earlier compatible output as `different`, including a later unavailable oracle. The original predicate had already reached `different` on that earlier packet mismatch, so no old `different` result becomes `no_oracle`.
4. Report compatible shape counts and repository weights separately. Preserve the exact numerator and every exact share. Replay recomputes the category from bound output bytes, not stored labels. Historical bundles retain their original method binding and cannot be rejudged by substituting these tools.
5. Test only generated byte records, fake executables and independent generated controls before merge. Cover positive removal, unchanged exact classification, changed SPS or PPS, other NAL changes, retained candidate parameter sets, malformed configuration and packets, `avc3`, non-copy streams, timing, decoded samples, mixed outputs, separate accounting and replay. Do not execute judged commands, the judged fixture generator, held-out inputs or the new predicate on retained judged outputs before this registration merges. Any later measurement needs a separate result submission without relabeling frozen exact results.

## Copied-packet keyframe amendment

Registered on 2026-11-08 before implementation or judged execution. The maintainer requested keyframe comparison on both the exact and avc1-compatible paths. Equal payloads and timestamps alone do not establish equal random-access signalling. For every copied packet, both paths additionally compare whether the pinned ffprobe packet `flags` contains `K`. A keyframe mismatch is `different`, never exact or compatible credit. Missing keyframe evidence cannot earn credit. Other packet flags and side data remain outside this amendment.

Commit this amendment first, then add regressions that change only a packet's keyframe flag on each path, including copied audio and later packets. Preserve the existing `github_shapes.method()` SHA-256 binding for the plan and comparator files. No population, fixture, threshold or unrelated judging registration changes. Before merge, test only synthetic records and independent controls, never judged commands, judged fixtures, retained judged outputs or held-out inputs. Any later judged run requires the merged method and a separate result submission without relabeling historical results.
