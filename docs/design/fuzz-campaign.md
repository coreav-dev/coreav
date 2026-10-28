# Fuzz campaigns: the 24 CPU-hour release gate

Status: Accepted. Scope: the fuzzing release gate: before launch 1, every fuzz target runs in campaigns totalling at least 24 CPU-hours with no
crash, hang or out-of-memory report; any finding resets the count..
Tool: `tools/fuzz-campaign/`.

## What runs

- **Targets:** every `[[bin]]` target of every fuzz project under
  `crates/*/fuzz`, as `scripts/fuzz_plan.py --all` lists them.
  The first campaign had 17 targets; the release commit has 63, which the
  gate's plan and evidence list one by one.

  A target added later joins the next campaign with the same share.
- **Time:** shards of 30 minutes, the same number for every target; none
  is weighted by size, because the gate is per target as much as in total.
  The first campaign ran 5 per target, 2.5 CPU-hours each, 42.5 in total
  for 17 targets. The gate run on the release commit runs **2 per target,
  1 CPU-hour each, 63 CPU-hours for its 63 targets**: the total the plan
  set for the gate when it had 25 targets (62.5), well above the 24 the
  gate requires, so that a short shard does not drop the total below the
  gate (a *lost* shard is run again, below), and every target is fuzzed
  for an hour in two independent shards. Rejected: five shards each (315 shards, 157.5
  CPU-hours: more depth than the gate asks for); one shard per target (31.5
  CPU-hours, and a lost shard would leave a target unfuzzed). Fixed before
  the run.
- **CPU-hour:** one libFuzzer process with a core to itself for an hour,
  **counted by the CPU time the kernel charged it** (`ru_utime +
  ru_stime` of the finished child). Startup and seed loading count;
  waiting for a container or for a CPU does not. A container with
  `cpu=1.0` reserves a core, so there the CPU time and the wall clock
  agree. On a machine that runs more than one shard, the CPU time leaves
  out the time a shard waited while something else ran, but not the
  sharing of one core's two hardware threads: the kernel charges each
  thread in full while it runs at about half speed. So the local launcher
  runs at most one shard per physical core among the CPUs it may use
  (counted from their topology, so a CPU set holding both threads of a
  core counts it once) and refuses more, and a
  campaign has the machine to itself while it runs; the evidence records
  the shards run at once (`jobs`).
- **Shard:** one libFuzzer process, its own `-seed`, starting from the
  target's seed corpus — a container of its own on a cloud host, a process of
  its own on a machine. Shards do not share their corpora while they run
  (a simpler, independent sample of the input space per shard; a finding
  in any shard counts).
- **Where a campaign runs:** the release gate's 126 shards want the cloud host's
  containers; a campaign that fits one machine runs there
  (`local_fuzz.py`), which costs nothing. Both launchers write the
  same result rows and the same evidence; `--jobs` defaults to the
  physical cores of the CPUs it may use, so that a shard has one.
- **Which targets:** every target, for the gate. A campaign for one bar
  runs the targets that bar names (`campaign.py prepare --targets`,
  qualified by crate where a name belongs to more than one project), and
  its evidence says so in the plan.
- **Waves:** at most 40 containers run at once (`max_containers` on the
  cloud function), so a plan of more shards runs in waves of at most 40,
  as many as needed and as full as the division allows (every wave but
  the last of the same size),
  each started when the one before has returned: the gate run's 126 shards
  in 4 waves of 32, 32, 32 and 30. Shards are ordered by shard number,
  then target, so every target's first shard runs in the first two waves
  and its second in waves 2 to 4: a run cut short after two waves still
  has a shard of every target. The launcher, not the cloud host's queue,
  holds the shards that wait, so it can stop starting them at its deadline
  (below). The account's limit of 100 containers is shared with other
  work; if that work holds more than 65, some of a wave's shards wait
  for a container, which the per-wave margin below absorbs only in part.
- **libFuzzer options:** `-max_total_time=1800 -timeout=10
  -rss_limit_mb=2048 -max_len=1048576 -print_final_stats=1`, with the
  container's memory at 4 GiB. A hang is an input that runs over 10 s; an
  out-of-memory report is RSS over 2 GiB. Inputs up to 1 MiB.
