# VPSSPSPPS_A oracle-only input preparation

This recipe was registered before execution and applies only to `VPSSPSPPS_A_MainConcept_1.bin`, SHA-256 `8501a5e5f0f191d3810a7aecab43aacac1fcc28f08cb3731158cc48a89501a27`. It does not alter the input supplied to CoreAV or permit any other stream preparation.

H.265.1 (10/2018) section 6.6.12.5 puts duplicated parameter sets in arbitrary order before the pictures. The published description lists six I pictures with changing resolutions and explicitly states `length: 6 frames`. The pinned oracle on the original input emits missing-parameter diagnostics and outputs only two pictures. CoreAV parses and outputs all six from the untouched original. The original mismatch remains recorded; this preparation does not classify incomplete oracle output as harmless.

**Captain decision, 2026-11-04:** order the leading parameter sets for this oracle input only. Require exact agreement with the published whole-output MD5 and retain the published description count of six. This addresses the oracle's handling of the parameter-set order tested by H.265.1 section 6.6.12.5; it does not change the native test.

## Closed transformation

1. Verify the exact stream name and original SHA-256 before preparing bytes.
2. Independently frame every Annex B NAL. The original has 318 leading parameter-set NALs: 106 VPS, 106 SPS and 106 PPS. Reject a non-parameter-set NAL in that prefix or a parameter-set NAL after the first slice; this is not a general remuxer.
3. Stably order that prefix as VPS, SPS, PPS. Preserve every NAL byte, its start code and associated padding. Preserve the order within each type, including duplicated IDs. Preserve any initial zero padding. Do not parse, rewrite, deduplicate, add or remove a parameter set.
4. Append the original first-slice unit and every following byte unchanged. Require identical total byte length and first-VCL position. Reframe the result and require every NAL payload to equal its corresponding original payload under the recorded permutation.
5. Record the original and prepared SHA-256, byte lengths, NAL permutation, per-NAL byte digests, prefix counts and unchanged-suffix proof. The prepared digest is observed after this registration; it is not invented in advance.
6. Give only this prepared file to the pinned FFmpeg/FFprobe oracle. CoreAV and its independently checked NAL records continue to use the original file. Record both input identities on every prepared-oracle diagnostic and reference receipt. Compare picture boundaries at unchanged VCL offsets; do not compare reordered parameter-set positions as if they were original positions.

The published reference remains `1ddf74263cb4953cfdfcf99c563d88ea` from `VPSSPSPPS_A_MainConcept_1_md5.txt`, attachment SHA-256 `94d141bea9d3c61631e35ea282a1cca485d47325a4bbc92bdeced14afb564870`. The count source is `VPSSPSPPS_A_MainConcept_1.txt`, SHA-256 `35df5c194219524b2bab8bcff8260eb0405f1b61df1af260db06284d1703e061`. All six declared geometries must agree with native and complete prepared-oracle output. A nonzero exit, incomplete output, unknown diagnostic, wrong count, byte-preservation failure or MD5 mismatch keeps the stream blocked. No other packing, parameter rewrite or oracle source is authorized.

## G1 evidence-source entry

| Stream | Count source | Reference hash | Oracle input | Reason and decision | Status |
|---|---|---|---|---|---|
| VPSSPSPPS_A | Published description, 6 pictures; source SHA-256 above | Published complete-output MD5 above | Stable VPS/SPS/PPS ordering of the leading prefix; original retained for CoreAV | H.265.1 section 6.6.12.5 tests arbitrary parameter-set order, which the pinned oracle does not handle on the original; Captain decision, 2026-11-04 | Verified in the complete G1 census at `028ca00a6831b64faa226614252edd0290ce642d`: six outputs and exact published-MD5 agreement |

The observed prepared SHA-256 is `d2097afe2c90bda74915fae48d567ac260b7932781644ba9cdb3ebbc69d315dd`. The complete receipt confirms zero exit, completed progress and all six outputs from that prepared input, while native header checks use the unchanged original. This is header/reference evidence, not a CoreAV reconstruction verdict.

The preregistration at `77c534c5ccfeca14291da8b65ace90482a5c599c` and the completed receipt bind the original document SHA-256 `bee17edd278a1a75644fce4fab67aa6378182c7aa5764e2f09b412237347ab2d`. This status update changes the document bytes; it does not change or relabel that historical receipt. Future preparation receipts recompute and bind the then-current document SHA-256.
