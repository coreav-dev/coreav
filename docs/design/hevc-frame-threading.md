# HEVC picture scheduling

## Scope

This is the implementation plan for bounded picture-level concurrency in the opt-in native HEVC decoder. Wavefront decoding is already implemented; picture scheduling is not. The existing decoder thread option remains the only CLI control. Build defaults, syntax admission, decoder isolation, filter arithmetic and prediction arithmetic stay unchanged.

Start with pictures whose complete references are available. This can overlap independent intra pictures and pictures sharing older completed anchors, including streams without WPP or tiles. It cannot overlap a chain in which each picture references its immediate predecessor. No input-coverage or speed claim follows from adding a queue. Verification must observe simultaneous picture reconstruction and report the actual eligible-picture share.

## Current boundary

`PictureDecoder::read_nal` validates headers and reconstructs each slice before returning. On the next first slice, `StreamCore` validates that slice header before finalizing the previous picture. `Dpb` permits only one pending attachment, publishes only complete filtered samples and owns POC, reference marking and presentation-order bumps.

Queued work cannot retain the current `SliceInput`: it borrows the local RBSP, entry array, tile layout and active parameter-set slots. Nor can a worker borrow the mutable DPB while the caller admits another picture. These ownership boundaries must change before dispatch can overlap pictures.

Keep the synchronous `read_nal` contract. The native backend can use a separate scheduled-input entry point through its existing send/receive protocol. That path may defer reconstruction completion and failures until synchronization, but it must select the same first failure and valid output prefix as serial execution. Header and packet validation, callback execution and external output reservations remain caller-owned. The new entry point must document this distinction; changing thread count must not silently change the synchronous API into an asynchronous one.

## Ownership and data flow

The caller owns one parser and one normative DPB. A private scheduler in `decoder/scheduler.rs` owns bounded picture jobs, their input storage, started worker handles and ordered outcomes. Workers own a `Frame`, immutable input and complete reference snapshots. They never mutate the normative DPB, invoke callbacks or reserve external output memory.

The intended internal shapes are:

```rust
struct PictureJob {
    index: u64,
    segments: Vec<OwnedSlice>,
    references: ReadyReferences,
    reservation: Reservation,
}

struct ReadyReferences {
    entries: Vec<RetainedReference>,
}

struct PictureResult {
    frame: Frame,
    outcomes: Vec<OperationOutcome>,
}
```

`OwnedSlice` owns the bytes, entry lengths, activated parameters and layout required by the existing slice walker. Requested and actual capacities are charged before ownership transfers. `RetainedReference` identifies a completed picture by coded index and POC, not a recyclable DPB slot. Its immutable storage remains pinned through the last borrowing job even if normative reference marking removes that picture. A single reference-reader interface serves the serial DPB and ready snapshots; reconstruction arithmetic is shared rather than copied.

Replace the single pending attachment with bounded indexed completion records. Keep reference marking, missing-reference validation and output selection in the existing DPB owner. A picture can run only after every selected reference has complete filtered samples and motion fields. Earlier incomplete pictures need not block an independent ready picture, but neither completion order nor DPB removal can release a reference still used by a job.

The caller reconciles results at synchronization points: required reference acquisition, output bumps, memory or entropy reservation pressure, explicit drain, end-of-sequence transitions and failure. It waits for the required serial prefix before committing counters, sample attachments and output callbacks. Suffix hashes stay associated with their original picture and are checked before that picture can become an output or reference.

## Ordering and limits

Operation order is finer than picture order. A later first-slice header is validated before the previous picture's boundary finalization, while an earlier slice's reconstruction failure precedes that later header. Record those operation positions and reconcile them in the same order. A later worker's error must never cancel an earlier worker before its result is known. Catch worker panics with the existing internal-error mapping, wake dependencies and join every started worker before teardown.

The execution limit is `min(requested_threads, 16, available_parallelism)`, with a one-worker fallback when host capacity is unavailable. The caller counts against this limit whenever it executes reconstruction. Frame and WPP work share this one limit; the first implementation selects one scheduling level for a group instead of nesting pools. An explicit request above host capacity must not create oversubscription.

