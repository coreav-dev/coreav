# Packet-listing speed against libavformat

The tools of the MPEG-TS packet-listing speed bars (`docs/design/mpegts.md`, "Speed bars, round 2"): CoreAV and libavformat of FFmpeg 9.0.2 each list every packet of a file in process, and the round compares their times file by file on one core of a host held at a fixed frequency.

| File | What it is |
|---|---|
| `src/main.rs` | `ts-speed packets FILE`: CoreAV's side. The probe, then every access unit of every kept stream, with the library defaults; prints `{"ok","ns","count","bytes"}`. |
| `lav_ts.c` | `lav_ts packets FILE`: libavformat's side. Opens the file as `mpegts`, finds stream info and reads every packet through a 64 KiB `pread` buffer; refuses any library but the pinned LGPL build. |
| `build-lav.sh DEST` | Downloads the pinned LGPL shared build, checks its SHA-256 and builds `DEST/lav_ts` beside the libraries and `ffprobe`. |
| `scripts/ts_speed.py` | Times the arms on one host: warm-up, rotated rounds, a null arm, frequency samples. |
| `scripts/ts_speed_gcp.py` | Runs `ts_speed.py` on a VM of the bar's host class, with a spend cap, and writes the run's verdict. |
| `scripts/ts_speed_bars.py` | Judges a report against the bars, or against the level that decides whether to open the draw (`--gate`). |

A round, from the repository's root on Linux x86_64:

```sh
cargo build --locked --profile dist -p coreav-ts-speed -p coreav-cli
tools/ts-speed/build-lav.sh /tmp/lav
python3 -I scripts/ts_speed_gcp.py --out runs/r1 --coreav target/dist/ts-speed --lav-dir /tmp/lav \
    --coreav-cli target/dist/coreav --cap-usd 2 FILE...
python3 -I scripts/ts_speed_bars.py runs/r1/report.json
```

`ts_speed_gcp.py` needs `COREAV_GCP_SA_KEY` and `COREAV_GCP_PROJECT` (`scripts/gcp_vm_job.py`). `ts_speed.py` runs on any Linux host as well, for a look that is not a bar.
