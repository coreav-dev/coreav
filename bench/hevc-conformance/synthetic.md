# Synthetic Main10 reference preparation

These four tiny recipes supplement the published conformance data with unfiltered Main10 intra references. They prepare reference data only. No CoreAV entropy decoding, reconstruction or G2 verdict is supplied here. Commit this registration and its expected hashes before any G2 judging; the parent conformance driver must independently classify the generated headers.

## Reproduce the references

Use the pinned Linux x86-64 FFmpeg and FFprobe installation. Their exact executable SHA-256 values, version identities, every encoder argument and every expected digest are in [`synthetic.json`](synthetic.json). Each complete version line is the program name, ` version `, `BUILD` from `scripts/oracle-ffmpeg.env`, and the recorded `version_suffix`, so the shared pin remains the only pin owner. The encoder version reported by the binary is `4.3+49-020d7054`. Only the binaries were used, with no encoder source, documentation or technique notes.

```sh
python3 scripts/hevc_fixture.py verify \
  --ffmpeg "$ORACLE/ffmpeg" --ffprobe "$ORACLE/ffprobe" \
  --output "$WORK/synthetic"
python3 -m unittest discover -s scripts/tests -p test_hevc_fixture.py
```

`verify` regenerates every recipe twice, requires identical compressed and reconstructed bytes, checks both executables against the registration, and compares every observed field to the committed record. `prepare` runs the same reference checks but writes a candidate record without accepting or modifying the committed registration. Both commands write `OUTPUT/reference.json` only after success. A changed executable, recipe, compressed digest, reconstructed digest or SEI hash fails verification.

Each generation produces `OUTPUT/run-1/ID.hevc` and `OUTPUT/run-1/ID.yuv`, with a second copy under `run-2`. The raw files contain two 64×64 planar 4:2:0 pictures in `yuv420p10le`, 12,288 bytes per picture. No media is committed. The only media input is the registered lavfi expression. Subprocesses receive a closed environment without credentials, one encoding thread, no thread pool, disabled encoder assembly, a 30-second wall deadline, a 20-second CPU limit, a 2 GiB address-space limit, a 2 MiB file limit and a 1 MiB limit on each captured output stream. These are preparation bounds, not performance measurements.

## Registered cases

All pictures are independently decodable IDRs. Binary header traces confirm profile 2 with Main10 compatibility, depth 10, no conformance window, no SAO and PPS deblocking disabled. `keyint=16` with forced IDRs retains Main10 signalling; the binary's `keyint=1` configuration instead selected Main10 Intra in profile 4 without Main10 compatibility, so it was not registered.

| ID | Configured CTU / maximum transform | Scaling lists | Transquant bypass | HEVC bytes | HEVC SHA-256 |
|---|---|---|---|---:|---|
| `main10-ctu16-tu4` | 16 / 4 | Disabled | Disabled | 7,470 | `2fe86d52ab736e4aea1baa5af766af033fdd71cbe16a17e9e66ddc48415a1ab7` |
| `main10-ctu32-tu16` | 32 / 16 | Default lists enabled | Disabled | 6,940 | `fb810c44931ead2b00c9a21e8e71b4f65b21239c5f49c30924b233aab018c6bc` |
| `main10-bypass` | 64 / 32 | Default lists enabled | Enabled | 16,842 | `1e3fba4822359e126953b5d409dd768af2560f939ae22dc9caae30ffceecc375` |
| `main10-bypass-changed` | 64 / 32 | Default lists enabled | Enabled | 16,847 | `34f99b0a9e7f1e79e394f412d2b7705c1667b95a7e798c5a62d6e5aaff14cc06` |

Transform skip is enabled in every PPS. The registered transform sizes and tool flags establish signalling, not which CABAC modes were used. Default scaling lists have no explicit SPS or PPS scaling-list data payload. No custom scaling-list payload, tile, WPP or dependent-segment coverage is claimed. Lossless bypass output additionally equals the synthetic input sample formula exactly. Establishing effective per-block syntax coverage remains reconstruction work.

The binary reports `Unknown option: pcm.` for `pcm=1` and `Unknown option: ipcm.` for `ipcm=1`, even though FFmpeg exits zero. Both attempts are retained as unsupported parameter probes, not passing PCM fixtures. Every registered SPS has `pcm_enabled_flag=0`. No supported PCM-generation control was found through these binary probes; they do not prove the absence of every possible binary control. PCM coverage must come from separately admitted published data or a separately authorized recipe. The verifier stops if these probe results change rather than silently adding an arm.

## Hash scopes and controls

H.265 clauses D.2.20 and D.3.20 define suffix SEI payload type 132 and MD5 `hash_type=0`. The encoder option `hash=1` selects that MD5 mode; its option value is not the bitstream's `hash_type`. The verifier parses suffix SEI framing from the specification and compares each Y, Cb and Cr digest to that component's reconstructed coded-picture bytes, low byte first above eight bits. Both pictures in every case have all three embedded component hashes. Any SEI/oracle disagreement voids preparation; there is no majority vote.

`raw.pictures[].coded_component_md5` and `decoded_picture_hash_sei_md5` have the coded-component scope. `raw.pictures[].visible_yuv_md5` hashes the concatenated Y, Cb and Cr bytes of one visible picture. `raw.whole_visible_yuv_md5` hashes the complete two-picture YUV file. These are different scopes even though coded and visible geometry coincide here. `raw.sha256` and `bitstream_sha256` bind the complete raw and compressed files. The committed per-component `sample_proof` records minima, maxima, samples above 255 and samples with nonzero low two bits, and rejects out-of-range values or reduced-depth output.

The changed-content bypass case increments every generated sample by one modulo 1024 before encoding. Both reconstructed pictures and every component MD5 change. Crossing either stream's SEI with the other stream's raw bytes must fail on component MD5 disagreement. Flipping a reconstructed byte or an embedded MD5 byte must also fail. The unit tests additionally reject changed registered hashes, missing or duplicate SEI, wrong hash modes, malformed framing, wrong geometry and packing, reduced depth, byte-order changes, unsupported options, resource exhaustion and credential inheritance.

In an evidence-source table, label these references **synthetic**, with counts from the fixed two-picture recipe, independently checked binary output and native headers. Their hashes are encoder SEI plus matching pinned-binary reconstruction, not published H.265.1 output MD5s. Passing this preparation does not resolve any separate published-data disagreement or establish a native decoder gate.
