# Transcode shared-frame handoff reference

The phase A handoff change does not bring these class medians within 1% of FFmpeg. AV1's paired point estimates improve by 1.66% with the FFmpeg encoder process and 1.29% with the x264 program. VP9's paired changes are smaller than the observed call variation. The later [phase B snapshot helper](#phase-b-rejected-snapshot-helper) is rejected because it regresses the full-cohort medians.

## Class summaries

Each ratio is computed per file from its three-call arm medians, then summarized by the median across files. Lower is better. The change column is candidate/main, not a division of the other two columns.

| Class | Encoder | Main / FFmpeg | Handoff / FFmpeg | Handoff / main | Handoff at most 1.01 × FFmpeg |
|---|---|---|---|---|---|
| AV1 | FFmpeg process | 1.0416 | 1.0259 | 0.9834 | 1/6 |
| AV1 | x264 program | 1.0479 | 1.0351 | 0.9871 | 1/6 |
| VP9 | FFmpeg process | 1.0262 | 1.0247 | 0.9997 | 3/10 |
| VP9 | x264 program | 1.0164 | 1.0181 | 0.9982 | 2/10 |

These are reference point estimates on previously used lossy inputs, not a held-out speed claim or a newly judged acceptance bar. There are only three calls per cell. The median within-cell range, divided by that cell's median, is 0.54–1.19% for the AV1 CoreAV arms and 0.86–1.42% for VP9; the largest VP9 cell spans 8.93%. Independent medians can move differently from the median paired change.

## Per-file times

Times are seconds, including process startup, decoding, encoding and final muxing. “FFmpeg” is its single-process transcode using the same decoder source revision. The two CoreAV columns for each encoder compare the same settings before and after the handoff change.

| File | Class | FFmpeg | Main, FFmpeg encoder | Handoff, FFmpeg encoder | Main, x264 | Handoff, x264 |
|---|---|---|---|---|---|---|
| `f51a56ed5718` | VP9 | 122.732299 | 124.561163 | 124.553776 | 124.758528 | 125.262511 |
| `a084bd45168c` | VP9 | 0.136926 | 0.153664 | 0.133983 | 0.133904 | 0.133463 |
| `70eec00bc7d2` | VP9 | 3.444012 | 3.523491 | 3.549862 | 3.576186 | 3.550666 |
| `9ffa165e752a` | VP9 | 132.756820 | 136.641551 | 136.211506 | 136.393344 | 136.316171 |
| `584a16b906bb` | VP9 | 1.194716 | 1.275293 | 1.274652 | 1.240866 | 1.220126 |
| `ea684713a5be` | VP9 | 102.207372 | 104.403716 | 104.587342 | 103.878299 | 103.497693 |
| `bb5d39d47357` | VP9 | 2.205539 | 2.313447 | 2.315960 | 2.232149 | 2.240088 |
| `b267a2ddac45` | VP9 | 8.047111 | 8.162051 | 8.101998 | 8.161755 | 8.137717 |
| `f944c05afaa1` | VP9 | 0.135800 | 0.133049 | 0.132698 | 0.132742 | 0.132769 |
| `59ad8ee403b3` | VP9 | 0.211481 | 0.229174 | 0.229885 | 0.228998 | 0.229788 |
| `effb7b78408d` | AV1 | 90.880143 | 94.510678 | 92.645112 | 95.434464 | 93.297364 |
| `357fb30f4693` | AV1 | 111.199454 | 115.246425 | 112.795657 | 132.714867 | 131.033635 |
| `d438e9cb804e` | AV1 | 110.069283 | 117.545013 | 114.187238 | 117.848244 | 115.019279 |
| `03e890cf5999` | AV1 | 2.946417 | 3.122294 | 3.080453 | 3.081097 | 3.040789 |
| `a34d5ee860fb` | AV1 | 0.826986 | 0.862826 | 0.853759 | 0.863335 | 0.858561 |
| `d9ab22f96e5e` | AV1 | 6.297269 | 6.326971 | 6.301024 | 6.357064 | 6.350626 |

## Per-file ratios

The four CoreAV columns divide by the corresponding file's FFmpeg median. The final column instead compares two FFmpeg commands without audio: a two-process raw-NUT pipe versus a single process.

| File | Main, FFmpeg encoder | Handoff, FFmpeg encoder | Main, x264 | Handoff, x264 | Split pipe / video-only |
|---|---|---|---|---|---|
| `f51a56ed5718` | 1.0149 | 1.0148 | 1.0165 | 1.0206 | 1.0168 |
| `a084bd45168c` | 1.1222 | 0.9785 | 0.9779 | 0.9747 | 1.0238 |
| `70eec00bc7d2` | 1.0231 | 1.0307 | 1.0384 | 1.0310 | 1.0238 |
| `9ffa165e752a` | 1.0293 | 1.0260 | 1.0274 | 1.0268 | 1.0502 |
| `584a16b906bb` | 1.0674 | 1.0669 | 1.0386 | 1.0213 | 1.0684 |
| `ea684713a5be` | 1.0215 | 1.0233 | 1.0163 | 1.0126 | 1.0267 |
| `bb5d39d47357` | 1.0489 | 1.0501 | 1.0121 | 1.0157 | 1.0550 |
| `b267a2ddac45` | 1.0143 | 1.0068 | 1.0142 | 1.0113 | 1.0236 |
| `f944c05afaa1` | 0.9797 | 0.9772 | 0.9775 | 0.9777 | 1.0091 |
| `59ad8ee403b3` | 1.0837 | 1.0870 | 1.0828 | 1.0866 | 1.0118 |
| `effb7b78408d` | 1.0399 | 1.0194 | 1.0501 | 1.0266 | 1.0312 |
| `357fb30f4693` | 1.0364 | 1.0144 | 1.1935 | 1.1784 | 1.0214 |
| `d438e9cb804e` | 1.0679 | 1.0374 | 1.0707 | 1.0450 | 1.0270 |
| `03e890cf5999` | 1.0597 | 1.0455 | 1.0457 | 1.0320 | 1.0389 |
| `a34d5ee860fb` | 1.0433 | 1.0324 | 1.0440 | 1.0382 | 1.0264 |
| `d9ab22f96e5e` | 1.0047 | 1.0006 | 1.0095 | 1.0085 | 1.0025 |

## Remaining encoder boundary

Splitting FFmpeg's own video-only command into a decoder/raw-NUT writer and an encoder process costs a median 2.67% on AV1 and 2.38% on VP9. The single and split commands have identical decoded output frame hashes, frame counts and reported rates on all sixteen files.

That is a boundary reference, not a measurement of CoreAV's pipe write alone. It includes a second process, raw-NUT serialization, pipe transfers and different overlap between stages. CoreAV's transports and encoder wrappers differ, and its ordinary arms retain copied audio on three files. These effects cannot be added to the handoff gain or used to assign all of the remaining gap to the pipe. No fresh stage profile was collected.

The external encoder process remains. This change does not introduce an in-process encoder or change encoder licensing or decoder isolation.

## Capture and correctness

The cohort is the same six AV1 and ten VP9 files in [the existing roster](../../bench/transcode-decoder-host/inputs.json), with no replacement or exclusion. All 336 calls completed successfully: sixteen files, seven arms and three rotated rounds. Each file's order starts at `(file_index + round_index) % 7` in `ffmpeg, main_ffmpeg, candidate_ffmpeg, main_x264, candidate_x264, video_only, split_pipe`.

Each input's hash and size are checked and its bytes read before timing. Outputs are on tmpfs. Output probing and frame hashing are outside the timed region. Every arm produces the same 12,120 frames across the cohort, with matching dimensions and pixel formats. Each CoreAV receipt reports the expected decoder, encoder, stream maps, dimensions and frame count, and its diagnostic identifies sandboxed decoding.

All 96 paired main/handoff complete-file SHA-256 comparisons match, covering both encoders and all three rounds. First-round decoded frame hashes also match for all 32 main/handoff encoder pairs. This proves identity with the existing CoreAV path for these inputs, not byte identity with FFmpeg.

Two existing output-rate metadata differences remain: on `70eec00bc7d2`, FFmpeg reports `7511/250` and both CoreAV builds report `30/1`; on `03e890cf5999`, they report `19001/317` and `60000/1001`. Frame counts match and the handoff does not change these differences.

## Host, sources and commands

Capture ran on 2026-11-05 UTC on one standard on-demand `c4-standard-8` reference host, with SMT off and four available cores: Intel Xeon Platinum 8581C, Debian 13, Linux `6.12.113+deb13-cloud-amd64`. The job runs alone; there is no CPU affinity override, host-drift gate or A/A arm.

Main is `b7496b9b65720b1e736549cf6e6b180202275741`; the measured handoff commit is `97a11c8ed43667aed31b593dc8c435fc4e2338c5`. Later kernel-page portability fixes preserve the tested 4 KiB host's slot and allocation geometry, but were not retimed. Both release builds use Rust `1.99.1 (58cf8f9c3 2026-10-13)` and:

```sh
cargo build --locked --release -p coreav-cli --features native-h264,dav1d,libvpx,ffmpeg-fallback
```

FFmpeg and ffprobe are the pinned `n9.0.2-17-g2a571b6068-20260930` build; x264 is `0.165.3223 0480cb0`. The FFmpeg input decoder is explicitly `-c:v libdav1d` or `-c:v libvpx-vp9`, not its native VP9 decoder. CoreAV statically links libvpx `v1.17.0` and dav1d source `9711965b60bb692ae24004659acf61f5c7d9ed61`, matching the pinned oracle's decoder source revisions. The dav1d build labels differ: CoreAV's pkg-config metadata says `1.5.4`, while the oracle reports `1.5.1-235-g9711965`. Library builds and wrapper threading are separate, not identical binary libraries.

CoreAV uses its ordinary sandboxed transcode with `--preset medium --crf 23` and either `--encoder ffmpeg` or `--encoder x264`. FFmpeg uses libx264 with `-preset medium -crf 23 -fps_mode passthrough`, explicit stream maps and copied audio. Its video-only and split arms use `-an`; the split decoder writes raw video in NUT to the second FFmpeg process. Exact normalized invocations, executable hashes, per-call times, output hashes, source archive identities and frame-hash checks are retained in [the compressed capture](transcode-frame-handoff/reference.json.gz). No numerical capture field was changed.

To recompute the class ratios from the repository root:

```sh
python3 - <<'PY'
import gzip, json, statistics as s
p = json.load(gzip.open("docs/measurements/transcode-frame-handoff/reference.json.gz", "rt"))
assert p["status"] == "complete" and len(p["calls"]) == 336 and not p["failures"]
for codec in ("av1", "vp9"):
    files = [f["sha256"] for f in p["inputs"] if f["class"] == codec]
    t = {(f, a): s.median(c["seconds"] for c in p["calls"] if c["sha256"] == f and c["arm"] == a) for f in files for a in p["arms"]}
    for encoder in ("ffmpeg", "x264"):
        main, candidate = "main_" + encoder, "candidate_" + encoder
        print(codec, encoder, *(s.median(t[f, a] / t[f, "ffmpeg"] for f in files) for a in (main, candidate)), s.median(t[f, candidate] / t[f, main] for f in files))
    print(codec, "split/video-only", s.median(t[f, "split_pipe"] / t[f, "video_only"] for f in files))
PY
```

## Phase B: rejected snapshot helper

Reject the snapshot helper. Its paired x264 time is 2.24% higher on AV1 and 15.10% higher on VP9, and none of the sixteen inputs is within 1% of FFmpeg. The helper is removed from the final change; production encoder paths, decoder isolation, defaults and release licensing remain unchanged.

### Class summaries

The same six AV1 and ten VP9 inputs use three-call medians per arm, followed by medians of per-file ratios. The candidate FFmpeg arm is an unchanged-path control. Candidate x264 selects the snapshot helper instead of the pipe-fed CLI. The paired column is computed directly per file, not by dividing the two class-level FFmpeg ratios.

| Class | Encoder | Main / FFmpeg | Candidate / FFmpeg | Candidate / main | Candidate at most 1.01 × FFmpeg |
|---|---|---|---|---|---|
| AV1 | FFmpeg process | 1.0202 | 1.0191 | 0.9979 | 2/6 |
| AV1 | x264 snapshot helper | 1.0251 | 1.0497 | 1.0224 | 0/6 |
| VP9 | FFmpeg process | 1.0244 | 1.0249 | 1.0006 | 1/10 |
| VP9 | x264 snapshot helper | 1.0354 | 1.1756 | 1.1510 | 0/10 |

These are reference point estimates on previously used lossy inputs, not a held-out speed claim or a new acceptance bar. The helper's median within-cell range is 1.40% for AV1 and 0.88% for VP9; the largest helper cells span 1.63% and 6.42%, respectively. The unchanged FFmpeg controls stay near unity in the paired comparison, while the helper is slower on fifteen of sixteen files.

### Per-file seconds

| File | Class | FFmpeg | Main, FFmpeg encoder | Candidate, FFmpeg encoder | Main, x264 | Helper | Helper / main |
|---|---|---|---|---|---|---|---|
| `f51a56ed5718` | VP9 | 118.819727 | 121.017176 | 121.634382 | 122.371950 | 127.492211 | 1.0418 |
| `a084bd45168c` | VP9 | 0.134224 | 0.132934 | 0.132764 | 0.133372 | 0.152943 | 1.1467 |
| `70eec00bc7d2` | VP9 | 3.345926 | 3.434952 | 3.432961 | 3.478085 | 3.839117 | 1.1038 |
| `9ffa165e752a` | VP9 | 128.453075 | 131.293372 | 131.379005 | 131.818721 | 141.632453 | 1.0744 |
| `584a16b906bb` | VP9 | 1.101662 | 1.175359 | 1.176033 | 1.162020 | 2.207948 | 1.9001 |
| `ea684713a5be` | VP9 | 97.343628 | 98.806160 | 99.349992 | 101.069787 | 109.626759 | 1.0847 |
| `bb5d39d47357` | VP9 | 2.069051 | 2.144304 | 2.178706 | 2.136477 | 4.182934 | 1.9579 |
| `b267a2ddac45` | VP9 | 7.428609 | 7.587586 | 7.600348 | 7.587154 | 8.943231 | 1.1787 |
| `f944c05afaa1` | VP9 | 0.121368 | 0.131840 | 0.130858 | 0.131432 | 0.151845 | 1.1553 |
| `59ad8ee403b3` | VP9 | 0.194146 | 0.215024 | 0.208053 | 0.208976 | 0.288591 | 1.3810 |
| `effb7b78408d` | AV1 | 87.158027 | 89.070098 | 89.077062 | 89.222610 | 90.646357 | 1.0160 |
| `357fb30f4693` | AV1 | 107.297256 | 109.276887 | 109.040614 | 125.641714 | 113.287492 | 0.9017 |
| `d438e9cb804e` | AV1 | 108.998967 | 112.367290 | 111.972618 | 112.216201 | 113.744462 | 1.0136 |
| `03e890cf5999` | AV1 | 2.921664 | 3.051124 | 3.061883 | 2.998889 | 4.314554 | 1.4387 |
| `a34d5ee860fb` | AV1 | 0.844907 | 0.853333 | 0.851520 | 0.848301 | 0.872806 | 1.0289 |
| `d9ab22f96e5e` | AV1 | 6.185457 | 6.187909 | 6.165491 | 6.239655 | 6.555983 | 1.0507 |

The profiled 3840 × 2160 10-bit AV1 outlier, `357fb30f4693`, improves from 125.641714 to 113.287492 seconds, a 9.83% reduction. It still takes 1.0558 times FFmpeg, and its improvement does not offset the full-cohort regressions. The strongest regressions occur on some short inputs, but this capture contains no fresh candidate profile that separates startup, snapshot operations, scheduling and encoder API work. Do not attribute the regression to one component from these totals alone.

### Capture and correctness

All 240 calls complete with zero failures: sixteen files, five arms and three rotated rounds. Order starts at `(file_index + round_index) % 5` in `ffmpeg, main_ffmpeg, candidate_ffmpeg, main_x264, candidate_x264`. Source hashes and sizes are checked and source bytes warmed before timing; outputs use tmpfs. Output probing, frame hashing and complete-file hashing are outside timing. The job retains all calls, with no excluded or substituted input.

Every arm produces the same 12,120 frames per round with matching dimensions and pixel formats. All 96 paired main/candidate complete-file comparisons match, covering both encoders and all three rounds; first-round decoded frame hashes match for all 32 CoreAV encoder pairs. This is identity with the existing CoreAV output, not with FFmpeg. Receipts identify the expected native decoder and encoder, stream maps and frame count, and each CoreAV call reports sandboxed decoding. A separate untimed execution trace confirms the candidate launches both the snapshot helper and its x264 timecode-clock probe.

The helper passes 23 native protocol and byte-identity tests on the reference host. Before capture, six synthetic complete-MP4 comparisons through sandboxed AV1/VP9 decoders cover 8-bit, 10-bit and 4:4:4, while ownership and hostile-process tests cover immutable forwarded references, malformed output, crashes, missing acknowledgements, deadlines and sink refusal. These checks establish the evaluated candidate's behavior, not a reason to ship a slower transport.

### Sources and limits

Main is `a451e99e6845acf318c361282766e53c39c5c1d0`; the [evaluated candidate](https://github.com/coreav-dev/coreav/commit/842be356ccb467d0d4890b0ff9ea1f37e36a019b) is `842be356ccb467d0d4890b0ff9ea1f37e36a019b`. Main and candidate use independent Cargo target directories. An initial setup reused a baseline dependency from a common target directory and failed compilation before any timing; its output supplies no performance result. The corrected capture builds each source independently and runs a synthetic preflight before the full cohort.

Capture ran on 2026-11-05 UTC on a standard on-demand `c4-standard-8`, SMT off, four available cores, Intel Xeon Platinum 8581C, Debian 13 and Linux `6.12.113+deb13-cloud-amd64`. Both release builds use Rust `1.99.1 (58cf8f9c3 2026-10-13)` with `native-h264,dav1d,libvpx,ffmpeg-fallback`. There is no affinity override or host-drift gate. FFmpeg and ffprobe are the pinned `n9.0.2-17-g2a571b6068-20260930` build, with explicit libdav1d or libvpx-vp9 decoding, libx264 medium/CRF 23 encoding, passthrough timing and copied audio. The unchanged x264 program and the helper's independently built libx264 use `0.165.3223 0480cb0`. Decoder source revisions remain dav1d `9711965b60bb692ae24004659acf61f5c7d9ed61` and libvpx `v1.17.0`; their CoreAV library builds are separate from the oracle's, as in phase A.

The helper uses a fresh sealed file per frame, retains process isolation and validates output through the existing reader and muxer. It adds per-frame file creation, sealing, opening, mapping and acknowledgement work, plus a zero-frame CLI probe to preserve the encoder's timecode clock. Safety and byte identity do not establish a net speed gain. The full comparison rejects this design; no production helper interface or new linked encoder dependency remains.

Exact normalized commands, helper-selection environment, executable hashes, per-call seconds, complete-file hashes, decoded frame hashes, source identities and host fields are retained in [the compressed phase B capture](transcode-frame-handoff/phase-b-reference.json.gz). No numerical capture field was changed. The source and protocol remain accessible at the evaluated commit for inspection, not as a shipped backend.

To recompute the class ratios and check all paired files from this checkout:

```sh
python3 - <<'PY'
import gzip, itertools, json, statistics as s
p = json.load(gzip.open("docs/measurements/transcode-frame-handoff/phase-b-reference.json.gz", "rt"))
c = {(x["sha256"], x["round"], x["arm"]): x for x in p["calls"]}
assert p["status"] == "complete" and len(p["calls"]) == len(c) == 240 and not p["failures"]
for f, n, e in itertools.product(p["inputs"], range(3), ("ffmpeg", "x264")):
    assert c[f["sha256"], n, "main_" + e]["output"] == c[f["sha256"], n, "candidate_" + e]["output"]
for codec in ("av1", "vp9"):
    files = [f["sha256"] for f in p["inputs"] if f["class"] == codec]
    t = {(f, a): s.median(c[f, n, a]["seconds"] for n in range(3)) for f in files for a in p["arms"]}
    for e in ("ffmpeg", "x264"):
        a, b = "main_" + e, "candidate_" + e
        print(codec, e, *(s.median(t[f, x] / t[f, "ffmpeg"] for f in files) for x in (a, b)), s.median(t[f, b] / t[f, a] for f in files))
PY
```

