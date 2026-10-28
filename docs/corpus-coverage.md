# Corpus v1 coverage report

Generated from corpus v1's private manifest ([`corpus.md`](corpus.md), "Corpus v1"). **Bold** marks a cell with fewer than 5 files. The `edge` and `hostile` columns are totals for those sets.

## Files per set

| set | total | GB |
|---|---:|---:|
| real | 1351 | 217.8 |
| edge | 1619 | 29.8 |
| hostile | 0 | 0.0 |

Holdout (real): 407 of 1351.

## Coverage tags (real)

| tag | real total | edge | hostile |
|---|---:|---:|---:|
| edit list: B-frame delay (`elst-bframe-delay`) | 452 | 219 | 0 |
| edit list: AAC priming (`elst-audio-priming`) | 208 | 89 | 0 |
| edit list: real edits (>1 media edit) (`elst-multi-edit`) | 11 | 48 | 0 |
| edit list: empty edit (`elst-empty-edit`) | 79 | 76 | 0 |
| negative composition offsets (`ctts-negative`) | 51 | 42 | 0 |
| fragmented with sidx (`frag-sidx`) | 94 | 75 | 0 |
| fragmented without sidx (`frag-no-sidx`) | 7 | 61 | 0 |
| media segment (no moov) (`segment`) | **4** | 21 | 0 |
| moov at end (`moov-end`) | 563 | 463 | 0 |
| moov at front (`moov-front`) | 485 | 715 | 0 |
| variable frame rate (`vfr`) | 169 | 184 | 0 |
| 10-bit or more (`10bit+`) | 95 | 137 | 0 |
| HDR PQ (`hdr-pq`) | 6 | 23 | 0 |
| HDR HLG (`hdr-hlg`) | 6 | 10 | 0 |
| Dolby Vision (`dolby-vision`) | **3** | 15 | 0 |
| rotation (display matrix) (`rotation`) | 15 | 16 | 0 |
| MV-HEVC / stereo (`mv-hevc`) | **0** | 21 | 0 |
| ProRes (any) (`prores`) | 25 | 20 | 0 |
| ProRes Proxy (`prores-proxy`) | **0** | 1 | 0 |
| ProRes LT (`prores-lt`) | **1** | 3 | 0 |
| ProRes 422 (`prores-422`) | **2** | 3 | 0 |
| ProRes 422 HQ (`prores-hq`) | 8 | 6 | 0 |
| ProRes 4444 (`prores-4444`) | 14 | 5 | 0 |
| ProRes 4444 XQ (`prores-4444xq`) | **0** | 1 | 0 |
| timecode track (`timecode`) | 111 | 83 | 0 |
| telemetry: GoPro GPMF (`telemetry-gpmf`) | 38 | 1 | 0 |
| telemetry: DJI (`telemetry-dji`) | **1** | 0 | 0 |
| telemetry (any) (`telemetry`) | 73 | 9 | 0 |
| chapters (`chapters`) | 26 | 71 | 0 |
| subtitle track (`subtitle-track`) | 37 | 139 | 0 |
| encrypted (CENC/cbcs) (`encrypted`) | 31 | 110 | 0 |
| interlaced (`interlaced`) | **3** | 70 | 0 |
| several audio tracks (`multi-audio`) | 19 | 79 | 0 |
| audio only (`audio-only`) | 161 | 231 | 0 |
| QuickTime brand (`quicktime`) | 153 | 272 | 0 |
| 3GPP brand (`3gpp`) | 20 | 76 | 0 |
| CMAF brand (`cmaf`) | 44 | 65 | 0 |
| WebM (`c:webm`) | 174 | 50 | 0 |
| Matroska (`c:matroska`) | 35 | 164 | 0 |
| Ogg (`c:ogg`) | 38 | 23 | 0 |
| Motion JPEG 2000 (`c:mj2`) | **0** | 1 | 0 |
| H.264 (`v:h264`) | 711 | 454 | 0 |
| HEVC (`v:hevc`) | 121 | 111 | 0 |
| AV1 (`v:av1`) | 95 | 55 | 0 |
| VP9 (`v:vp9`) | 74 | 33 | 0 |
| VP8 (`v:vp8`) | 53 | 23 | 0 |
| MPEG-4 Part 2 (`v:mpeg4`) | 41 | 123 | 0 |
| AAC (`a:aac`) | 554 | 393 | 0 |
| Opus (`a:opus`) | 75 | 29 | 0 |
| E-AC-3 (`a:eac3`) | **4** | 7 | 0 |
| ALAC (`a:alac`) | 5 | 12 | 0 |
| PCM audio (`pcm`) | 68 | 151 | 0 |
| long (>= 30 min) (`long-30min`) | 98 | 52 | 0 |
| large (>= 2 GB) (`large-2gb`) | 26 | 0 | 0 |

## Gaps: fewer than 5 files in the whole `real` set

- media segment (no moov): 4 real (21 edge)
- Dolby Vision: 3 real (15 edge)
- MV-HEVC / stereo: 0 real (21 edge)
- ProRes Proxy: 0 real (1 edge)
- ProRes LT: 1 real (3 edge)
- ProRes 422: 2 real (3 edge)
- ProRes 4444 XQ: 0 real (1 edge)
- telemetry: DJI: 1 real (0 edge)
- interlaced: 3 real (70 edge)
- Motion JPEG 2000: 0 real (1 edge)
- E-AC-3: 4 real (7 edge)

## Oracle outcomes (all sets)

- edge / error: 70
- edge / ok: 1336
- edge / ok-warn: 213
- real / ok: 1325
- real / ok-warn: 26
