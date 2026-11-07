# Public text

Pull request title, body, and issue-reference text.

| Rule | Enforced by |
|---|---|
| H.264 coefficient-level reference text publishes per-input Y, U and V changes and the five-input weighted mean, not per-input weighted deltas | Not enforced: review judgment. The individual plane limits and aggregate improvement govern the decision; a generic text checker cannot infer which numerical column is a weighted delta |
| Public reference-host records and profile descriptions use a neutral provider label, while supplier receipts stay private behind the allocation digest | `test_macos_host_profile.py` checks the same fixed neutral label in the registration owner, frozen profile and host record, and checks the hardware-only wording at each reference-profile introduction. A type cannot govern Markdown text; supplier disclosures elsewhere in prose remain private-term-check and review judgment |
| A pull request's title and its commit subjects are Conventional Commits, `type(scope): summary` with the template's types (merge commits excepted); the title's summary is at most 50 characters and the title under 72; commit subjects are at most 72 characters | `scripts/pr_shape_check.py` (`pr-text.yml`) |
| No Korean text, run identifiers or spend details in a pull request's text | `scripts/pr_text_check.py` (`pr-text.yml`; the private list only on the agent's machine before opening) |
| No file names an issue or pull request by a bare number; one of this repository is written `coreav-dev/coreav#N`, any other is described | `scripts/issue_refs_check.py` (`deny.yml`) fails on `#N`, `issue N`, `PR #N`, `prN` and `pull request N` in every tracked text file, and on `issue-N` and `prN` in paths. A number alone in a file name or a build label passes it, so that is review judgment |
| A pull request's body opens each section the template keeps | `scripts/pr_shape_check.py` (`pr-text.yml`): a body without a level-2 heading for one of the template's required sections fails and names the section, read from markdown-it-py's tokens, pinned in `scripts/pr-text-requirements.txt`. Its test reads `.github/pull_request_template.md` and fails when the required and optional lists no longer hold its level-2 headings |
| A pull request's body has no hard-wrapped prose lines | Not enforced: judgment. Review judges it: to a parser a line break inside a paragraph and a line break between a tool's inline HTML are the same text, so a script cannot tell prose it should reject from markup it should keep |
| One outcome per pull request | Not enforced: judgment |
| A maintainer-directed measurement-policy change attributes the decision to the maintainer instead of presenting it only as a technical conclusion | Not enforced: judgment. The PR's Background and Decisions must make the attribution; a text check cannot establish who decided a change |
