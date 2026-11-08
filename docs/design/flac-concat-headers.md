# FLAC concat frame headers

`coreav ffmpeg -f concat -i list.txt -c copy output.mp4` must describe one FLAC stream, rather than several independently numbered streams in one track. RFC 9639 Sections 8.2, 9.1 and 9.1.5 require one blocking strategy, contiguous coded numbers and valid block-size bounds. FFmpeg 9.0.2 retains the input headers, including a short internal block and restarted frame numbers.

## Plan

Survey bounded frame headers before building the output tables. Reuse the audio crate's RFC parser and read at most 16 bytes per packet. When the joined headers already describe a continuous stream, keep the packet bytes. Otherwise rewrite every header to variable blocking with the cumulative sample number. Copy subframes through the existing fixed window and update CRC-8 and CRC-16 without buffering or decoding frames. Preserve an invalid frame's CRC-16 error syndrome rather than concealing corruption.

The survey and table passes share each input's packet-work budget. The rewritten sizes determine sample tables and chunk offsets before publication. A changed input or a size disagreement fails the temporary output. Single-input copies and the native `join` command retain their current packet-copy behavior. Unparseable headers retain packet copy rather than adding a new native refusal; the conformance guarantee concerns valid FLAC inputs with established sample clocks. Existing refusals for internal blocks below 16 samples are unchanged, because headers cannot repair those blocks.

## Metadata

RFC 9639 Section 8.2 Table 3 describes the output, not its first input. Set the total to the sum of frame block sizes, and MD5 to zero, meaning unknown. Set frame-byte bounds from the rewritten sample sizes. Block bounds include every non-final block and the largest block. Variable blocking permits conservative bounds; if the measured bounds are equal, widen the declared range so STREAMINFO does not describe fixed blocking. Keep sample rate, channel count and bit depth.

## Verification

Use generated PCM to property-test header rewriting across coded-number lengths, uncommon header fields and copy-buffer splits. Decode rewritten frames and compare every sample with the input PCM in order. Check cumulative sample numbers, both CRCs and STREAMINFO. The pinned-oracle concat regression must retain complete container and video checks while proving the intentional FLAC packet difference and equal decoded PCM. Keep the measurement rosters, baselines and divergence registry unchanged.
