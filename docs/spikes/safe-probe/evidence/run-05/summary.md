Run valid (sanity checks): True
Held-out in scope: 303; opened by both parser engines: 300; headline set: 241

Missing metadata: CoreAV does not extract tags, chapters, codec configuration, profile and level, or bit rate; libavformat parses them at open.

| Pair | Median per-file ratio [95%]; p10, p90 | Claim |
|---|---|---|
| parser speed (memory) (B-lean / A-index) | 2.33x [2.26, 2.40]; p10 1.36, p90 2.75; n=241 | faster |
| parser speed (file system) (B-lean-file / A-index-file) | 2.97x [2.82, 3.09]; p10 1.45, p90 3.89; n=241 | faster |
| B-mov reference (B-mov / A-index-file) | 3.74x [3.58, 3.91]; p10 1.92, p90 4.96; n=241 | faster |
| design: probe only vs B (B / A) | 5.98x [5.85, 6.09]; p10 4.94, p90 10.96; n=241 | faster |
| design: probe only vs B-mov (B-mov / A) | 4.19x [4.02, 4.29]; p10 3.09, p90 10.22; n=241 | faster |
| operational cost (lean) (D-lean / A-sbx) | 1.26x [1.20, 1.51]; p10 1.07, p90 1.76; n=300 | faster |
| operational cost (default ffprobe) (D / A-sbx) | 1.80x [1.72, 1.85]; p10 1.14, p90 4.93; n=300 | faster |

### parser speed (memory) by category
| Category | Ratio |
|---|---|
| group-a | 1.63x [1.30, 2.06]; p10 0.91, p90 2.30; n=13 |
| group-b | 2.54x [2.36, 2.72]; p10 2.28, p90 2.80; n=18 |
| group-c | 1.63x [1.46, 2.36]; p10 1.17, p90 2.76; n=35 |
| group-d | 2.40x [2.29, 2.49]; p10 1.60, p90 2.72; n=106 |
| group-e | 2.57x [2.27, 2.67]; p10 2.12, p90 2.89; n=19 |
| group-f | 2.32x [2.17, 2.49]; p10 1.92, p90 2.72; n=34 |
| group-g | 2.18x [1.64, 2.35]; p10 1.07, p90 2.75; n=16 |

### parser speed (memory) by index entries per file
| Entries | Ratio |
|---|---|
| 0-1000 | 2.55x [2.52, 2.61]; p10 2.24, p90 2.84; n=114 |
| 1000-10000 | 2.24x [2.15, 2.31]; p10 1.64, p90 2.60; n=72 |
| 10000-100000 | 1.54x [1.24, 1.71]; p10 0.89, p90 1.86; n=43 |
| 100000-1000000000000 | 1.40x [1.25, 1.63]; p10 1.08, p90 1.64; n=12 |

### parser speed (memory) by tag
| Tag | Ratio |
|---|---|
| moov-front | 2.36x [2.23, 2.49]; p10 1.17, p90 2.81; n=105 |
| moov-end | 2.32x [2.22, 2.41]; p10 1.61, p90 2.70; n=136 |
| frag-sidx | 2.32x [2.06, 2.50]; p10 1.99, p90 2.59; n=11 |
| elst | 2.36x [2.30, 2.43]; p10 1.37, p90 2.76; n=207 |
| elst-multi-edit | 1.62x [0.93, 2.32]; p10 1.07, p90 2.18; n=2 |
| elst-empty-edit | 2.40x [2.14, 2.59]; p10 1.62, p90 2.69; n=21 |
| multi-audio | 2.69x [2.53, 2.99]; p10 2.56, p90 2.92; n=4 |
| long-30min | 1.40x [0.93, 1.62]; p10 0.89, p90 1.65; n=10 |
| large-2gb | 1.59x [1.18, 1.72]; p10 1.02, p90 2.03; n=7 |
| pcm | 2.57x [2.35, 2.73]; p10 2.30, p90 2.76; n=12 |
| timecode | 2.54x [2.38, 2.74]; p10 2.15, p90 2.92; n=17 |
| v:h264 | 2.40x [2.32, 2.46]; p10 1.61, p90 2.75; n=188 |
| v:hevc | 2.08x [1.82, 2.64]; p10 1.77, p90 2.93; n=17 |
| v:av1 | 1.39x [1.03, 1.96]; p10 0.89, p90 2.41; n=17 |
| v:prores | 2.52x [2.41, 2.93]; p10 2.41, p90 2.93; n=6 |
| audio-only | 1.63x [1.30, 2.06]; p10 0.91, p90 2.30; n=13 |
| quicktime | 2.50x [2.42, 2.64]; p10 2.00, p90 2.88; n=34 |

