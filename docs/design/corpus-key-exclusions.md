# Corpus key-row comparison and audit tools

`tools/compat-corpus/corpus_kd.py` implements the comparator and audit
mechanism of the [round 9 plan](../measurements/compat-ffprobe/round9-plan.md).
It does not run a measurement as part of its tests. Its production registry
is empty: tests use synthetic records and bytes, not new corpus exceptions.
Eligible records need their own exact-pair adapters after the records land.

## Cutoff and activation

The `freeze` command takes the full SHA of the authoritative main commit
introducing the corpus exclusion amendment. It refuses a commit outside
`origin/main`, a commit without the amendment, or a later commit. It reads
each implemented record at that cutoff and requires its complete contents
to match. The inventory retains the record, specification citation, digest
and implemented key scopes, plus the shared oracle pin's digest.

```sh
python3 tools/compat-corpus/corpus_kd.py freeze \
  --plan-commit "$PLAN_COMMIT" --out inventory.json
```

Run inputs cannot supply predicates or accepted value pairs. Only the
committed registry can do that. A record absent from the cutoff or a
changed record cannot activate. Missing or unreadable byte evidence covers
nothing; an unexpected reader error aborts rather than reducing coverage.

## Exact occurrence coverage

The legacy comparator and the new path share `run.key_pairs`. Section
arrays retain their indices and packets retain listing order. MPEG-TS
adapters also require the same PID at the existing stream position, and
the same stream index and PID for a packet pair. They never search for a
matching PID or reorder either listing. A stream-order difference remains
a difference even when both listings contain the same set of PIDs.

Each adapter returns the exact expected oracle and candidate values and
whether the candidate field is present, established independently from the
media bytes. Both observed values must match that pair. A broad stream
classification predicate cannot certify unrelated fields or packets.

The accumulator visits every reference-listed leaf, including differences
after the first saved example. A file/key row is excluded only when it
differs and every differing occurrence is validated. Raw results stay
unchanged. Existing `KD-STRICT` coverage remains separate and never becomes
a new exclusion. The default `run.py` modes, R1 and R2 are unchanged.

## Mutation replay

The audit replays that same occurrence iterator against candidate-only
mutations. It changes section values, stream identities and stream counts,
and packet values and counts at the first, middle and last packet of each
stream. Timing mutations also move claimed evidence with the value.
Tests change independent byte evidence and exercise agreeing and
nonmatching captures as well as covered ones.

A new or worsened raw difference must be uncovered at that exact path and
value. An unrelated caught difference does not excuse a covered one.
Coverage that activates on an existing uncovered difference also fails.
This detects an already-wrong value changed to another wrong value even
when aggregate mismatch counts stay constant. An audit failure makes the
report nonpassing and is not permission to change an exclusion or rerun.

The raw comparison's blind spots are reported, not credited as catches:
candidate-only leaves and trailing packets are not reference-listed,
repeated compact side-data keys overwrite earlier values, and missing
candidate leaves compare like null in the historical raw method. The
adapter's pair check still distinguishes absence from an explicit null.

## Capture and reporting

`compare` takes a private JSON list of the fixed selection. Each row has
`alias` (`f` followed by six digits), `path`, `sha256`, `container`
(`isobmff`, `matroska` or `mpegts`) and a Boolean `holdout`. Duplicate aliases
or hashes are refused. Every media checksum is verified before capture.
The shared pin is verified before either listing is captured.

```sh
python3 tools/compat-corpus/corpus_kd.py compare \
  --inventory inventory.json --files files.json \
  --coreav "$COREAV" --oracle-dir "$ORACLE_BIN" --out "$OUTPUT"
```

The output directory must not exist. Its `private/` directory retains the
selection, captures, exit statuses, raw rows, examples and audit details.
`report.json` contains only opaque file aliases, counts, inventory and
binary/selection digests. It retains the expected population and completed,
failed and unprocessed outcomes after an infrastructure abort. Partial
populations, missing audits, fallback activity and audit failures cannot
pass. The registered arm needs all 1,315 inputs and all three containers;
the manifest's identity must still be checked against the frozen selection
before the run. A matching file count alone does not prove selection.

Per key, the report separates direct agreement, existing covered
agreement, new exclusions and uncovered disagreement. The gated numerator
is direct plus existing covered agreement; the denominator removes only
new exclusions. Empty adjusted denominators are unmeasured. Raw agreement
and raw eligibility counts are retained. Common tag keys and the pooled
rare-tag diagnostic remain visible apart from the non-tag headline counts.
The overall per-key rates judge the bar; container and development/held-out
tables add no bar. Held-out flags do not assert that historical development
never used those files.

Excluded rows are published separately, once per file/key, with overlapping
KD attribution and counts per KD, key, container and development/held-out
part. No media hashes, paths or captured values enter that public report.
The 98% threshold and existing selections are not changed. This tool does
not alter fixtures, B4 timing or historical results.