- **AddressSanitizer options:** `ASAN_OPTIONS=quarantine_size_mb=64` for
  every run of a target: the shards (the plan records the value and both
  launchers set it), the corpus merges, and CI's fuzz job. Below,
  "AddressSanitizer options".
- **Build:** `cargo +nightly fuzz build -O` of each project on the commit
  under test, as CI builds them (AddressSanitizer, debug assertions on,
  `x86_64-unknown-linux-gnu`). The binaries are built once, recorded by
  SHA-256, and shipped to every shard; the AddressSanitizer runtime is
  linked in, so the container needs no toolchain.

## AddressSanitizer options

AddressSanitizer keeps freed blocks in a quarantine (256 MiB by default on
64-bit Linux) before they can be reused, so that a use after free reads
poisoned memory. Each size class of its allocator grows to hold its share
of the quarantine, and a class's memory goes back to the system only
in whole pages. A target that frees much memory per input, in blocks of
many sizes, therefore reaches an RSS that reflects the sanitizer's
bookkeeping rather than what the target holds. The out-of-memory limit is
meant to catch the second.

The gate on 82b02d3b found this. Both shards of `coreav-mpegts`
`packets` reached the 2 GiB limit, one before its end (peak 2,640 MB after
77,345 runs, with an empty artifact) and one at its exit. Other targets
peaked at 1.26 GB or less. At the out-of-memory report the live heap
was 461 MB, 91% of it libFuzzer's corpus. The target itself holds nothing
between inputs and at most 0.5 MB during one: that was measured with a
counting allocator over the shard's 1,063-unit corpus (it frees all of
about 750 KB per input, in blocks of up to 128 KB, a quarter of them
reallocations), and 210,000 mutated runs found no input above 64 MiB.
Replaying that corpus 20 times on a gate container, without mutation, so
libFuzzer never purges the allocator:

| Target, quarantine | Peak RSS |
|---|---|
| `packets`, 256 MiB (default) | 1,484 MB |
| `packets`, 64 MiB (measured after the choice) | 1,109 MB |
| `packets`, 32 MiB | 734 MB |
| `packets`, 256 MiB, never released to the system | 2,280 MB |
| `probe`, 256 MiB (default) | 1,809 MB |

`probe`, which allocates half as often, peaks higher in replay: the RSS
follows the quarantine, not the count of allocations. While fuzzing,
libFuzzer purges the allocator once RSS passes half the limit, which holds
`probe` near 1 GB but not `packets`.

64 MiB is the choice. A use after free of memory freed in the last
several dozen inputs is still caught (`packets` frees under 1 MB an
input). In the replay it lowers the peak by a quarter, and 32 MiB lowers
it by half; while fuzzing, libFuzzer's purges take back most of the rest.
Rejected:
- a higher `-rss_limit_mb`: the limit would hide a real growth of the
  same size;
- a smaller quarantine for this target only: a list of exceptions that
  grows with each target;
- `__asan_default_options` in the harness: an `unsafe` attribute in a fuzz
  project for what is a campaign setting;
- changes to the library: it holds no memory across inputs, and `probe`'s
  replay shows that fewer allocations do not lower the peak.

Pre-registered check, fixed before its run: one 30-minute shard of
`coreav-mpegts` `packets` on a gate container (1 core, 4 GiB) with the
gate's libFuzzer options, seed 1001, the 64 MiB quarantine, and the gate
pack's corpus of that target as its seeds. It passes when it ends with no
finding and a `peak_rss_mb` below 1,536 (75% of the limit). The same
shard with the default quarantine peaked at 1,773 MB, and printed an
out-of-memory report of 2,175 MB at its exit.

Result: passed. The shard ran 285,743 inputs and ended with exit status 0,
no artifact and a `peak_rss_mb` of 1,387, and printed no out-of-memory
report at its exit. The same options on a local machine for 30 minutes:
216,554 inputs, peak 645 MB, no finding.

## Seeds

Each target starts from the same seed set in every shard, built by
`tools/fuzz-campaign/campaign.py prepare` from:

- the generated fixtures: `scripts/gen-fixtures.sh` (MP4, MOV, M4A and the
  WebM fixture) and `scripts/gen-fixtures-mkv.sh` (Matroska and WebM);
