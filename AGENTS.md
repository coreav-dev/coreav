# Rules for agents and contributors, and what enforces each

Read `CONTRIBUTING.md` and `.github/pull_request_template.md` first; design notes for each area are in `docs/design/`. This file pairs every rule with what enforces it, so a rule that nothing enforces is visible.

When a review corrects a mistake, fix it and add its rule here. If the rule is already here and nothing enforces it, that is a repeat: in the same change, enforce it at the highest level that works (one owner or one way to do it, then a type, then a check whose error names the fix, then a test) and say why a higher level did not. Drop a row once its mistake cannot happen.

| Rule | Enforced by |
|---|---|
| No `unwrap`, `expect`, `panic!`, `todo!`, `unimplemented!` or slice indexing in production code | Workspace lints in `Cargo.toml`, `cargo clippy --workspace --all-targets -- -D warnings` (`ci.yml`) |
| No `unsafe` outside the crates `docs/unsafe-sites.md` names; every `unsafe` site has a row there | `unsafe_code = "forbid"` (`Cargo.toml`); `scripts/unsafe_sites_check.py` (`ci.yml`) |
| Every commit carries `Signed-off-by` matching its author (`git commit -s`) | DCO step of `deny.yml` |
| Dependencies keep to the license policy | `cargo deny check` and the fuzz projects' license check (`deny.yml`) |
| Default builds link no still-patented codec | `deny.yml` |
| A pull request's title is `type(scope): summary` with the template's types, a summary of at most 50 characters and the whole under 72; commit subjects are at most 72 characters; no `Co-Authored-By` or "Generated with" line | `scripts/pr_shape_check.py` (`pr-text.yml`) |
| No Korean text, run identifiers or spend details in a pull request's text | `scripts/pr_text_check.py` (`pr-text.yml`; the private list only on the agent's machine before opening) |
| Every action is pinned to a full commit sha | `scripts/workflow_check.py` (`deny.yml`) |
| The required-check table in `docs/design/ci-checks.md` and the workflows agree | `scripts/workflow_check.py` (`deny.yml`) |
| No change makes a guarded command run more instructions without a stated cost | `bench-guard.yml`: a pull request against its base, `main` against `bench/guard/baseline.json` |
| An H.264 speed round's bars are judged on every stream of its set | `scripts/h264_parity.py` records requested and failed streams; `scripts/h264_parity_bars.py` refuses incomplete reports |
| A new round adds no copy of an analyzer under `docs/measurements` | `scripts/tests/test_measurement_analyzers.py` (`deny.yml`) |
| The numbers of `docs/measurements/compat-ffprobe.md` are those of its `b3a-round*.json` | `tools/compat-corpus/record.py --check` (`deny.yml`) |
| Which keyframe a thumbnail may give is decided in one place | `select::offered` in `coreav-thumbnail`, and the agreement test in `mkv.rs` |
| Thresholds are committed before the run they judge | Not enforced: judgment (the commit before the run is the evidence) |
| One outcome per pull request; the template's sections; no hard-wrapped body lines | Not enforced: judgment |
| Design notes change in the same pull request as the design | Not enforced: judgment |
| Code is written from specifications, never from other implementations' source | The code-similarity check for `coreav-h264`, `coreav-kernels-h264` and `coreav-opus` (`docs/design/ci-checks.md`, "Before opening a pull request"); elsewhere review |
