# Untimed Opus capacity preflight

This tooling checks whether the complete Opus measurement path can finish on long, already-spent inputs before a future speed round is registered. It produces no speed result and registers no round. The [design note](../../docs/design/audio-decode.md#opus-capacity-prerequisite) defines the prerequisite.

## Inputs and tools

Use Linux on the fixed Intel Xeon Platinum 8581C host class, with SMT disabled. Run one owner at a time. Put temporary outputs on a dedicated tmpfs with more free space than the configured budget, and leave host RAM for the operating system and temporary files. The inherited per-process address-space limit does not prevent system-wide out-of-memory handling.

Install the pinned numerical dependency in a virtual environment:

```sh
python3 -m venv .venv
.venv/bin/pip install --require-hashes -r bench/audio-decode/requirements.txt
```

The input document has `scope: "spent"` and an `inputs` array. Each record contains `file` (relative to the media directory), `sha256` (the complete file hash), `class` (`mp4` or `matroska`), `channels` (one or two), and `packets` (observed Opus TOC counts, such as `{"celt/20": 45000}`). The dominant TOC mode determines its cell. A qualifying roster covers CELT, Hybrid and SILK in both containers; every input must decode at least 43,200,000 raw frames at 48 kHz. Input files stay private.

The tools document maps each role to its executable path: `base_abench`, `head_abench`, `base_coreav`, `head_coreav`, `lav`, `ffmpeg`, `ffprobe` and `opus_compare`. Use the shared pinned FFmpeg oracle. The two `abench` programs and `lav` are the measurement harnesses, not product commands. Their success summaries must provide actual work counters:

| Operation | Required output |
| --- | --- |
| `abench raw INPUT STREAM OUTPUT` | JSON `ok`, `packets`, `samples`; the per-packet raw file below |
| `abench lib INPUT STREAM` | JSON `ok`, `samples`, `bytes` |
| `abench decoder INPUT STREAM` | JSON `ok`, `samples` |
| `lav decode INPUT STREAM` | JSON `ok`, `samples`, `bytes`, `errors` |
| `lav decoder INPUT STREAM` | JSON `ok`, `samples`, `packets`, `errors` |

Raw identity files begin with a little-endian `u64` packet count. Each packet stores four little-endian `u64` values: packet index, decoded frames, channels and scalar count, followed by that many little-endian `f32` bit patterns. `scripts/opus_identity.py` validates both files completely, including boundaries and trailing bytes, then compares bits before integer conversion. Command identity separately compares the two complete s16 WAV outputs. Timing fields returned by existing harnesses are discarded.

The limits document contains exactly these positive integer fields:

```json
{
  "per_call_seconds": 900,
  "job_seconds": 3600,
  "concurrency": 1,
  "memory_bytes": 8589934592,
  "tmpfs_bytes": 4294967296
}
```

The per-call maximum is two hours and the whole-job maximum is six hours. Use a smaller operational deadline where possible. These are deadlines, not measurements. NumPy and child-process thread counts are set to one; FFmpeg audio inputs use the shared input arguments.

## Run and resume

First exercise the tooling on short spent fixtures with `--smoke`. That flag prevents qualification even if all operations finish. Use separate smoke input and checkpoint documents; it does not shorten the supplied media.

For the full pass, run from the checkout that supplies the judging code:

```sh
.venv/bin/python bench/audio-decode/run.py \
  --repo "$PWD" --inputs full-inputs.json --media "$MEDIA" \
  --tools tools.json --limits limits.json --work "$TMPFS" \
  --checkpoint checkpoint.json --out capacity.json
```

Repeat the same command with `--resume` after an interruption. The runner verifies all input, tool and judging bytes before reuse and around each operation. It needs no old temporary PCM files. A saved `running` operation becomes interrupted and is run again; completed operations are skipped, including terminal failures. A failed binary cannot silently turn into a pass on resume. Keep the failed receipt if a tooling correction requires a fresh binding.

Local checkpoints and output documents are atomically replaced. For preemptible hosts, optionally add `--r2-prefix PREFIX`; install `boto3` in the environment and supply `R2_ENDPOINT`, `R2_BUCKET`, `R2_ACCESS_KEY_ID` and `R2_SECRET_ACCESS_KEY` securely. The runner synchronously uploads each operation transition before continuing. A new host can use `--resume` with no local checkpoint to recover the same binding's remote checkpoint. Keep exact media and executable bytes in durable storage separately, and do not start another owner until the previous host is stopped. A storage failure stops the pass rather than leaving uncheckpointed work running.

The receipt contains its binding manifest. Save that manifest as `binding.json`, then independently validate the result:

```sh
.venv/bin/python bench/audio-decode/capacity.py validate \
  --receipt capacity.json --binding binding.json
```

The validator recomputes qualification; it does not trust `passed`. Only a complete full-length pass can satisfy the future registration line in the design note. A normal oracle disagreement is completed capacity work, not agreement approval. Errors and incomplete counters never qualify.

## Oracle failures

`oracle.py` maintains the independent presentation and agreement model used by the preflight. It uses the existing presentation reader through an injected executor, without replacing global subprocess functions. `judge` returns an `opus-oracle/1` envelope with either a normal agreement result or a closed error receipt.

An error identifies the leaf operation and tool, exit or signal status, and bounded diagnostic counts. For example, a reference decode failure remains `oracle.reference_decode` / `ffmpeg`, rather than becoming an undifferentiated outer worker failure. The comparator's explicit `Out of memory.` diagnostic at exit 1 becomes `out_of_memory`, never an ordinary disagreement; a signal alone cannot establish that cause. A recognized nonmonotonic-DTS diagnostic has a count; unknown lines have counts only. FFmpeg and ffprobe error diagnostics fail the operation even when the process exits zero. Subprocess groups are killed and reaped on timeout or cancellation, including descendants that outlive their parent.

No receipt contains command arguments, paths, stdout, stderr, PCM or arbitrary exception text. The same closed result schema governs runtime checkpoints and independent validation. Private subprocess buffers are bounded and excluded from their result representation.

## Synthetic checks

These tests use short generated data and child processes, not real media or a speed benchmark:

```sh
OPENBLAS_NUM_THREADS=1 .venv/bin/python -m unittest discover -s bench/audio-decode/tests
.venv/bin/python -m unittest scripts.tests.test_opus_capacity scripts.tests.test_opus_identity
.venv/bin/python bench/audio-decode/capacity.py check-registrations
```

The unfiltered required `deny` check installs the hash-pinned dependency, runs these regressions and checks every future registration. The historical round outcomes are not reclassified by this tooling.

## Retained validation evidence

The [full receipt](evidence/capacity.json) completed all 60 operations on six already-spent full-length inputs. Its [binding](evidence/binding.json) has canonical SHA-256 `c3962b386482b448471085b8844e42e0c6f6db2c10f5ec44afb59c08b8b83250`. These are capacity results, not a new speed round or decoder acceptance. No timing values are retained.

| Mode | Container | Raw decoded frames | Packets |
| --- | --- | ---: | ---: |
| CELT | MP4 | 81,932,640 | 170,693 |
| CELT | Matroska | 84,508,320 | 352,118 |
| Hybrid | MP4 | 54,895,200 | 114,365 |
| Hybrid | Matroska | 54,895,200 | 114,365 |
| SILK | MP4 | 64,111,680 | 133,566 |
| SILK | Matroska | 64,111,680 | 133,566 |

The host was an Intel Xeon Platinum 8581C with SMT off. The bound envelope is one concurrent operation, 8 GiB inherited address space per process, a 4 GiB temporary-output budget on tmpfs, a 900-second per-call deadline and a 3,600-second whole-pass deadline. Python was 3.13 and NumPy was 2.5.4. Both CoreAV arms use the existing fixed executables; the oracle and libavcodec baseline use the shared FFmpeg pin. The full receipt contains the executable and judging-file hashes and all sample and byte counts.

The [initial pilot](evidence/failed-pilot.json) completed 18 operations but failed both comparator operations under a 3 GiB address-space limit; it remains failed. Its original protocol errors do not establish a cause. A separate [diagnostic receipt](evidence/memory-error.json) reproduces an explicit comparator allocation error under that limit with the current classifier. Increasing the separately bound envelope to 8 GiB let the [second pilot](evidence/pilot.json) finish all 20 operations. Neither pilot can qualify because its purpose is smoke and it covers only two cells. This diagnostic does not identify the causes of the historical speed round's unretained worker errors.

The [resume proof](evidence/resume.json) records an actual signal interruption after one completed operation. The new process recovered from remote storage after losing its local checkpoint and temporary outputs, kept the completed result and finished the remaining work. A second remote-only replay left the completed receipt unchanged. The numerical and schema regressions replay these retained documents in CI.

Replay the full qualification without media or external executables:

```sh
.venv/bin/python bench/audio-decode/capacity.py validate \
  --receipt bench/audio-decode/evidence/capacity.json \
  --binding bench/audio-decode/evidence/binding.json
```
