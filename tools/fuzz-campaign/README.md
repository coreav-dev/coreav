# Fuzz campaigns

The 24 CPU-hour release gate. The plan,
the definitions and the rules for findings are in
[`docs/design/fuzz-campaign.md`](../../docs/design/fuzz-campaign.md).

Two launchers write the same rows: a cloud launcher kept with the
project's operations, a container per shard, for the gate's 126; and
`local_fuzz.py`, a process per physical core (and no more), for a campaign
that fits an otherwise idle machine. Both bill a shard by the CPU time the kernel charged it, so the
hours mean the same thing either way.

```sh
# Seeds: the fixtures and the hostile suite.
scripts/gen-fixtures.sh /tmp/fx
scripts/gen-fixtures-mkv.sh /tmp/mkvfx
python3 -I tools/hostile/generate.py /tmp/hostile /tmp/fx ffmpeg

# Build every target and the seed sets; writes /tmp/fuzz-campaign/plan.json.
python3 -I tools/fuzz-campaign/campaign.py prepare --out /tmp/fuzz-campaign \
  --iso-fixtures /tmp/fx --mkv-fixtures /tmp/mkvfx --hostile /tmp/hostile

# A later campaign seeds from an earlier one's corpus: the launcher's
# `corpus` directory, or an extracted corpus archive (its `corpus` and
# `findings` directories).
#   campaign.py prepare ... --corpus /tmp/corpus-0bca9b9/fuzz-corpus/corpus \
#     --corpus /tmp/corpus-0bca9b9/fuzz-corpus/findings

# Run every shard on this machine, one per physical core. `--targets` above
# plans one bar's targets rather than every target of the repository
# (`coreav-h264/decode` where a name belongs to more than one project).
# Detach it (setsid nohup ... &) when the shell may end before the shards:
# results are written as each shard finishes.
python3 -I tools/fuzz-campaign/local_fuzz.py --plan /tmp/fuzz-campaign/plan.json \
  --out /tmp/fuzz-campaign/results.json

# Totals per target, findings and the gate verdict. A shard the
# infrastructure lost is run again and its file given beside the first;
# the later result wins where both hold the same shard, unless the
# earlier one holds a finding.
python3 -I tools/fuzz-campaign/campaign.py report --plan /tmp/fuzz-campaign/plan.json \
  --results /tmp/fuzz-campaign/results.json [--results /tmp/fuzz-campaign/rerun.json] \
  --out docs/measurements/fuzz-campaign-N.json
```

A finding's artifacts come back base64-encoded in the results; reproduce
one with `ASAN_OPTIONS=quarantine_size_mb=64 cargo +nightly fuzz run TARGET
ARTIFACT -- -rss_limit_mb=2048 -timeout=10` in the target's fuzz project:
the shards' AddressSanitizer and libFuzzer options (the plan's
`asan_options` and `libfuzzer`), so that an out-of-memory report or a
timeout is judged under the limits that raised it.
