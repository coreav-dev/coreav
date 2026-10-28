# Contributing to CoreAV

CoreAV does not accept code pull requests. Bug reports, feature requests
and proposals are welcome as issues.

## Issues

- **Bug reports**: what you ran (command or API call, CoreAV version or
  commit, platform), what you expected, and what happened, with the exact
  output. A file CoreAV rejects or misreads that `ffprobe` reads is a bug
  report on its own: attach the smallest such file, and say what
  `ffprobe` (and, if you have it, MediaInfo) reports for it.
- **Feature requests**: what you need from CoreAV and why: the formats,
  operations or interfaces, and what you use today instead.
- **Proposals and discussion**: a change to what CoreAV does or how, with
  your reasoning and, where you have them, measurements.
- **Sample media and benchmark results**: real files in formats or from
  devices and services CoreAV handles badly or not at all; benchmark
  inputs, commands, hardware and numbers, ideally next to FFmpeg's on the
  same machine.

The issue forms ask for these fields. Security problems go through
[`SECURITY.md`](SECURITY.md), not through public issues.

### Files you send

Attach only files you may share, and say which of these they are:
recorded by you for CoreAV testing; shared with written permission for
CoreAV testing; or published under a license that allows this use (name
it). Never send a file that shows an identifiable person without that
person's consent, and prefer clips without identifiable people or
locations at all. Say what personal data a file holds (faces, a location,
a device serial) where you know. The real-file corpus that the
correctness bars are measured on is private: say whether a file may be
published, kept private, or only used for measurement. A file attached to
a public issue is public; for a file that should not be, say so in the
issue, or write to [hello@coreav.dev](mailto:hello@coreav.dev), and the
maintainer will arrange a private transfer. Media files are
never committed to the repository.

## Code pull requests

A pull request is not merged. It is turned into an issue that quotes its
description without its code, and closed; until that redirect is switched
on, it stays open unmerged, and an issue with the same description is the
way in. Code in a pull request, an issue, a comment or an attached file is
not used to make a change: whoever makes it works from the behavior you
describe, your reproducer files and the specifications. The most useful
thing a report can carry is therefore what is wrong and how to see it, not
a patch.

A change that comes from your issue links it, and you are credited by your
GitHub handle in the issue and the change, unless you ask not to be.
Project commits carry a Developer Certificate of Origin sign-off
(`Signed-off-by`); there is no contributor license agreement. Code from
outside is not merged, so neither asks anything of you.
