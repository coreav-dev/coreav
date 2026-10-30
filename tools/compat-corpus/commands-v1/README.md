# `commands-v1`: the command-coverage set

A fixed set of real `ffmpeg` and `ffprobe` invocations, used to measure how
much of the way people actually call those tools the compatibility command
answers, and to make sure it refuses the rest
([`docs/design/compat-cli.md`](../../../docs/design/compat-cli.md), where the
bars are).

Unlike the media corpora ([`docs/corpus.md`](../../../docs/corpus.md)), this one is **public and committed**:
it holds command lines, not files. [`commands.jsonl`](commands.jsonl) is
the set; nothing else is needed to reproduce a measurement.

| | |
|---|---|
| Commands | 1,147 distinct option sequences |
| From | 894 answers to 398 Stack Overflow questions, 1,428 command-and-answer pairs |
| Snapshot taken | 2026-10-20 |
| `ffprobe` | 143 commands: 93 native, 48 out of scope, 2 unusable |
| `ffmpeg` | 1,004 commands: 79 native, 897 out of scope, 28 unusable |

## Where it comes from

Two queries against the public Stack Exchange API, both `sort=votes`,
`order=desc`, on `stackoverflow`:

- the **whole `ffprobe` tag**, 449 questions, which is every question the
  tag had on the snapshot date;
- the **500 highest-voted questions of the `ffmpeg` tag**.

`tools/compat-corpus/collect.py` takes the snapshot, with every answer of
those questions. The snapshot holds whole post bodies and is not committed;
the question and answer ids of every command are, so the same commands can
be found again in the API or in the public Stack Exchange data dump.

The `ffmpeg` stratum is deliberately **not** enriched towards stream-copy
questions. It is the general population of highly-voted `ffmpeg` use, so
the share of it the compatibility command reaches is an honest number
rather than a flattering one.

## How a command is chosen

`tools/compat-corpus/extract.py` reads the `<code>` and `<pre>` blocks of
every answer scoring 1 or more — never prose, never a question, because an
answer's command is one the site's readers voted on — and applies the rules
in its docstring: join `\` and `^` continuations, drop a comment line,
drop a shell prompt and a leading `sudo`, split at `|`, `&&`, `;` and `&`
**outside quotes**, cut at a redirection, keep each part whose program is
`ffmpeg` or `ffprobe`, split the arguments as a POSIX shell would, and drop
what is not an invocation. 413 candidates were dropped that way on this
snapshot: a command substitution the shell would have expanded, a version
banner, a line of Java or JavaScript that names the tool, an empty argument
list.

**The file names are then replaced by placeholders** (`INPUT`, `INPUT2`,
`OUTPUT`): a coverage set is about which options are combined, every
measurement run substitutes a corpus file for each input anyway, and the
names in a post are examples — `input.mp4`, `$f`, somebody's disk. Only
the positions that are file names are replaced: the value of `-i` and of
the options that name a file, and FFmpeg's output, which is the last bare
argument. A bare token anywhere else belongs to an option the classifier
does not know and keeps its own text, so the record stays the invocation
it was. Two invocations that differ only in their file names are one
record, with both answers in `sources`.

Each record:

| Field | |
|---|---|
| `tool` | `ffmpeg` or `ffprobe` |
| `args` | the argument list, as a shell would pass it, with the file names replaced |
| `label`, `reason` | what `tools/compat-corpus/classify.py` makes of it, and the token that decided |
| `sources` | every answer the same option sequence was found in, highest score first: `question`, `answer`, `link`, `score`, `license` and which of the two queries found it |

## The labels

`classify.py` gives every command one of three labels, by a rule written
down before anything was measured:

- **`native`** — every option in it is in step 1's vocabulary, there is one
  input and one output, both containers CoreAV reads and writes, and the
  job is one of step 1's. The command must run it.
- **`out-of-scope`** — a real invocation of a job step 1 does not do: an
  encode, a filter, several inputs, a device, an image sequence, a
  container CoreAV does not read or write, an option not implemented. The
  command must refuse it with a clear error, never run it differently.
- **`unusable`** — the extraction cut the line short, or an option value
  was never expanded by a shell (`-ss $start`), so it is not a job that
  can be run. 30 of 1,147. Counted, listed and outside both bars.

The share that is native, of the usable commands: **66.0%** of `ffprobe`
(93 of 141) and **8.1%** of `ffmpeg` (79 of 976). The `ffmpeg` number is
low because most highly-voted `ffmpeg` questions are about encoding, which
step 1 does not do and later steps will.

## Where the commands came from, and under what terms

Every command was found in a Stack Overflow answer. Each record's
`sources` names every answer it was found in, with a link to it and the
Creative Commons licence that answer carries — 918 of the 1,428
command-and-answer pairs under CC BY-SA 4.0, 487 under 3.0 and 23 under
2.5. Keeping every source, rather than the first one found, is deliberate:
otherwise the credit, and the count above, would be an artefact of the
order the extraction happened to walk the answers in.

**What is committed is not an excerpt of those answers.** A record is a
sequence of command-line options with its file names replaced, derived by
the rules above and selected and ordered by this repository's own code: a
statement of which options people combine, which is a fact about a tool's
interface rather than anything an answer's author wrote expressively. So
`commands.jsonl` is under the repository's own terms, MIT and Apache-2.0,
like every other file here, and the project keeps its "MIT OR Apache-2.0
permanently" principle literally true.
The links and licences are recorded because provenance belongs in a
measurement input and because crediting the answers is the right thing to
do, not because a licence obliges this file to carry terms of its own.

That is a decision, taken explicitly rather than left open, and it is cheap to reverse: dropping `args` from the
committed file and keeping the rest leaves a manifest that still names
every source, and `extract.py` rebuilds the commands from a snapshot.

No post body, author name, profile link, e-mail address or home
directory is committed, and no file name in an input or output position:
the placeholders replace those, and `classify.scrub` makes any home
directory in a surviving value anonymous. What can survive is the value
of an option the classifier does not know, which in 21 of the 1,147
records is itself a file name, a host name or a URL an answer used — a
`watermark.png` in a `-vf` filter graph, an `rtmp://` destination of a
stream the answer was setting up. A test keeps the two positions that
are file names clean and keeps every record free of a home directory.

