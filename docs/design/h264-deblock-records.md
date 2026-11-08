# Borrow deblocking records

The encoder keeps its current deblocking order, boundary strengths, thresholds and scalar pixel filter. This change borrows macroblock records while deriving each edge instead of copying their motion-vector arrays into edge and segment temporaries. Record lookup and the public encoder API stay unchanged.

This is an output-preserving optimization. Verification compares complete streams and reconstructions with the baseline, in dispatched and scalar builds at one, two, three and eight workers. Existing deblocking conformance and encoder fixtures remain required. Instruction ceilings and alternating one-worker and four-worker timing run on native x86-64 hosts before acceptance.

Pixel SIMD and overlap with row decisions are outside this change. They have separate safety and dependency requirements and are not needed to remove record copies.