- the hostile-input suite: `tools/hostile/generate.py` (parts 1
  to 3; part 4 holds third-party reproducers that are never committed or
  shipped).

A campaign does not read `crates/*/fuzz/regressions/<target>`, which holds
the inputs of past findings and of reported bugs, kept in git unlike the
corpus: CI runs those on every change (`ci.yml`), and the hostile suite
above carries the ones that are also hostile-suite files into every
campaign.

ISO base media targets get the MP4, MOV and M4A fixtures and the hostile
suite; Matroska targets get the Matroska and WebM fixtures; the policy
check and the WebAssembly operations, which read either container, get
both. The `coreav-text` `sample` target, whose input is one 3GPP timed
text sample rather than a file, gets a small built-in set: empty, plain,
multi-line and markup text, `styl` and `hlit` modifiers, and UTF-16 text.
The HLS index parser (`hls_index`), whose input is an index rather than a
media file, gets the index `coreav hls index` builds for each ISO base media
fixture the packager accepts (62 of 87); the packager (`hls`) gets each file
with its two option bytes (the first audio and 2 s segments, and no audio
and 1 s segments).
Files over 1 MiB are left out (libFuzzer would cut them anyway). Targets
whose input starts with option bytes get each file once with those bytes
zero (the defaults: the first stream, every stream, drop unsupported),
except `hls`, as above;
`mkv_remux` also gets each file with its fixed-header bit set, `check`
each file under its first two built-in policies, `sanitize` each file
keeping the default metadata and keeping none, `split` each file cut
every half second and at 0.2 and 0.5 s, and `ops` each file under
probe, audio extraction and clip, and under probe with an injected read
failure. The seed set of each target is recorded by count and SHA-256 in
the plan.

A campaign also carries the corpus of the one before it forward. Each
shard returns the units it added (the smallest first, up to 48 MiB), and
`prepare --corpus` adds them, or an archived minimized corpus, and any
finding inputs to the next campaign's seeds, minimized together with the seeds by libFuzzer's `-merge=1`: only
units that add coverage join, and every seed above stays. The count carried
over is recorded per target.

## The gate run

On the release commit, one command runs every step.

It checks the commit out in a fresh worktree, generates the seeds, adds the
corpus carried from the bug-finding pass (`fuzz/corpus-0bca9b9/fuzz-corpus.tar.zst` in the private corpus bucket, its SHA-256 checked), builds
every target, runs the plan above on a cloud host,
each container under a 45-minute hard timeout and the launch under 4
hours of wall time, and writes the results, `campaign.py report`'s
evidence and `campaign.py pack`'s minimized corpus archive into its work
directory. It reads R2 from the operator's machine and uses no cloud secret. The results, the evidence and the measurement record the commit
beside what identifies its source in a repository with another history
(the public repository starts from a commit of its own, so a commit of
this one does not exist there): its tree hash
(`git rev-parse SHA^{tree}`), the hash of its `crates` subtree (every
crate and fuzz project, with their lock files) and the SHA-256 of the root
`Cargo.toml` and `Cargo.lock`, and the nightly `rustc` and `cargo fuzz`
versions that built the binaries. Later documentation changes, the
measurement's own pull request included, change the tree but not the
others, so a release's code can be shown identical to the fuzzed code.
`gate.sh SHA --rehearsal` runs the same steps with one
60-second shard per target, 10-minute containers and an
hour of wall time (two waves for 63 targets, and an image upload that
grows with the targets), to check the tooling on a commit before the gate.

The wall time: the 4 waves take about 2.3 hours (35 minutes each, 30 of
fuzzing plus startup and seed loading) and at most 3.27 hours (each wave
49 minutes: every container at its 2-minute startup limit and its
45-minute timeout, plus 2 minutes for scheduling).
Before them, building and uploading the image (the binaries and seeds,
about 2 GB for 25 targets, more for 63) took about 10 minutes in the
rehearsals with 25 targets. The 4 hours leave
about half an hour of margin at the worst (3.27 hours of waves and 10
minutes of upload, against the deadline below at 3.92 hours), so the
limit stays; the launcher refuses to start if the upload took more than
39 minutes. `gate.sh` passes the launcher
a deadline 5 minutes before the limit: it starts nothing when the waves,
each at its timeout, could end after it, and starts no later wave that
could. The fuzz build runs before the launch and is not counted.

