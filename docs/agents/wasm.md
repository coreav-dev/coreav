# WASM

The WASM module and its HTTP source.

| Rule | Enforced by |
|---|---|
| WASM memory gates read only the registered bars, reject decoded duplicate registry keys and require complete isolate-bound readings with exact harness concurrency | `registry`, `gates` and `batch` own the parser, closed gate ledger and request lifecycle; their regressions run with the example unit suite in `wasm-contract`. The module job judges the installed artifact and emits no raw readings |
| WASM host exceptions stay in `cause`, not in I/O document text or the public error message | Fixed open-failure documents and the Rust-owned read-failure document in `engine.js`; source-open and source-read credential-marker regressions in `packages/wasm/test/module.test.mjs`. Types cannot classify sensitive data in arbitrary JavaScript exceptions |
| HTTP source sizes describe the identity bytes actually read; URLs carry no userinfo, and cross-origin redirect chains never forward caller headers | The one HTTP adapter owns request headers, redirect handling and encoding checks; real gzip 200/206 and header-forwarding regressions in `packages/wasm/test/http.test.mjs` |
| WASM initialization names missing JSPI or exception-reference support before the generated glue runs | `EXNREF_PROBE` plus the JSPI/tag checks in `init`; a Node subprocess with exception references disabled in `packages/wasm/test/module.test.mjs` |
| An HLS rebuild does not accept re-read stored bytes as a fresh build, and concurrent loads retain one owner | The cache owns stored-versus-built provenance in its ready and loading records; stored-load/rebuild and uncached-build regressions in `packages/wasm/examples/test/unit/indexes.test.mjs`. A new byte array alone cannot encode that provenance |
| A measurement inventory classifies every public export as measured or explicitly excluded, without filtering away unknown exports | Not enforced: judgment until the planned measurement harness exists. `docs/design/wasm-workers-measurement-plan.md` requires disjoint, complete inventories and rejection before timing; a prose check cannot prove a future harness's classification |