Cold serial admission remains authoritative. Charge scheduler records, owned input, retained reference owners, working pictures, bounded stacks and finalization scratch, without double-counting shared samples. Before an optional job or retained allocation could make a required allocation fail, reconcile the necessary work and release optional storage. A decode admitted without scheduling must remain admitted with scheduling at the same caller limit. Test this against an independent cold decoder, not only against a prior warm call. Do not introduce a pressure-sensitive worker-cache policy.

Reserve a conservative complete entropy-work bound before speculative dispatch. Keep independent stream and picture limits. If a reservation cannot fit, reconcile prior consumption and continue through the existing serial budget path. Reconcile actual bin counts in operation order rather than allowing a racing shared counter to select a failure. Allocation and deadline failures remain terminal, with no fallback to another decoder.

## Existing worker follow-ups

- Resolve host capacity once through the shared worker-limit owner. Test requests below, equal to and above available CPUs, including sixteen requested on four CPUs.
- Do not copy a working picture into each picture job. Move its buffer owner. For reused WPP frames, replace whole-frame `reset_private` copies with a proved row-local initialization and publication rule. Stale samples, cells or filter records must not become available to prediction. Full-frame zero fills are not the intended optimization.
- Preserve scratch admission for skipped pictures. RBSP and entry-array storage is allocated before temporal extraction or RASL suppression is known. Compare its success and refusal boundaries with independent cold decoding, both with and without retained worker storage. Do not remove a charge because no sample frame exists.

## Alternatives

Incremental reference rows offer overlap in dependent picture chains. They require separate publication of motion fields and filtered samples, waiting for actual motion-compensation footprints including fractional-filter halos and edge replication. A fixed row lead is insufficient. Current finalization performs whole-picture vertical and horizontal deblocking followed by SAO, so useful early publication also requires a separate filtering control-flow and storage redesign. That work is outside this first implementation.

A syntax-ahead pipeline could overlap entropy decoding with the previous picture's reconstruction while retaining complete-reference publication. It requires a bounded command representation and syntax-only neighbour state. The current traversal immediately resolves motion, predicts and reconstructs residuals, so this is a different front-end and reconstruction split, not a scheduler-only shortcut.

Choose complete-reference jobs because they retain existing filtering and CTU semantics behind a small scheduled-input boundary. Accept limited DAG width rather than combining several ownership redesigns in one change. Tile scheduling follows separately unless the same ready-job and publication machinery applies without another ownership change.

## Verification before timing

First generate a non-WPP anchor with two pictures that depend on that anchor but not on each other. Barriers must prove both reconstruction jobs run before either finishes. A strict reference-chain control must prove that a dependent job cannot read an unfinished reference. A queue-only or all-serial test does not establish concurrency.

Compare complete planes, coded hashes, presentation order, timestamps, origins, warnings and work counters with the synchronous decoder. Exercise independent and dependent segments, suffix hashes, output suppression, POC wrap, reference removal, sequence changes, geometry changes and partial worker start. Reverse worker completion and failure order. Inject errors, panics, callback failure and deadlines after real parallel entry, then prove all workers and pinned allocations are released.

Locate independent cold memory and bin-budget boundaries with large parser scratch, skipped pictures, queued outputs and retained references. Require identical admission and first-error selection across thread counts. Poison reusable storage to detect omitted initialization, and delay reference completion to detect premature reads. Run complete conformance at 1, 2, 4 and 8 requested threads, recording host-bounded actual workers, plus sanitizer stress where supported.

Only then run paired reference measurements on existing inputs, using exact candidate and baseline commits and the qualified pinned FFmpeg oracle. Separate useful frame overlap, WPP execution and serial calls. Report output identity, coverage, wall time, CPU time and host capacity. Shared-runner figures are references, not acceptance bars. Every pending remote run has a timer backstop of at most thirty minutes.

Sources remain ITU-T H.265 and conformance data only. The first implementation step is the generated overlap and dependency proof together with its bounded ownership path, before a wider scheduling rollout.
