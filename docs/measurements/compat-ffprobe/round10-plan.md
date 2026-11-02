# B4 round 10 measurement plan

This is a new B4 round, not a replacement or re-score of round 9. The [round 9 plan](round9-plan.md) and recorded outcomes remain unchanged. This plan merges first, a separate driver change merges second, and a separately authorized run can happen only after both are on `main`. This document authorizes no infrastructure launch. Simulation code/results are design evidence, not an implemented timing driver or a product verdict.

The [calibration design](../../design/b4-calibration.md) fixes the algorithms, synthetic envelope, receipts and limitations.

| Item | Round 10 registration |
|---|---|
| Product bars | Unchanged: median CoreAV/FFmpeg ratio at most 1.00 per command, overall and per container; a faster claim requires a 95% upper bound below 1.00 |
| Product commands/inputs | Unchanged: 93 native ffprobe commands of commands-v1, frozen 20 ISO base media, 20 Matroska/WebM and 17 MPEG-TS inputs |
| Eligibility, refusal, failure, fallback, completeness | Unchanged; rejected or missing work cannot become a speed win |
| Product statistic/interval | Unchanged: five timed pairs, ratio of side medians, stratified whole-file bootstrap with 10,000 draws and seed 0 |
| Calibration inputs | Eight development files per container, 24 total, separate from product inputs |
| Calibration statistic | Exponentiated median of balanced two-pair log contrasts, replacing the arithmetic median of individual call ratios |
| Calibration interval | Synchronized global four-pair blocks plus stratified files, 999 draws; 99.5% gate interval and diagnostic 95% interval |
| Calibration point band | Unchanged numerical band [0.98, 1.02], applied to the new calibration estimate |
| Conditioning/order | Fixed conditioning, counterbalanced AB/BA, interleaved calibration arms, independently recorded logical side and position |
| Timing | Unchanged whole-process wall time, including launch and blocking reap, one process at a time |
| Telemetry | Per-call child resources and exposed host counters, outside the timed function; no subtraction |
| Other bars | B1, B2, B3a, B3b, B5, B6 and R4 are not amended |

The frozen product selection has historical observations. Reuse does not make it newly untouched validation data. Report prior use; do not claim independent new held-out evidence merely because the round number changes. Development calibration never contributes product speed ratios.

## Freeze identities and selection

Before payload access, freeze a private manifest containing the merged plan/tooling commits, immutable candidate and fetched-main baseline, default-feature release binary checksums, oracle pin, command hash, both selections and the complete call schedule. The candidate and judging tools must be fixed `main` revisions with green required CI. Use result method `b4-counterbalanced-v2`; the reporter rejects earlier methods, never re-scores them.

Use the exact shared FFmpeg 9.0.2 pin in `scripts/oracle-ffmpeg.env`, verified by `oracle_pin.py --verify-bin` before use, including cache hits. Arguments, features, strict-safe mode, output destinations and absence of gconv modules remain as registered for B4. Nothing changes within the arm.

For calibration, use the existing metadata's explicit real development partition, existing container/extension mapping and 256 MiB size limit. Sort eligible files within a container by `sha256("coreav-b4-calibration-v2:" + file_sha256)` and take the first eight. Missing partition metadata is not evidence of development membership: require explicit development membership under the manifest's schema. Verify digest disjointness from every registered held-out roster before downloading. Fewer than eight eligible files in any container means unsupported: do not reduce the count, substitute, borrow held-out files or select by timing/agreement.

Freeze the selected digest/size roster and verify every calibration and product file before conditioning. Public evidence uses opaque aliases, not hashes or source paths. Keep roster identities and source metadata in the private manifest.

## Host, conditioning and schedule

Use on-demand GCP c4-standard-8, SMT off, one pinned physical core for parent and both children, with the existing host gate. Record CPU model, topology, clock policy, operating system and timer. Preparation/builds are separate from judged timing. A failed host gate prevents measurement.

Calibration uses two commands, `-show_format -show_streams` and `-show_packets`, and two identical-argv arms, FFmpeg/FFmpeg and CoreAV/CoreAV. Execute four global visits. In each visit interleave containers by selected rank, then visit all four cases of that file. Alternate arm-first order and command-first order by visit ordinal plus file ordinal. Each case performs four timed pairs per visit. Before its first visit, run four untimed pairs AB, BA, AB, BA. Before each later visit, run two untimed pairs AB, BA to restore per-file conditioning after intervening inputs. Every case therefore has ten untimed pairs and 16 timed pairs. No cache flushing or adaptive settling is permitted.

Generate timed crossover orientations with `random.Random(10)` before execution, iterating selected rank, container, command and crossover ordinal. For each of eight crossovers call `getrandbits(1)` once: zero means AB then BA, one means BA then AB. Share this orientation between calibration arms. Untimed schedules consume no random bits. Logical A remains the numerator when it executes second.

There are 4,992 planned calibration child invocations: 24 files times two commands times two arms times 26 pairs times two calls. Of these, 3,072 are timed. Finish both calibration arms before making a statistical decision. Stop early only for a process, capture, integrity, fallback or safety failure and mark incomplete. A normal statistical miss finishes the fixed schedule and is final.

The gate requires both new calibration estimates in [0.98, 1.02], both 99.5% intervals containing 1.00, complete observations and no fallback records. Record these components separately. Publish case, position and early/late summaries plus telemetry without adding post-hoc rejection thresholds. A failed calibration blocks product execution; it does not prove a physical host malfunction.

After a passing calibration, execute the unchanged product selection/commands. Each product case gets four fixed untimed pairs AB, BA, AB, BA, then five timed pairs alternating AB and BA. Starting orientation is the parity of command ordinal plus within-container file rank. Odd rounds cannot balance positions within one case; opposite starts across files/commands limit that imbalance without changing the product estimator or repetition count.

Keep native eligibility/refusal coverage, failed phases and both logical exit statuses. Executed fallback or unreadable fallback records invalidate native measurement. Missing eligible pairs, failed timed pairs and unmeasured groups prevent a clean B4 verdict. Never replace a failing input or count a refusal as speed evidence.

Keep each child's 600-second event-driven timeout. A separately approved launch must set a hard total deadline and in-script infrastructure cap before provisioning. Reaching a safety limit gives an incomplete result, not a passing partial arm. No adaptive stopping, dropped timed rounds, startup subtraction, telemetry correction or retry for a narrow miss is allowed. Replacement requires a separately documented host fault, not a new method.

## Evidence and interpretation

The tools PR implements and tests this frozen design before a run. This plan's synthetic validation must pass for every registered scenario under both fixed simulation seeds. Its 99% upper bound on whole-gate null rejection must be at most 5%; report per-arm power for a persistent 2% bias and sensitivity at 0.5%. These are conditional simulations, not measured FFmpeg speed or real-host coverage guarantees.

Publish sanitized call receipts and planned, executed, missing, refused, ineligible and failed counts. Show both calibration interval levels and say explicitly that 99.5%, not 95%, governs calibration. Product reports retain per-command, per-container and overall medians/95% intervals, the share of successful ratios below 1.00 and coverage alongside verdicts. Separate development calibration from product results.

Publish host class, exact versions, immutable public commits and sanitized timings/telemetry without instance identifiers, home paths or source identifiers. Preserve bulky raw evidence separately. A later unexpected result may motivate another future plan, never a changed gate or rejudgment of this arm.