## Output names

`commands.jsonl` replaces every output name with `OUTPUT`, so a command
whose output no `-f` names does not say which container it writes.
[`outputs.json`](outputs.json) records it for every usable `ffmpeg`
command: the output of each invocation in the command's own answers that
normalizes to it, reduced to `OUTPUT` with the name's number pattern and
extension (`OUTPUT.mp4`, `OUTPUT%03d.jpg`), or the pipe it names (`-`,
`pipe:1`), and an empty string for an invocation with no output position.
`tools/compat-corpus/coverage_set.py` reads the answers again through the
public API and writes it. They were read on 2026-10-30, ten days after the
snapshot, and each of the 976 commands was found in at least one of its own
answers. Like the set, the file holds no post body and no file name. An
answer edited later can give another output, so this file, not a new
reading, is what the encode breadth measure uses
([`docs/design/compat-cli.md`](../../../docs/design/compat-cli.md), "The 0.1.0
codec coverage bars").

## Taking the snapshot again

    python3 -I tools/compat-corpus/collect.py --tag ffprobe --questions 500 \
        --out /tmp/snapshot-ffprobe.json
    python3 -I tools/compat-corpus/collect.py --tag ffmpeg --questions 500 \
        --out /tmp/snapshot-ffmpeg.json
    python3 -I tools/compat-corpus/extract.py \
        --snapshot /tmp/snapshot-ffprobe.json --snapshot /tmp/snapshot-ffmpeg.json \
        --out tools/compat-corpus/commands-v1/commands.jsonl

Scores change and answers are edited, so a later snapshot is a later set,
not this one. **This set is fixed**: a measurement quotes `commands-v1`,
and a new snapshot becomes `commands-v2` in the pull request that measures
against it. What may change in place is the `label` and `reason` of a
record, when step 1's vocabulary widens; `tools/compat-corpus/tests/`
recomputes every label and fails when the file and `classify.py` disagree,
so the change is visible in the pull request that makes it.
