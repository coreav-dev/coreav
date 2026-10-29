Gate drift c0 -1.2%, c1r -3.1%, c2r -1.2%, c3r +1.6%, c4 +1.1%, c5b +1.0%, c6b -2.7%; void: False []; expected backends {'libvpx': 'libvpx', 'fallback': 'ffmpeg'}

Arms ['libvpx', 'fallback']; backends {'libvpx': {'libvpx': 74}, 'fallback': {'ffmpeg': 74}}; ratios left out for another backend {'libvpx': 0, 'fallback': 0}; files where the arms chose another frame 0; apart {}

### S1 (ratio: arm / FFmpeg; libvpx/fallback: libvpx / fallback)

| Arm | Set | Files | Median | 95% CI | Below 1.00 | Max | Left out |
|---|---|---:|---:|---|---:|---:|---:|
| libvpx | all | 73 | 0.254 | 0.226 to 0.290 | 100% | 0.639 | 1 |
| libvpx | dev | 45 | 0.254 | 0.232 to 0.297 | 100% | 0.597 | 1 |
| libvpx | held-out | 28 | 0.226 | 0.122 to 0.298 | 100% | 0.639 | 0 |
| fallback | all | 73 | 0.294 | 0.252 to 0.346 | 100% | 0.876 | 1 |
| fallback | dev | 45 | 0.328 | 0.257 to 0.391 | 100% | 0.703 | 1 |
| fallback | held-out | 28 | 0.257 | 0.158 to 0.339 | 100% | 0.876 | 0 |
| libvpx/fallback | all | 74 | 0.846 | 0.782 to 0.901 | 85% | 1.156 | 0 |
| libvpx/fallback | dev | 46 | 0.873 | 0.787 to 0.925 | 83% | 1.156 | 0 |
| libvpx/fallback | held-out | 28 | 0.789 | 0.712 to 0.876 | 89% | 1.150 | 0 |

### S2 (ratio: arm / FFmpeg; libvpx/fallback: libvpx / fallback)

| Arm | Set | Files | Median | 95% CI | Below 1.00 | Max | Left out |
|---|---|---:|---:|---|---:|---:|---:|
| libvpx | all | 74 | 0.549 | 0.513 to 0.622 | 99% | 1.173 | 0 |
| libvpx | dev | 46 | 0.596 | 0.517 to 0.642 | 98% | 1.173 | 0 |
| libvpx | held-out | 28 | 0.499 | 0.453 to 0.616 | 100% | 0.834 | 0 |
| fallback | all | 74 | 0.631 | 0.594 to 0.709 | 96% | 1.259 | 0 |
| fallback | dev | 46 | 0.627 | 0.590 to 0.742 | 98% | 1.259 | 0 |
| fallback | held-out | 28 | 0.631 | 0.566 to 0.709 | 93% | 1.185 | 0 |
| libvpx/fallback | all | 74 | 0.846 | 0.782 to 0.901 | 85% | 1.156 | 0 |
| libvpx/fallback | dev | 46 | 0.873 | 0.787 to 0.925 | 83% | 1.156 | 0 |
| libvpx/fallback | held-out | 28 | 0.789 | 0.712 to 0.876 | 89% | 1.150 | 0 |

### S3 (ratio: arm / FFmpeg; libvpx/fallback: libvpx / fallback)

| Arm | Set | Files | Median | 95% CI | Below 1.00 | Max | Left out |
|---|---|---:|---:|---|---:|---:|---:|
| libvpx | all | 73 | 0.994 | 0.895 to 1.053 | 51% | 1.862 | 1 |
| libvpx | dev | 45 | 1.030 | 0.895 to 1.104 | 42% | 1.554 | 1 |
| libvpx | held-out | 28 | 0.915 | 0.765 to 1.041 | 64% | 1.862 | 0 |
| fallback | all | 73 | 1.114 | 1.049 to 1.154 | 32% | 3.033 | 1 |
| fallback | dev | 45 | 1.131 | 1.091 to 1.175 | 27% | 1.899 | 1 |
| fallback | held-out | 28 | 1.049 | 0.974 to 1.185 | 39% | 3.033 | 0 |
| libvpx/fallback | all | 74 | 0.899 | 0.862 to 0.931 | 82% | 1.333 | 0 |
| libvpx/fallback | dev | 46 | 0.917 | 0.875 to 0.938 | 78% | 1.207 | 0 |
| libvpx/fallback | held-out | 28 | 0.860 | 0.812 to 0.943 | 89% | 1.333 | 0 |

