# H.264 conformance archive acquisition

## Plan

Keep the conformance manifest, its digests, URL candidates, selected streams and decode bars unchanged. Archive acquisition stays in `scripts/h264_conformance.py`; every cached or newly downloaded archive must match its pinned SHA-256 before extraction.

The existing Actions cache is saved only after a successful job, so an acquisition or decoder failure can discard the downloads from that run. Split cache restore and save around a download-only preparation step in both conformance jobs. Keep the complete-cache key bound to the manifest bytes. Save partial progress under a separate manifest-bound recovery key after acquisition failure, and restore it only when no complete cache exists. Save before decoding so decoder failures do not discard verified downloads. Keep every action pinned to its existing full commit SHA.

For each existing URL candidate, make at most three network attempts with backoff of 5 and 10 seconds. Retry connection refused, connection or host unreachable, connection reset, timeouts, incomplete HTTP transfers and HTTP 5xx. A permanent HTTP failure moves to the next existing candidate without backoff. Other operating-system failures fail immediately. Log each URL before opening it and log its outcome. Use a temporary download file and publish it only after digest verification.

A digest mismatch permits at most one re-download within the existing URL candidates and attempt budget. A second mismatch fails the archive immediately, without trying another URL or accepting the bytes. A corrupt cached archive also consumes the first mismatch, so only one replacement download may fail its digest. Network failures consume the same bounded per-URL attempt budget. Invalid cached bytes are removed before reacquisition and never used.

## Verification

Use standard-library tests with synthetic archive bytes and injected network outcomes. Cover valid and corrupt cache hits, transient recovery and exhaustion, permanent HTTP fallback, one digest re-download and repeated mismatch, partial-transfer cleanup, local filesystem errors and URL diagnostics. Exercise download-only preparation without invoking Cargo or extraction. Check the workflow cache ordering, digest-bound keys, failure retention and pinned actions.

Run the focused tests and workflow checks locally without fetching the suite. GitHub Actions runs the unchanged full conformance suite in fast and safe modes. An upstream outage that exceeds the bounded retries remains a failure, not a decoder pass; no new mirror or service is introduced.
