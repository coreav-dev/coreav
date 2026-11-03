# Public text

Pull request title, body, and issue-reference text.

| Rule | Enforced by |
|---|---|
| A pull request's title and its commit subjects are Conventional Commits, `type(scope): summary` with the template's types (merge commits excepted); the title's summary is at most 50 characters and the title under 72; commit subjects are at most 72 characters | `scripts/pr_shape_check.py` (`pr-text.yml`) |
| No Korean text, run identifiers or spend details in a pull request's text | `scripts/pr_text_check.py` (`pr-text.yml`; the private list only on the agent's machine before opening) |
| No file names an issue or pull request by a bare number; one of this repository is written `coreav-dev/coreav#N`, any other is described | `scripts/issue_refs_check.py` (`deny.yml`) fails on `#N`, `issue N`, `PR #N`, `prN` and `pull request N` in every tracked text file, and on `issue-N` and `prN` in paths. A number alone in a file name or a build label passes it, so that is review judgment |
| A pull request's body opens each section the template keeps | `scripts/pr_shape_check.py` (`pr-text.yml`): a body without a level-2 heading for one of the template's required sections fails and names the section, read from markdown-it-py's tokens, pinned in `scripts/pr-text-requirements.txt`. Its test reads `.github/pull_request_template.md` and fails when the required and optional lists no longer hold its level-2 headings |
| A pull request's body has no hard-wrapped prose lines | Not enforced: judgment. Review judges it: to a parser a line break inside a paragraph and a line break between a tool's inline HTML are the same text, so a script cannot tell prose it should reject from markup it should keep |
| One outcome per pull request | Not enforced: judgment |