### S4 (ratio: arm / FFmpeg; libvpx/fallback: libvpx / fallback)

| Arm | Set | Files | Median | 95% CI | Below 1.00 | Max | Left out |
|---|---|---:|---:|---|---:|---:|---:|
| libvpx | all | 74 | 0.646 | 0.590 to 0.678 | 99% | 1.162 | 0 |
| libvpx | dev | 46 | 0.651 | 0.597 to 0.693 | 98% | 1.162 | 0 |
| libvpx | held-out | 28 | 0.597 | 0.498 to 0.689 | 100% | 0.891 | 0 |
| fallback | all | 74 | 0.719 | 0.654 to 0.752 | 96% | 1.222 | 0 |
| fallback | dev | 46 | 0.718 | 0.649 to 0.776 | 98% | 1.203 | 0 |
| fallback | held-out | 28 | 0.719 | 0.644 to 0.773 | 93% | 1.222 | 0 |
| libvpx/fallback | all | 74 | 0.868 | 0.815 to 0.894 | 88% | 1.164 | 0 |
| libvpx/fallback | dev | 46 | 0.871 | 0.827 to 0.926 | 85% | 1.164 | 0 |
| libvpx/fallback | held-out | 28 | 0.817 | 0.763 to 0.889 | 93% | 1.131 | 0 |

### S5 (ratio: arm / FFmpeg; libvpx/fallback: libvpx / fallback)

| Arm | Set | Files | Median | 95% CI | Below 1.00 | Max | Left out |
|---|---|---:|---:|---|---:|---:|---:|
| libvpx | all | 74 | 0.241 | 0.211 to 0.262 | 100% | 0.746 | 0 |
| libvpx | dev | 46 | 0.243 | 0.209 to 0.273 | 100% | 0.705 | 0 |
| libvpx | held-out | 28 | 0.228 | 0.175 to 0.330 | 100% | 0.746 | 0 |
| fallback | all | 74 | 0.287 | 0.253 to 0.343 | 100% | 0.794 | 0 |
| fallback | dev | 46 | 0.287 | 0.253 to 0.343 | 100% | 0.794 | 0 |
| fallback | held-out | 28 | 0.287 | 0.216 to 0.385 | 100% | 0.700 | 0 |
| libvpx/fallback | all | 74 | 0.829 | 0.772 to 0.871 | 88% | 1.173 | 0 |
| libvpx/fallback | dev | 46 | 0.839 | 0.781 to 0.905 | 87% | 1.173 | 0 |
| libvpx/fallback | held-out | 28 | 0.777 | 0.714 to 0.852 | 89% | 1.116 | 0 |

### By FFmpeg's time for the bar's command

