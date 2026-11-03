# H.264 intra macroblock bit limit

Status: Implementation plan for a G1 correctness fix. The encoder remains opt-in, safe scalar, fixed QP, intra-only, and CAVLC. No fallback, evaluation input, quality metric, or speed measurement is added.

## Defect and normative requirement

Rec. ITU-T H.264 (08/2024), clause A.3.1(n), limits each Constrained Baseline `macroblock_layer` to 3,200 bits. The existing access-unit and level checks do not enforce this separate bound. The original dense synthetic QP10 fixture has two otherwise-representable candidates of 4,031 and 3,909 macroblock-layer bits, so accepting either violates the declared level.

## Ownership and refusal

Make the existing macroblock syntax traversal own the bound. Record its starting bit position and check the complete syntax and residual length before returning. Counting and byte emission use the same traversal. Count neither slice headers nor RBSP trailing bits, NAL headers, emulation-prevention bytes, or any future skip-run prefix.

Add `Error::MacroblockLimit` as a typed refusal. Reject an oversized candidate before comparing costs and continue considering the existing legal modes. If a macroblock has no admissible candidate and at least one exceeded the bound, refuse the whole picture with that error. Keep coefficient and caller byte-budget errors distinct. No partial access unit is returned, and a refusal advances neither frame counters nor the next IDR identity.

Do not change transforms, quantisation, mode-search choices, QP, or tie ordering. Preserve output bytes for the existing conforming synthetic snapshot. No I_PCM escape or QP adjustment is introduced.

## Limitation and later fallback

Fixed QP with no I_PCM means that some dense content at low QP is refused, even when dimensions, plane layout, and coefficients are otherwise supported. A caller must handle that error. A later, separately planned slice may add I_PCM or a per-macroblock QP raise. Neither is part of this fix, and neither may silently change the fixed-QP evaluation method.

## Verification

- Reject the original dense QP10 input with `Error::MacroblockLimit`.
- Preserve the existing assertion that an admissible access unit can exceed 192 bytes, using a separate dense fixture at an admissible QP. Keep the explicit smaller caller budget refusal.
- Check refusal rollback by comparing subsequent successful access units and frame limits with a fresh encoder at the same successful prefix.
- Keep formatting, full-feature Clippy, unit tests, and default-feature isolation green. Run the existing complete synthetic three-decoder conformance on Actions.
- Pass the similarity guard on the exact public head against the main allowlist. Every commit carries DCO sign-off.

This prerequisite lands separately because it corrects the intra-only encoder before its evaluation. Dependent inter-picture work must use the same bit-limit owner after integration.
