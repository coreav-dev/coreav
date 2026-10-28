# Known divergences: verified picture sizes versus the oracle

Bar S2 of [`design/frame-sizes.md`](design/frame-sizes.md) requires the
scanned size of every video stream that `ffprobe` decodes to be at least
`ffprobe`'s `width` and `height`, except where an entry of this page covers
the difference. The corpus v1 run needed none: no stream's
scanned size was smaller than `ffprobe`'s (`docs/spikes/frame-sizes/`).

Differences in the other direction (the scanned size larger than
`ffprobe`'s) are within the bound and are listed with the results of bar
S3 rather than here.

There are no entries.