| Bar | Arm | FFmpeg's time | Files | Median | 95% CI | Below 1.00 | Max |
|---|---|---|---:|---:|---|---:|---:|
| S1 | libvpx | under 50 ms | 20 | 0.446 | 0.382 to 0.499 | 100% | 0.639 |
| S1 | libvpx | 50 to 300 ms | 35 | 0.254 | 0.226 to 0.290 | 100% | 0.433 |
| S1 | libvpx | over 300 ms | 18 | 0.071 | 0.056 to 0.093 | 100% | 0.254 |
| S1 | fallback | under 50 ms | 20 | 0.503 | 0.445 to 0.556 | 100% | 0.876 |
| S1 | fallback | 50 to 300 ms | 35 | 0.294 | 0.252 to 0.332 | 100% | 0.564 |
| S1 | fallback | over 300 ms | 18 | 0.103 | 0.070 to 0.133 | 100% | 0.407 |
| S1 | libvpx/fallback | under 50 ms | 20 | 0.858 | 0.802 to 0.947 | 90% | 1.156 |
| S1 | libvpx/fallback | 50 to 300 ms | 35 | 0.873 | 0.740 to 0.925 | 83% | 1.150 |
| S1 | libvpx/fallback | over 300 ms | 18 | 0.782 | 0.623 to 0.896 | 83% | 1.103 |
| S2 | libvpx | under 50 ms | 47 | 0.646 | 0.592 to 0.695 | 98% | 1.173 |
| S2 | libvpx | 50 to 300 ms | 25 | 0.415 | 0.363 to 0.453 | 100% | 0.833 |
| S2 | libvpx | over 300 ms | 2 | 0.271 | 0.207 to 0.336 | 100% | 0.336 |
| S2 | fallback | under 50 ms | 47 | 0.711 | 0.634 to 0.767 | 94% | 1.259 |
| S2 | fallback | 50 to 300 ms | 25 | 0.565 | 0.512 to 0.598 | 100% | 0.920 |
| S2 | fallback | over 300 ms | 2 | 0.464 | 0.384 to 0.544 | 100% | 0.544 |
| S2 | libvpx/fallback | under 50 ms | 47 | 0.908 | 0.850 to 0.956 | 81% | 1.156 |
| S2 | libvpx/fallback | 50 to 300 ms | 25 | 0.690 | 0.641 to 0.784 | 92% | 1.142 |
| S2 | libvpx/fallback | over 300 ms | 2 | 0.578 | 0.538 to 0.618 | 100% | 0.618 |
| S3 | libvpx | under 50 ms | 20 | 1.041 | 0.904 to 1.191 | 45% | 1.463 |
| S3 | libvpx | 50 to 300 ms | 35 | 1.013 | 0.814 to 1.143 | 49% | 1.862 |
| S3 | libvpx | over 300 ms | 18 | 0.894 | 0.773 to 1.044 | 61% | 1.318 |
| S3 | fallback | under 50 ms | 20 | 1.075 | 0.943 to 1.159 | 35% | 1.899 |
| S3 | fallback | 50 to 300 ms | 35 | 1.129 | 0.993 to 1.304 | 34% | 1.510 |
| S3 | fallback | over 300 ms | 18 | 1.118 | 1.027 to 1.191 | 22% | 3.033 |
| S3 | libvpx/fallback | under 50 ms | 20 | 0.963 | 0.932 to 1.021 | 65% | 1.207 |
| S3 | libvpx/fallback | 50 to 300 ms | 35 | 0.875 | 0.829 to 0.943 | 83% | 1.333 |
| S3 | libvpx/fallback | over 300 ms | 18 | 0.793 | 0.707 to 0.883 | 100% | 0.973 |
| S4 | libvpx | under 50 ms | 44 | 0.704 | 0.674 to 0.747 | 98% | 1.162 |
| S4 | libvpx | 50 to 300 ms | 28 | 0.448 | 0.414 to 0.515 | 100% | 0.824 |
| S4 | libvpx | over 300 ms | 2 | 0.273 | 0.204 to 0.343 | 100% | 0.343 |
| S4 | fallback | under 50 ms | 44 | 0.773 | 0.741 to 0.832 | 93% | 1.222 |
| S4 | fallback | 50 to 300 ms | 28 | 0.597 | 0.567 to 0.632 | 100% | 0.963 |
| S4 | fallback | over 300 ms | 2 | 0.471 | 0.392 to 0.550 | 100% | 0.550 |
| S4 | libvpx/fallback | under 50 ms | 44 | 0.907 | 0.872 to 0.944 | 86% | 1.164 |
| S4 | libvpx/fallback | 50 to 300 ms | 28 | 0.744 | 0.695 to 0.808 | 89% | 1.131 |
| S4 | libvpx/fallback | over 300 ms | 2 | 0.572 | 0.521 to 0.624 | 100% | 0.624 |
| S5 | libvpx | under 50 ms | 7 | 0.431 | 0.180 to 0.637 | 100% | 0.705 |
| S5 | libvpx | 50 to 300 ms | 65 | 0.235 | 0.205 to 0.258 | 100% | 0.746 |
| S5 | libvpx | over 300 ms | 2 | 0.276 | 0.217 to 0.334 | 100% | 0.334 |
| S5 | fallback | under 50 ms | 7 | 0.435 | 0.205 to 0.601 | 100% | 0.638 |
| S5 | fallback | 50 to 300 ms | 65 | 0.273 | 0.246 to 0.334 | 100% | 0.794 |
| S5 | fallback | over 300 ms | 2 | 0.485 | 0.428 to 0.541 | 100% | 0.541 |
| S5 | libvpx/fallback | under 50 ms | 7 | 0.940 | 0.879 to 1.134 | 71% | 1.173 |
| S5 | libvpx/fallback | 50 to 300 ms | 65 | 0.814 | 0.764 to 0.853 | 89% | 1.117 |
| S5 | libvpx/fallback | over 300 ms | 2 | 0.562 | 0.507 to 0.618 | 100% | 0.618 |

