# Transcode shared-frame handoff reference

The handoff change does not bring these class medians within 1% of FFmpeg. AV1's paired point estimates improve by 1.66% with the FFmpeg encoder process and 1.29% with the x264 program. VP9's paired changes are smaller than the observed call variation.

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

