# Release VP9 decode cost

The release's static musl build has additional decode CPU cost compared with a GNU build linked to Debian's libvpx. This round isolates that additional cost. It does not attempt to close libvpx's remaining gap against FFmpeg's native VP9 decoder.

## Plan and acceptance bars

The measured base is the queued copy-once head `065479c8a6d3a47a965301d4e5eba6ee10c88e68` of coreav-dev/coreav#178. The harness and candidate use that exact base, not the earlier measurement's source. Before this change opens for review, compare its diff again after the copy-once change reaches main.

Profile the release-equivalent decode harness and the GNU/Debian harness at function level on a Compute Engine c4-standard-8 with SMT disabled, using the same ten B5 files as the earlier libvpx comparison. Separate profiling from timing. Retain allocation and frame-export work in both timed harnesses, and describe this integration cost beside any decoder comparison. Before choosing a change, test the ceiling of removing the implicated work in a diagnostic build.

The candidate and its exact build commands will be fixed in a commit before the judged run. The judged host is a c4-standard-8, four physical cores with SMT disabled, Debian 13, and the shared pinned FFmpeg 9.0.2. Run five balanced, alternating rounds per file. Use equal repetition units for short commands. Record CPU time, wall time, exit status and frame count for every invocation. Run pixel hashes outside timing on all ten files, requiring frame-by-frame identity between stock, candidate and FFmpeg.

Acceptance requires all of the following:

- The median candidate/Debian CPU ratio on the three 4K and 8K files is at most 1.02.
- No file's median candidate/stock CPU ratio exceeds 1.02, allowing only measurement noise rather than a regression.
- The median candidate/Debian CPU ratio across all ten files is at most 1.03.
- Every frame hash agrees, all commands succeed, and all frame counts agree.
- The before/after host gate drifts no more than 5%, and the identical-command control has a median ratio within 2% of 1.00.

Report each file's median and range, the median ratios with file-bootstrap 95% confidence intervals, and the fraction of files cheaper than FFmpeg. These bars target release/Debian parity, not a claim of being faster than FFmpeg. Such a claim needs an interval whose upper bound is below 1.00. A completed missed candidate is not rerun; only a recorded host fault permits a replacement run.
