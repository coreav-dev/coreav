# Bundle 3b: local screen

Implementer's VM (Intel Xeon @ 2.50 GHz, family 6, model 85, 8 vCPUs; not the bar class), `base` `m-1e7d` against
`b3n` (`../binaries.sha256`), run after the bars were committed (`4b5a85af`). Scripts: bundle 2's `ab1t.py` and
`ab8.py` (`../../bundle2/`), the paths to this VM's streams and binaries.

- One thread pinned, six rounds (one warm-up not kept), discard, CPU time (`local-1t.txt`): all 32 **0.9873
  [0.9811, 0.9911]**, 28 of 32 faster; sd 0.9911, 720p 0.9840, 1080p 0.9884, 4k 0.9692. The screen (at most 0.990,
  upper end below 1.000) is met. During part of it a documentation check (the public-reset dry run) ran on other
  cores of the VM; the builds' order rotates each call, so it fell on both arms.
- Eight threads, five rounds, discard (`local-8t.txt`): wall **0.9861 [0.9633, 1.0031]**, 21 of 32 faster; sd
  0.9963, 720p 0.9572, 1080p 1.0014, 4k 0.9730; CPU 0.9878. The screen (at most 1.005) is met.

The gating run is requested (`../gating-request.md`).
