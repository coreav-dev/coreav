# Spikes

A spike is an experiment or a measurement made outside the Cargo workspace,
before or beside the production code: it states a hypothesis and its pass
and kill criteria before the run, then the results. Each folder here is
one spike: its `README.md`, and the data its results come from
(`evidence/`, and the input sets and lists it names). The code that ran a
spike is not part of the published tree; its README says what it did. How the evidence is kept, and what is redacted in
it, is in [`../measurements/README.md`](../measurements/README.md).

| Spike | What it measured |
|---|---|
| [`spike-a`](spike-a/README.md) | Stage 1 Spike A, the ML loading slice; [`h7-sidx`](spike-a/h7-sidx/README.md), the census of `sidx` in fragmented MP4, and [`h7-adaptive`](spike-a/h7-adaptive/README.md), adaptive routing |
| [`nvdec-direct`](nvdec-direct/README.md) | Spike B: NVDEC driven directly from Rust |
| [`parsing-guard`](parsing-guard/README.md) | Spike C: the parsing-layer guard, with its isolated libavformat [`baseline`](parsing-guard/baseline/README.md) |
| [`safe-probe`](safe-probe/README.md) | Stage 2 W1: the safe probe benchmark, with its probe [`baseline`](safe-probe/baseline/README.md) and sandbox |
| [`h1-gpu`](h1-gpu/README.md) | Stage 2 W2: H1 with direct NVDEC in the Spike A loader |
| [`w1-remeasure`](w1-remeasure/README.md), [`w2-hls-index`](w2-hls-index/README.md), [`w3-sample-walk`](w3-sample-walk/README.md) | The library probe, the packet index and the HLS index against libavformat, in three rounds |
| [`h264-n1`](h264-n1/README.md) | Track N1: H.264 decoder conformance |
| [`h264-n2`](h264-n2/README.md) | Track N2: H.264 decoder performance |
| [`h264-native-thumbnail`](h264-native-thumbnail/README.md) | Thumbnails on CoreAV's own H.264 decoder |
| [`thumbnail-bench`](thumbnail-bench/README.md) | Thumbnails and the decode boundary against FFmpeg |
| [`audio-decode-bench`](audio-decode-bench/README.md) | Audio decoding against FFmpeg: agreement and speed |
| [`audio-extract-bench`](audio-extract-bench/README.md) | Audio extraction to M4A on the corpus |
| [`clip-bench`](clip-bench/README.md) | Keyframe-aligned clips to MP4 on the corpus |
| [`split-bench`](split-bench/README.md) | Keyframe splits on the corpus |
| [`join-bench`](join-bench/README.md) | Joins on the corpus |
| [`remux-bench`](remux-bench/README.md) | Full-file remux: faststart and MOV to MP4 on corpus v1 |
| [`hls-bench`](hls-bench/README.md) | HLS packaging on corpus v1, and [sources with several edits](hls-bench/several-edits/README.md) |
| [`large-tables`](large-tables/README.md) | Large sample tables against libavformat |
| [`parity-bench`](parity-bench/README.md) | Matroska, subtitles and chapters at parity with FFmpeg; the Python binding's overhead |
| [`remote-bench`](remote-bench/README.md) | Remote media against `ffprobe` and `ffmpeg` on the same URLs |
| [`read-window`](read-window/README.md) | The read window's in-place refill |
| [`frame-sizes`](frame-sizes/README.md) | The picture size verification on corpus v1 |
| [`policy-check`](policy-check/README.md) | The upload policy check on corpus v1 |
| [`policy-matroska`](policy-matroska/README.md) | The upload policy checks of Matroska and WebM files |
