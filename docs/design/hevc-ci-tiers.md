# HEVC CI tiers

Keep the existing header checks on pull requests and compare the four registered synthetic recipes, covering eight Main10 64×64 unfiltered pictures, on Linux. Reuse the existing synthetic inspector and intra metadata and pixel validators without changing their contracts. Reject incomplete or repeated picture identities and retain a safe incomplete report on failure.

Run the unchanged full vector and fixed-file metadata jobs only on main pushes, the daily 03:45 UTC schedule, and manual dispatches. Do not add merge-queue work. Full-run reporting reads only Actions metadata from trusted main, requires the existing proof steps, and deduplicates failures on the HEVC tracking issue. Explicit suite selection verifies HEVC full evidence without making it a default final-head requirement.

Validate child boundaries and adversarial evidence locally with mocks. Run real synthetic decoding only in Linux Actions. Preserve all existing headers, intra and filter coverage, metadata census membership, secret handling, and sealed artifacts.