CoreAV slower on 9 files: 2ac72e8784ecca18 (group-a, 28.9 MB, 0.676); fb932c60f5ef172f (group-e, 2468.1 MB, 0.782); 3f89a1f748a962fe (group-a, 17.3 MB, 0.81); e9f1ee99874e1c0a (group-d, 207.8 MB, 0.87); 3637f155f6d08104 (group-g, 308.8 MB, 0.893); 09a07d37fd0963a2 (group-d, 208.9 MB, 0.896); 6e67814d336c695a (group-c, 411.3 MB, 0.926); 892f0243deef8061 (group-c, 548.0 MB, 0.932); 0c09c76072802293 (group-d, 119.9 MB, 0.976)

### operational cost (lean) by category
| Category | Ratio |
|---|---|
| group-a | 1.15x [1.11, 1.21]; p10 1.05, p90 1.51; n=23 |
| group-b | 1.13x [1.10, 1.24]; p10 1.07, p90 1.66; n=27 |
| group-c | 1.53x [1.28, 1.58]; p10 1.06, p90 2.97; n=57 |
| group-d | 1.55x [1.50, 1.57]; p10 1.09, p90 1.72; n=113 |
| group-e | 1.56x [1.20, 1.69]; p10 1.10, p90 1.81; n=23 |
| group-f | 1.12x [1.09, 1.23]; p10 1.04, p90 1.76; n=36 |
| group-g | 1.13x [1.10, 1.19]; p10 1.08, p90 1.54; n=21 |

### operational cost (lean) by tag
| Tag | Ratio |
|---|---|
| moov-front | 1.21x [1.16, 1.28]; p10 1.07, p90 1.73; n=138 |
| moov-end | 1.51x [1.24, 1.56]; p10 1.07, p90 1.77; n=162 |
| frag-sidx | 1.12x [1.09, 1.18]; p10 1.07, p90 1.22; n=20 |
| elst | 1.31x [1.22, 1.51]; p10 1.06, p90 1.75; n=245 |
| elst-multi-edit | 1.28x [1.05, 3.01]; p10 1.08, p90 2.41; n=5 |
| elst-empty-edit | 1.66x [1.58, 1.73]; p10 1.52, p90 2.85; n=22 |
| multi-audio | 1.17x [0.98, 1.66]; p10 1.04, p90 1.50; n=5 |
| long-30min | 2.52x [1.57, 2.98]; p10 1.25, p90 4.57; n=23 |
| large-2gb | 1.11x [1.05, 2.99]; p10 1.05, p90 3.58; n=8 |
| pcm | 1.11x [1.09, 1.13]; p10 1.05, p90 1.23; n=23 |
| timecode | 1.12x [1.09, 1.16]; p10 1.04, p90 1.54; n=28 |
| telemetry | 1.14x [1.09, 1.19]; p10 1.07, p90 1.60; n=18 |
| v:h264 | 1.55x [1.39, 1.57]; p10 1.08, p90 1.78; n=206 |
| v:hevc | 1.18x [1.11, 1.24]; p10 1.06, p90 1.53; n=27 |
| v:av1 | 1.14x [1.09, 1.52]; p10 1.08, p90 1.58; n=19 |
| v:prores | 1.07x [1.01, 1.12]; p10 1.01, p90 1.12; n=6 |
| audio-only | 1.17x [1.11, 1.22]; p10 1.07, p90 1.51; n=22 |
| quicktime | 1.13x [1.09, 1.19]; p10 1.03, p90 1.57; n=46 |
| encrypted | 1.15x [1.09, 1.22]; p10 1.08, p90 1.51; n=9 |

CoreAV slower on 2 files: 98506324b6878eb5 (44.1 MB, 0.979); 1a5fb95f5146fee8 (469.9 MB, 0.998)

### Sanity checks
| More work / less work | Median [95%] | Passed |
|---|---|---|
| B' / B | 4.34x [3.93, 5.14]; p10 1.21, p90 33.54; n=342 | True |
| B / B-mov | 1.41x [1.39, 1.44]; p10 1.06, p90 1.71; n=342 | True |
| A-index / A | 1.41x [1.34, 1.46]; p10 1.05, p90 6.04; n=337 | True |
| D / D-lean | 1.22x [1.06, 1.46]; p10 0.99, p90 3.27; n=340 | True |

