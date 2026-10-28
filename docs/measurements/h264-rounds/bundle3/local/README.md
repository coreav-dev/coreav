# Bundle 3: local screen

Implementer's VM (Intel Xeon @ 2.50 GHz, family 6, model 85, 8 vCPUs; not the bar class), `base` `m-974f` against
`b3` (`../binaries.sha256`), run after the bars were committed (`e6489c25`, `4311a512`). Scripts: bundle 2's
`ab1t.py` and `ab8.py` (`../../bundle2/`), the paths to this VM's streams and binaries.

- One thread pinned, six rounds (one warm-up not kept), discard, CPU time (`local-1t.txt`): all 32 **0.9851
  [0.9820, 0.9918]**, 27 of 32 faster; sd 0.9922, 720p 0.9820, 1080p 0.9910, 4k 0.9835; the 16 streams at 1080p and
  4k 0.9873 (14 of 16 faster). The screen (at most 0.990, upper end below 1.000) is met.
- Eight threads, five rounds, discard (`local-8t.txt`): wall **0.9918 [0.9832, 0.9962]**, 23 of 32 faster; sd 0.9901,
  720p 0.9878, 1080p 0.9886, 4k 1.0105; CPU 0.9873. The screen (at most 1.005) is met.

The gating run was requested. Read beside the bars, as information: the 1080p and 4k streams here (0.9873) are above
B3-2's 0.985, and the 4k class at eight threads (1.0105) is just above B3-6's 1.010; this host is not the bars' host.
