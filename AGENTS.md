# Rules for agents and contributors, and what enforces each

Read `CONTRIBUTING.md` and `.github/pull_request_template.md` first; design notes for each area are in `docs/design/`. This file pairs every rule with what enforces it, so a rule that nothing enforces is visible.

When a review corrects a mistake, fix it and add its rule here. If the rule is already here and nothing enforces it, that is a repeat: in the same change, enforce it at the highest level that works (one owner or one way to do it, then a type, then a check whose error names the fix, then a test) and say why a higher level did not. Drop a row once its mistake cannot happen.

| Rule | Enforced by |
|---|---|
| No `unwrap`, `expect`, `panic!`, `todo!`, `unimplemented!` or slice indexing in production code | Workspace lints in `Cargo.toml`, `cargo clippy --workspace --all-targets -- -D warnings` (`ci.yml`) |
| No `unsafe` outside the files `docs/unsafe-sites.md` lists; the change that adds or changes an `unsafe` site adds or updates its row there | `unsafe_code = "forbid"` (`Cargo.toml`); `scripts/unsafe_sites_check.py` (`ci.yml`) fails on a file with `unsafe` and no row. It checks files, not sites: a new site in a listed file passes it, so its row is review judgment |
| Every commit a pull request adds carries a `Signed-off-by` with its author's email (`git commit -s`) | `scripts/dco_check.py` (`deny.yml`): the name may differ and the email's case is ignored; commits already on `main` are skipped |
| Dependencies keep to the license policy | `cargo deny check` and the fuzz projects' license check (`deny.yml`) |
| Default builds link no still-patented codec | `deny.yml` |
| A pull request's title and its commit subjects are Conventional Commits, `type(scope): summary` with the template's types (merge commits excepted); the title's summary is at most 50 characters and the title under 72; commit subjects are at most 72 characters | `scripts/pr_shape_check.py` (`pr-text.yml`) |
| No Korean text, run identifiers or spend details in a pull request's text | `scripts/pr_text_check.py` (`pr-text.yml`; the private list only on the agent's machine before opening) |
| Every action is pinned to a full commit sha | `scripts/workflow_check.py` (`deny.yml`) |
| The oracle FFmpeg and the x264 commit of its libx264 are pinned once, in `scripts/oracle-ffmpeg.env`; scripts and workflows read them from there | `scripts/oracle_pin.py --check` (`deny.yml`) fails on an FFmpeg download URL, archive name, cache-key version or `VERSION` constant, an x264 commit, or a value of the pin, in any tracked file outside `docs/`, Markdown and `evidence/` or `results/` directories. Prose that names a version is review judgment |
| No file names an issue or pull request by a bare number; one of this repository is written `coreav-dev/coreav#N`, any other is described | `scripts/issue_refs_check.py` (`deny.yml`) fails on `#N`, `issue N`, `PR #N`, `prN` and `pull request N` in every tracked text file, and on `issue-N` and `prN` in paths. A number alone in a file name or a build label passes it, so that is review judgment |
| The required-check table in `docs/design/ci-checks.md` and the workflows agree | `scripts/workflow_check.py` (`deny.yml`) |
| Current comparisons use the shared FFmpeg pin; the historical training extractor stays isolated | `scripts/oracle_pin.py --verify-bin` before oracle use, including cache hits; `--check` restricts historical-extractor references to the training job and its measurement directory (`deny.yml`) |
| No change makes a guarded command run more instructions without a stated cost | `bench-guard` in `ci.yml` fails when a case counts more than the tolerance above its count (5%, `tolerance` in `bench/guard/baseline.json`): a pull request against its base, `main` against the baseline. A rise within the tolerance passes, and a change that moves the baseline only gets a warning: that its cost is stated is review judgment |
| An H.264 speed round's bars are judged on every stream of its set | `scripts/h264_parity.py` records requested and failed streams; `scripts/h264_parity_bars.py` refuses incomplete reports |
| A new round adds no copy of an analyzer under `docs/measurements` | `scripts/tests/test_measurement_analyzers.py` (`deny.yml`) |
| The numbers of `docs/measurements/compat-ffprobe.md` are those of its `b3a-round*.json` | `tools/compat-corpus/record.py --check` (`deny.yml`) |
| The documentation site's command reference shows each command's `--help` as the command prints it, and every command has a page | `crates/coreav-cli/tests/docs_site.rs` (`ci.yml`; `COREAV_BLESS_DOCS=1` rewrites the help text) |
| The documentation site's links into the repository and between pages resolve, without the `/docs` base path | `scripts/tests/test_docs_site.py` (`deny.yml`); Mintlify's link check (`docs-site.yml`, not required) |
| State kept per PID in `coreav-mpegts` is bounded: by a limit of `Limits`, or by the 8,192 PIDs a packet can name | `PidMap` (`crates/coreav-mpegts/src/pid_map.rs`): `PidMap::streams` and `PidMap::programs` take their limit from `Limits`, `PidMap::pids` the PID space; `disallowed-types` in `crates/coreav-mpegts/clippy.toml` refuses the standard maps elsewhere in the crate (clippy, `ci.yml`) |
| Which keyframe a thumbnail may give is decided in one place | `select::offered` in `coreav-thumbnail`, and the agreement test in `mkv.rs` |
| Missing FLAC or ALAC configurations, invalid FLAC block-size bounds, and unsupported ALAC cookie versions produce a malformed-configuration warning, not configured audio values | `a_malformed_lossless_configuration_is_refused` in `coreav-isobmff/tests/audio_layouts.rs`; the library parses these bytes before exposing configured values, and `coreav_isobmff::lossless_config` is the one parser of them (the writers read the sample description's values or call it) |
| Thresholds are committed before the run they judge | Not enforced: judgment (the commit before the run is the evidence) |
| One outcome per pull request; the template's sections; no hard-wrapped body lines | Not enforced: judgment |
| Design notes change in the same pull request as the design | Not enforced: judgment |
| Code is written from specifications, never from other implementations' source | The code-similarity check for `coreav-h264`, `coreav-kernels-h264` and `coreav-opus` (`docs/design/ci-checks.md`, "Before opening a pull request"); elsewhere review |