Frames under 30 dB against FFmpeg's PNG (or not comparable), by bar and arm: {'S2': {'libvpx': 0, 'fallback': 0}, 'S3': {'libvpx': 0, 'fallback': 0}}

### Decode only (information, no bar; 73 files; first / second)

FFmpeg decoders in the binary: {'vp9': True, 'libvpx-vp9': True}; median seconds {'ffmpeg_libvpx_vp9': 1.7511502169999176, 'ffmpeg_vp9': 1.455384788000174, 'oracle': 6.5594905739999945}; failed {'oracle': 5, 'ffmpeg_vp9': 1}; skipped {}

| Pair | Set | Files | Median | 95% CI | Below 1.00 | Max |
|---|---|---:|---:|---|---:|---:|
| ffmpeg_libvpx_vp9/ffmpeg_vp9 | all | 72 | 1.036 | 0.979 to 1.106 | 42% | 1.760 |
| ffmpeg_libvpx_vp9/ffmpeg_vp9 | dev | 44 | 1.088 | 1.033 to 1.200 | 30% | 1.760 |
| ffmpeg_libvpx_vp9/ffmpeg_vp9 | held-out | 28 | 0.958 | 0.880 to 1.050 | 61% | 1.515 |
| oracle/ffmpeg_vp9 | all | 68 | 4.100 | 3.381 to 5.099 | 0% | 16.893 |
| oracle/ffmpeg_vp9 | dev | 42 | 5.022 | 3.921 to 6.038 | 0% | 16.893 |
| oracle/ffmpeg_vp9 | held-out | 26 | 2.493 | 1.719 to 3.826 | 0% | 10.211 |
| oracle/ffmpeg_libvpx_vp9 | all | 68 | 3.763 | 3.133 to 4.374 | 0% | 11.198 |
| oracle/ffmpeg_libvpx_vp9 | dev | 42 | 4.057 | 3.543 to 5.041 | 0% | 11.198 |
| oracle/ffmpeg_libvpx_vp9 | held-out | 26 | 2.458 | 1.782 to 4.032 | 0% | 10.740 |

### Frames against FFmpeg framemd5 (bar 1: 99% of streams, 99.9% of frames)

| Set | Streams | Agree | Frames | Equal | Not decoded | Decoder |
|---|---:|---:|---:|---:|---|---|
| all | 74 | 74 (100.0%) | 8408 | 8408 (100.00%) | {} | {'libvpx 1.15.0': 74} |
| dev | 46 | 46 (100.0%) | 5274 | 5274 (100.00%) | {} | {'libvpx 1.15.0': 46} |
| held-out | 28 | 28 (100.0%) | 3134 | 3134 (100.00%) | {} | {'libvpx 1.15.0': 28} |
