# Installed H.264 preflight

This Linux-only driver compares the installed, optimized tarball with the native build of the same bounded operation. It is not a hosted Workers verdict. Do not run it, the fixture generator, or the ignored media tests on a personal device. The registered bars and the sixteen decoder-free gates are unchanged.

## Inputs

- Use the Node pin from `.github/workflows/wasm.yml` and `npm ci --prefix packages/wasm/examples` for its existing pinned esbuild/workerd dependencies.
- Build and pack the H.264 package through its distribution script. Capture the existing package check privately: `env -u GITHUB_STEP_SUMMARY node packages/wasm-h264/scripts/check-package.mjs --tarball "$TARBALL" > "$PRIVATE/package-record.json"`. This record must describe the same tarball, module, edge profile, feature closure and pinned compiler/optimizer/bindings generator.
- Build the native `edge-native` reference with `native-h264`, and the Linux-only `coreav-wasm` `thumbnail_contract` integration-test executable with that feature. The counting allocator is in the test executable, not the reference executable.
- Install both programs from `scripts/oracle-ffmpeg.env` with the existing oracle installer and the pinned x264 executable with `scripts/install-x264.sh`. No other multimedia implementation is a reference.

All directories below must be outside the checkout. Capture directories and their contents remain private.

```sh
COREAV_THUMBNAIL_HOSTILE_DIR="$PRIVATE/hostile" \
  "$TEST_BINARY" --ignored --exact export_synthetic_contract_fixtures

python3 scripts/wasm-thumbnail-fixtures.py \
  --oracle-bin "$ORACLE_BIN" --x264 "$X264" \
  --hostile-manifest "$PRIVATE/hostile/hostile.json" \
  --output "$PRIVATE/fixtures"

COREAV_THUMBNAIL_MANIFEST="$PRIVATE/fixtures/manifest.json" \
COREAV_THUMBNAIL_ALLOCATION_REPORT="$PRIVATE/allocations.json" \
  "$TEST_BINARY" --ignored --exact generated_4k_refusals_allocate_no_decoded_frame

node packages/wasm-h264/test/installed/check.mjs \
  --fixtures "$PRIVATE/fixtures" --tarball "$TARBALL" \
  --package-record "$PRIVATE/package-record.json" \
  --edge-native "$EDGE_NATIVE" \
  --allocation-report "$PRIVATE/allocations.json" \
  --allocation-binary "$TEST_BINARY" \
  --host-cases packages/wasm-h264/test/installed-host-cases.mjs \
  --output "$PRIVATE/capture"
```

The generator's `--plan` option prints invocations and the fixture roster without launching binaries. It does not generate media or produce a verdict. All launched subprocesses have finite deadlines. Pattern input is two seconds at 25 fps, progressive 8-bit 4:2:0, without audio; closed IDRs occur at frames zero and 25. The generator verifies installed pins, SPS coded grids, stream formats, packet counts and keyframe cadence. Lying-container variants patch fixed-width container fields without changing SPS or media payloads.

The hostile handoff is a JSON array of `{id,path,sha256,options,expectation}` rows. Paths are relative to that file. The exact 17 invalid and four valid-boundary identifiers are owned by `roster.json`; their expectations are independent structured error kinds or `success`. Generation copies and rehashes those bytes. The driver refuses incomplete, duplicate or changed rosters. Successful boundary cases receive full image and summary comparison, not a refusal exemption.

## Captures

The eleven memory gates require 338 individual readings: a separate startup isolate; two entropy modes per fresh gate; four honest/lying 4K refusals followed by benign reuse per rejection gate; 312 sequential warm cases; and three actual lease-held admission stages. Every reading is checked, not only the last one. Additional parity crosses JPEG qualities 1/91/100, all supported filter/chroma modes, dimensions and explicit stream selection. Host short-read, hidden oversized backing and opening/output cancellation tests run through a separate required module hook on a real installed-engine isolate.

Each response is streamed and hashed without collecting its image in the workerd isolate. Both its body and summary must finish, source cleanup must settle, and deferred work must drain before memory is read. Captures bind fixture hashes, canonical option hashes, request UUIDs, monotonic engine operation identities, module/tarball hashes, runtime identity, native executable identity and the source-bound allocation witness. Only `decoder.name` may differ in the summary; packet, time, geometry and every other field must agree. A 4K witness requires a qualified allocation bound and zero trough, not an inference from WASM pages.

`diagnostics().host.maxBackingBytes` and `maxChargedBytes` already conservatively combine independently observed source and output peaks. They are not exact simultaneous hosted-memory peaks. The output-specific peaks remain separately checked against the queue partition. Startup has no host operation and may report no host account. Missing operation accounting fails even when pages fit.

`captures.ndjson`, `verdict.json`, runtime/native logs, the generation record and the package record are private evidence. Public driver output contains verdict counts or a failure label, never raw readings. Initialization failures also leave a nonpassing private record. The driver refuses stale package provenance, mutable fixture bytes, runtime exits, incomplete work, missing host tests or allocation evidence, unknown/repeated gate steps and changed isolate identities.

## Synthetic checks

These commands do not generate or open media:

```sh
node --test packages/wasm-h264/test/installed/ledger.test.mjs
python3 -m unittest discover -s scripts/tests -p test_wasm_thumbnail_fixtures.py
node --check packages/wasm-h264/test/installed/check.mjs
node --check packages/wasm-h264/test/installed/worker.js
```

The implementation and roster must be committed before a later judged run. A passing Linux preflight establishes component evidence only; hosted total memory, correlated provider outcomes, the runtime reserve and release readiness remain unvalidated until their separately authorized complete run.