### Fixed cost and per-sample cost (Theil-Sen, headline files)
| Engine | us per open [95%] | ns per entry [95%] |
|---|---|---|
| A-index | 99.3 [96.2, 102.7] | 41.8 [40.6, 43.7] |
| B-lean | 275.9 [263.3, 286.7] | 63.3 [57.7, 70.0] |
| A-index-file | 60.3 [58.3, 62.1] | 41.0 [39.7, 42.3] |
| B-lean-file | 251.1 [239.4, 259.2] | 65.7 [59.2, 72.5] |

### Bytes read (warm, headline files)
| Engine | Median per file | Total MB |
|---|---|---|
| A | 12767.0 | 53.4 |
| A-index | 12767.0 | 53.4 |
| A-index-file | 16621.0 | 60.0 |
| B-lean | 16599.0 | 54.6 |
| B-lean-file | 16599.0 | 54.6 |
| B-mov | 45520.0 | 62.6 |
| B | 45520.0 | 62.6 |
| B' | 135696.0 | 162.2 |

### Cold cache (UNVERIFIED: eviction worked for 0% of requests)
- parser speed (file system): 3.46x [3.18, 3.65]; p10 1.45, p90 4.93; n=241
- B-mov reference: 4.45x [4.15, 4.75]; p10 1.86, p90 6.52; n=241
- eviction: {'max': 811026, 'files_not_evicted': 1021}
- bytes A-index-file: median 16621.0, total 60.0 MB
- bytes B-lean-file: median 16599.0, total 54.6 MB
- bytes B-mov: median 45518.0, total 62.6 MB

### Memory per request (KiB, headline files)
- A: median 4464.0, max 16548
- A-index: median 4556.0, max 26900
- B-lean: median 86292.0, max 109148
- B-mov: median 85980.0, max 121728
- A-sbx: median 4188.0, max 11192
- D-lean: median 16904.0, max 49380
- D: median 20832.0, max 130800

### Decision 15 as written
{"W1-1 A/B summed": {"files": 300, "ratio": 10.341056886248108, "ci95": [10.027083484297018, 10.698986078310279]}, "W1-5 A/D summed": {"files": 300, "ratio": 218.39401319917474, "ci95": [210.88403610651028, 226.71737835976592]}, "p99_ms": {"A": 2.568274, "A-sbx": 27.879999, "D": 376.435843, "D-lean": 91.619515}}

### Hostile suite
{"counts": {"A": {"rejected": 20, "ok": 90, "dropped": 1}, "A'": {"rejected": 20, "ok": 90, "dropped": 1}, "A-index": {"rejected": 20, "ok": 90, "dropped": 1}, "A-sbx": {"rejected": 20, "ok": 90, "dropped": 1}, "D": {"rejected": 9, "ok": 102}, "A-strict": {"rejected": 21, "ok": 90}}, "worst": {"A": {"seconds": 0.03284318299984079, "peak_kib": 4952}, "A'": {"seconds": 0.03254519100005382, "peak_kib": 4996}, "A-index": {"seconds": 0.017302670999924885, "peak_kib": 4972}, "A-sbx": {"seconds": 0.032802990999925896, "peak_kib": 4992}, "D": {"seconds": 0.0302457720001712, "peak_kib": 23432}, "A-strict": {"seconds": 0.03070346899994547, "peak_kib": 4980}}, "failures": [], "total_files": 111}

### Self-made, biased
| Pair | Ratio |
|---|---|
| parser speed (memory) | 2.37x [2.22, 2.52]; p10 1.75, p90 2.69; n=37 |
| parser speed (file system) | 3.44x [3.09, 3.77]; p10 2.37, p90 4.03; n=37 |
| B-mov reference | 4.56x [4.03, 4.81]; p10 3.32, p90 5.28; n=37 |
| design: probe only vs B | 5.13x [5.02, 5.42]; p10 4.44, p90 5.83; n=37 |
| design: probe only vs B-mov | 3.14x [2.91, 3.25]; p10 2.47, p90 3.91; n=37 |
| operational cost (lean) | 1.19x [1.10, 1.46]; p10 1.04, p90 1.70; n=37 |
| operational cost (default ffprobe) | 1.22x [1.12, 1.69]; p10 1.08, p90 1.83; n=37 |