The results go to:
- `docs/measurements/fuzz-campaign-2.md` and `.json`: the gate's evidence
  (the report, with the commit, binaries and seed sets, per-shard
  statistics and findings), in the pull request that
  records the run;
- The private corpus bucket, `fuzz/gate-SHORTSHA/`: `results.json` (with the
  findings' inputs and log tails, kept out of the repository) and
  `fuzz-corpus.tar.zst`, the corpus later campaigns and OSS-Fuzz seed
  from; the archive's size and SHA-256 are recorded in the measurement.
  The upload is a separate, manual step with write credentials; it must
  not overwrite an object, and it checks each object's SHA-256.

## Findings

A finding is any shard that ends with a crash, a timeout, an out-of-memory
report or a leak (a libFuzzer artifact other than a slow unit), or a
libFuzzer exit status other than 0. Then:

1. the input is reproduced locally and minimized;
2. the defect is fixed, and the input becomes a regression test in the
   crate that failed;
3. the count starts again: a new campaign of the full plan on the fixed commit.

After the first release, a finding in released code is handled as a
reported vulnerability (`SECURITY.md`): the fix is made in a private GitHub
security advisory and its private fork and disclosed once the fixed
release ships. Until then the input, the stack trace and the log tail stay
in the advisory, not in a public issue or a pull request's description or
comments; the input becomes a regression test in the change the advisory
publishes. A campaign's results stay off GitHub (the cloud host and the private
bucket) until then, and its evidence pull request names the finding only
by the advisory. CI's short fuzz runs on each change (`ci.yml`) log in
public and keep a crash's input as a run artifact: when one crashes in
released code, the run's logs and artifacts are deleted once the advisory
holds them.

A shard that fails for an infrastructure reason (a preempted container, a
lost connection) is not a finding: it is run again, and only completed
shards count. **The gate asks that every planned shard completed**, not
that every target has one: a shard with no result may have been the one
to find something, and a campaign with a hole in it cannot say otherwise.
Running the lost shards again is therefore how a campaign is finished,
not a reason to run the whole plan again — `report --results` takes the
re-run's file beside the first, and the later result wins where both
hold the same shard, except over a finding: a fuzzer's run does not
repeat, so a clean re-run does not undo a crash or hang, and the shard
that found it keeps its row. Every results file must name the plan's
commit, or `report` refuses it. The launcher writes a row for every planned shard,
so a hole is visible rather than silent.

## Re-runs and the gate

Campaigns before the release commit are bug-finding passes: they run the
same plan on the code as it stands, so findings surface while the parsers
still change, and their corpora seed the gate run. The gate is judged on
the release commit: it is a condition of a release, not of the public
repository's first commit. A campaign counts for a later
commit when no fuzzed crate (or a workspace crate it depends on) changed
materially in between; a material change to a parser means a new campaign.
Each campaign's evidence names its commit.

## Evidence

`docs/measurements/fuzz-campaign-N.md` and `.json` per campaign: the commit,
its tree and `crates` subtree hashes and the SHA-256 of the root
`Cargo.toml` and `Cargo.lock` (from the gate tooling on),
the binaries' and seed sets' SHA-256, and per shard the target, the seconds
fuzzed, the executions, the coverage (`cov` edges and `ft` features at the
end), the corpus size, the peak RSS and the outcome; the total CPU-hours
and the findings.

## Alternatives considered

- **Leaving the fan-out to the cloud host's queue:** `max_containers` alone would
  run 126 shards 40 at a time, but the launcher could not see which shards
  wait, nor stop starting them at the deadline; explicit waves make both
  visible in the launcher's output.
- **GitHub Actions runners:** free minutes, but 6-hour job limits and
  shared runners; 24 CPU-hours would take many jobs and block the queue for
  other work.
- **One long process per target:** fewer startups, but a lost container
  loses a whole target's share, and the 63 containers still run in waves
  of at most 40.
- **Merging shard corpora between rounds:** more coverage per hour, but the
  gate asks for hours without findings, not for coverage, and a single
  round keeps the evidence simple. Coverage is recorded so that later
  campaigns can compare.
- **Building in the container:** needs a nightly toolchain in the image and
  repeats the build per shard or per image; shipping one set of binaries
  also makes every shard run identical code.
