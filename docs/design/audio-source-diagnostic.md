# Audio source and encoder boundary diagnostic

This diagnostic separates decoder sample differences from presentation and encoder differences when a supported source is re-encoded as AAC. It uses only trusted, generated fixtures and the shared pinned FFmpeg oracle. It does not change B9, a tolerance, a baseline or a release bar.

## Inputs and execution

Generate the existing two-second VP9/Opus browser fixture, a 44.1 kHz FLAC control and a 48 kHz FLAC control. Record each file's SHA-256, sample rate and channel count. Run on GitHub Actions with finite command and job timeouts; no corpus or held-out input is read. A pull-request or manual workflow produces a small JSON report.

## Decoder comparison

Decode each input through `coreav decode-audio` as native-rate, one-thread F32 PCM. Compare the actual WAV data samples with the pinned FFmpeg's F32 decode, using the shared audio input arguments. Report sample counts, hashes, zero-lag SNR and maximum absolute difference. Compute normalized cross-correlation over a bounded sample window for lags from -2,048 to 2,048 samples, reporting the best candidates and the zero-lag score. A periodic source can make lag peaks ambiguous; a peak alone does not prove a presentation error.

## Encoder comparison

Encode the two captured PCM streams independently with the pinned AAC encoder, using the pipeline's raw F32 input settings and its default bitrate. Also encode each PCM stream through an ordinary FFmpeg raw-input command. Compare the ordered AAC packet hashes against an explicit `coreav ffmpeg -c:v libx264 -c:a aac` output and the ordinary FFmpeg source encode. This explicit audio choice remains available even when an implicit Opus-to-AAC choice is refused.

## Interpretation

Equal counts and alignment, with each PCM stream reproducing its respective AAC packets, isolate decoder numerics from encoder or presentation behavior. Unequal lengths, an unambiguous lag or a failure to reproduce the encoder output needs investigation in the source or encoder pipeline. The report records observations, not a passing quality verdict. Infrastructure or command failures abort the diagnostic rather than becoming evidence of agreement.

B9's exact end-to-end comparison stays unchanged. A diagnostic result cannot authorize a different predicate or a known-divergence exemption.
