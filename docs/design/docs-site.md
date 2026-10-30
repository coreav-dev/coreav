# The documentation site (`docs-site/`)

Status: Accepted.

The user documentation is a [Mintlify](https://www.mintlify.com) site whose source is `docs-site/`. It is meant to be served at `coreav.dev/docs`; the `coreav-dev/site` repository serves the rest of `coreav.dev`. Its pages are written for people who use the command and the module: what each command does, how to install it, strict-safe mode, the build features, the safety properties. Each page links the design note or the measurement page that has the depth, and states nothing that those do not.

## Where it lives

`docs-site/` holds `docs.json` (the navigation, colours and logo), one MDX file per page, `snippets/help/` (the generated help text) and the logo and favicon, copied from the `coreav-dev/site` repository. Mintlify reads one directory of the repository, set as the project's documentation path.

- **Its own directory, not `docs/`.** Mintlify builds every Markdown file under its directory, listed in the navigation or not: a page left out of `docs.json` is still served at its path (tried with the CLI's preview). With `docs.json` in `docs/`, every design note, measurement page and known-divergence table would be published as a page of the site, and their raw `<` and `{` would have to parse as MDX. `docs/` stays what it is, notes for maintainers, and the site links into it.
- **`docs-site`, not `mintlify` or `site`.** The name says what the directory builds rather than which tool builds it, so it stays right if the tool changes, and it cannot be taken for the `coreav-dev/site` repository.

## Serving under `/docs`

The base path is a setting of the Mintlify project (the custom domain's "Host at" path), not of the repository: Mintlify builds the same source for any base path and prefixes every page's path with it. What the source must do is not assume one:

- A page links another page by its root-relative path without the base path (`/install`, never `/docs/install`, which would be served as `/docs/docs/install`). `scripts/tests/test_docs_site.py` fails on a link that starts with `/docs`.
- The logo links to `https://coreav.dev`, the site's home page, which the other repository serves.

## The command reference and the help text

Each page under `docs-site/commands/` shows its command's `--help` text from `docs-site/snippets/help/<command>.mdx`, which a test generates from the command itself. Nothing in the reference repeats an option by hand, so the two cannot drift: `crates/coreav-cli/tests/docs_site.rs` runs `coreav --help`, then `--help` of every command and subcommand it lists, and fails when a snippet differs, when a snippet is left over from a command that is gone, or when a command has no page, no entry in `docs.json` or no import of its snippet.

- `COREAV_BLESS_DOCS=1 cargo test -p coreav-cli --test docs_site` rewrites the snippets after a change to the help text; with `--features ffmpeg-fallback` it rewrites those of `thumbnail` and `transcode`, which exist only in builds with a decoder backend. The page's own prose is then read again, by hand, against the new text.
- `cargo test --workspace` runs the test in the default build, and `ci.yml` runs it once more with `--features ffmpeg-fallback` for the two decoder commands. In a default build the test also fails if either of those two appears, since the pages say they need a decoder backend.
- `coreav ffmpeg --help` is not native (strict-safe mode refuses it, the fallback runs `ffmpeg --help`), so `ffmpeg` has a page and no snippet.
- Rejected: generating whole pages from the help, which would leave no place for examples and for what the help does not say; and a script that a workflow runs on a built binary, where a test under `cargo test` runs wherever the command is built and tested already.

## Checks

| What | Where |
|---|---|
| Every command's help text and page | `crates/coreav-cli/tests/docs_site.rs`, in `ci.yml` |
| Every page in `docs.json` exists and every page is in it; links into the repository name a file and a heading on the same commit; links between pages resolve without the base path; every page of `docs/measurements` is on the measurements page; the rules of `scripts/pr_text_check.py`, and no speed claim against FFmpeg off its measurement page | `scripts/tests/test_docs_site.py`, in `deny.yml`, which runs on every pull request |
| Mintlify's strict build and its check of the links between pages and their anchors | `.github/workflows/docs-site.yml`, on changes to `docs-site/`; not a required check |

Links into the repository point at `main`, so a page and the file it links change in the same pull request: the test reads the files of the commit under test.

## Preview

The CLI's npm package runs the site locally with no account:

    cd docs-site
    npx --yes mint@4.2.1176 dev

`mint validate` is the strict build and `mint broken-links --check-anchors` the link check that the workflow runs; the version is the one `docs-site.yml` pins. Search needs an account (`mint login`) and stays off in a local preview.

## Publishing (the maintainer's steps)

None of these is in the repository:

1. Create the Mintlify project, connect it to `coreav-dev/coreav` through Mintlify's GitHub app, and set the documentation path to `/docs-site` (the monorepo setting).
2. Add `coreav.dev` as the custom domain with "Host at" on and `/docs` as the base path.
3. Route `coreav.dev/docs`, `/docs/*`, `/_mintlify/*` and `/mintlify-assets/*` to the project's `mintlify.site` host, with a Cloudflare Worker or the host's rewrites, since `coreav.dev` itself is served from the `coreav-dev/site` repository. Mintlify serves the site's styles and scripts from the two root paths, so a proxy of `/docs` alone gives pages without styles.
4. Once the site is live, point the README and `packages/cli/README.md` at it.
