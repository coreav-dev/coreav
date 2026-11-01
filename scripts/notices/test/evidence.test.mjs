import assert from "node:assert/strict";
import { test } from "node:test";
import { moduleEvidence, nativeEvidence, noticeSections, validateEvidence } from "../evidence.mjs";

const commit = "a".repeat(40);
const context = {
  version: "1.2.3", commit,
  members: new Set(["coreav_wasm", "coreav_types"]),
  memberPaths: new Set(["crates/coreav-wasm", "crates/coreav-types", "tools/test-tool"]),
  library: new Set(["core", "alloc", "std", "stdarch", "compiler-builtins"]),
};

function u32(value) {
  const bytes = [];
  do {
    const digit = value % 128;
    value = Math.floor(value / 128);
    bytes.push(digit | (value ? 128 : 0));
  } while (value);
  return Buffer.from(bytes);
}

function text(value) {
  const bytes = Buffer.from(value);
  return Buffer.concat([u32(bytes.length), bytes]);
}

function section(id, bytes) {
  return Buffer.concat([Buffer.from([id]), u32(bytes.length), bytes]);
}

function module(...sections) {
  return Buffer.concat([Buffer.from([0, 97, 115, 109, 1, 0, 0, 0]), ...sections]);
}

function nameMap(records) {
  return section(1, Buffer.concat([u32(records.length), ...records.map(([index, name]) => Buffer.concat([u32(index), typeof name === "string" ? text(name) : name]))]));
}

function names(...symbols) {
  return section(0, Buffer.concat([text("name"), nameMap(symbols.map((name, index) => [index, name]))]));
}

function paths(...values) {
  return section(0, Buffer.concat([text("source-evidence"), Buffer.from(`\0${values.join("\0")}\0`)]));
}

function evidence(sourcePaths = [], symbols = ["__udivti3"]) {
  return moduleEvidence(module(paths(...sourcePaths)), module(names(...symbols)));
}

function notice(label, origin = "crates.io", license = "MIT", extra = "") {
  return `=== ${label} (${origin}) ===\nLicense: ${license}\n${extra}\n--- LICENSE ---\nUpstream permission.\n`;
}

const standard = () => notice(`Rust standard library ${context.version} (${commit})`, "Rust toolchain", "MIT OR Apache-2.0", "Crates: alloc, core, std, std_detect, rustc-std-workspace-core\n");
const notices = () => [
  standard(), notice("compiler_builtins 0.1.160", "Rust toolchain"),
  notice("dlmalloc 0.2.14", "Rust toolchain, crates.io", "MIT/Apache-2.0"),
  notice("serde 1.0.0"), notice("wasm-bindgen 0.2.129"),
].join("\n");

test("notice sections retain all three origins and grouped standard-library crate names", () => {
  const sections = noticeSections(notices() + notice("dlmalloc 0.2.14"));
  assert.equal(sections.size, 6);
  assert.deepEqual(sections.get("Rust toolchain, crates.io\0dlmalloc 0.2.14"), { origin: "Rust toolchain, crates.io", label: "dlmalloc 0.2.14", license: "MIT/Apache-2.0" });
  assert.ok(sections.has("crates.io\0dlmalloc 0.2.14"));
  assert.deepEqual(sections.get(`Rust toolchain\0Rust standard library ${context.version} (${commit})`).crates, ["alloc", "core", "std", "std_detect", "rustc-std-workspace-core"]);
  assert.equal(noticeSections(notices().replaceAll("\n", "\r\n")).size, 5);
  assert.equal(noticeSections("").size, 0);
  assert.equal(noticeSections(notices() + "=====================================\nUpstream separator\n=====================================\n").size, 5);
});

test("legacy crate identifiers keep their leading underscores", () => {
  assert.deepEqual(evidence([], ["_ZN7__rustc3foo17h0123456789abcdefE"]).roots, new Set(["__rustc"]));
});

test("crate identity paths are checked even when the embedded file is not Rust source", () => {
  const result = evidence(["/rust/deps/dlmalloc-0.2.14/LICENSE-MIT", "/cargo/registry/index/serde-1.0.0/README.md", `\/rustc/${commit}/library/core/Cargo.toml`]);
  assert.deepEqual(result.deps, new Map([["dlmalloc", new Set(["0.2.14"])]]));
  assert.deepEqual(result.registry, new Map([["serde", new Set(["1.0.0"])]]));
  assert.deepEqual(result.library, new Set(["core"]));
});

test("vendored source identities cannot hide under the standard-library copyright section", () => {
  const result = evidence([`/rustc/${commit}/library/vendor/dlmalloc-0.2.14/src/lib.rs`]);
  assert.deepEqual(result.deps, new Map([["dlmalloc", new Set(["0.2.14"])]]));
  assert.throws(() => validateEvidence(result, noticeSections(standard()), context), /vendored crate dlmalloc 0.2.14/);
});

test("notice parsing rejects malformed headers, origins, licences, grouped lists and duplicates", () => {
  for (const value of [
    "=== broken ===\nLicense: MIT\n",
    notice("serde 1.0.0", "other"),
    "=== serde 1.0.0 (crates.io) ===\nNo licence line.\n",
    "=== serde 1.0.0 (crates.io) ===\nLicense: \n",
    notice("serde"),
    standard().replace(" (Rust toolchain) ===", " (crates.io) ==="),
    standard().replace(/^Crates:.*\n/m, ""),
    standard().replace("Crates: alloc, core, std, std_detect, rustc-std-workspace-core", "Crates: core, core"),
    standard().replace("Crates: alloc, core, std, std_detect, rustc-std-workspace-core", "Crates: std-detect, std_detect"),
    standard().replace(commit, "short"),
  ]) assert.throws(() => noticeSections(value));
  assert.throws(() => noticeSections(notice("serde 1.0.0") + notice("serde 1.0.0")), /duplicate/);
  assert.throws(() => noticeSections(standard() + standard()), /duplicate/);
});

test("both artifacts contribute paths with exact commits, library dirs, versions and relative members", () => {
  const result = moduleEvidence(
    module(paths(`/rustc/${commit}/library/core/src/fmt.rs`, "/rust/deps/dlmalloc-0.2.14/src/lib.rs", "/cargo/registry/index/serde-1.0.0/src/lib.rs", "crates/coreav-types/src/lib.rs")),
    module(names("__udivti3"), paths(`/rustc/${commit}/library/alloc/src/vec.rs`, "/rust/deps/hashbrown-0.17.1/src/raw.rs", "/cargo/registry/index/serde-1.0.1/src/lib.rs", "tools/test-tool/src/main.rs")),
  );
  assert.deepEqual(result.commits, new Set([commit]));
  assert.deepEqual(result.library, new Set(["core", "alloc"]));
  assert.deepEqual(result.deps, new Map([["dlmalloc", new Set(["0.2.14"])], ["hashbrown", new Set(["0.17.1"])]]));
  assert.deepEqual(result.registry, new Map([["serde", new Set(["1.0.0", "1.0.1"])]]));
  assert.deepEqual(result.members, new Set(["crates/coreav-types", "tools/test-tool"]));
  assert.deepEqual(result.roots, new Set());
});

test("source scanning handles separate printable paths and refuses unknown or traversing paths without disclosure", () => {
  const result = evidence(["panic at crates/coreav-types/src/lib.rs and tools/test-tool/src/main.rs"]);
  assert.deepEqual(result.members, new Set(["crates/coreav-types", "tools/test-tool"]));
  for (const path of ["/private/person/secret.rs", "/x/y.rs", "C:\\Users\\person/src/file.rs", "/rustc/bad/library/core/src/lib.rs", `/rustc/${commit}/library/core/../../secret.rs`, "/rust/deps/unknown/src/file.rs", "/cargo/registry/index/pkg-not-a-version/src/file.rs", "elsewhere/src/file.rs"]) {
    assert.throws(() => evidence([path]), error => {
      assert.equal(error.message, "module carries an unclassified source path");
      assert.ok(!error.message.includes(path));
      return true;
    });
  }
  assert.throws(() => evidence([`/rustc/${commit}/library/core/src/lib.rs /private/person/secret.rs`]), /unclassified/);
});

test("v0 and legacy extraction takes the length-delimited owner root, not instantiating crates or identifiers", () => {
  const result = evidence([], [
    "_RNvCs1a_8dlmalloc4free",
    "_ZN4core3fmt5write17h0123456789abcdefE",
    "_RNvC7___rustc4shim",
    "_RNvMs1C9_NtCsC4_4core3fmtCs1a_7unknown",
    "_RNCCs1a_3std4call",
    "_RINvC3foo9C6forgedCs1a_7unknown",
    "_ZN12coreav_types4mainE",
    "__udivti3", "memcmp", "round", "wasm_bindgen_export", "plain_C4core",
  ]);
  assert.deepEqual(result.roots, new Set(["dlmalloc", "core", "__rustc", "std", "foo", "coreav_types"]));
  for (const symbol of ["_R", "_RC999short", "_RC0", "_RCsbad!_4core", "_ZN999short", "_RNC", "_RC4bad!"]) {
    assert.throws(() => evidence([], [symbol]), /malformed Rust symbol/);
  }
});

test("WebAssembly parsing fails closed on missing or empty raw function names", () => {
  const empty = module();
  assert.throws(() => moduleEvidence(empty, empty), /keep profile.edge strip = "debuginfo"/);
  assert.throws(() => moduleEvidence(module(names("_RC4core")), empty), /no function names/);
  assert.throws(() => moduleEvidence(empty, module(names())), /no function names/);
  assert.throws(() => moduleEvidence(empty, module(names(""))), /no function names/);
  assert.throws(() => moduleEvidence(empty, module(section(0, Buffer.concat([text("\ufeffname"), nameMap([[0, "_RC4core"]])])))), /no function names/);
  assert.doesNotThrow(() => moduleEvidence(empty, module(names("__udivti3"))));
  assert.deepEqual(evidence([], ["\ufeff_RC4core"]).roots, new Set());
});

test("WebAssembly section and subsection readers bound ULEB32, counts, UTF-8 and payloads", () => {
  const empty = module();
  const invalid = [
    Buffer.from([0, 97, 115, 109]),
    module(Buffer.from([0, 128])),
    module(Buffer.from([0, 128, 128, 128, 128, 16])),
    module(Buffer.from([0, 128, 128, 128, 128, 128, 0])),
    module(Buffer.from([0, 127, 0])),
    module(section(0, Buffer.from([1, 255]))),
    module(section(0, Buffer.concat([text("name"), Buffer.from([1, 127, 0])]))),
    module(section(0, Buffer.concat([text("name"), section(1, u32(0xffffffff))]))),
    module(section(0, Buffer.concat([text("name"), nameMap([[0, Buffer.from([1, 255])]])]))),
    module(section(0, Buffer.concat([text("name"), nameMap([[0, Buffer.from([127, 65])]])]))),
    module(section(0, Buffer.concat([text("name"), section(1, Buffer.from([0, 0]))]))),
    module(names("a"), names("b")),
    module(section(0, Buffer.concat([text("name"), nameMap([[0, "a"]]), nameMap([[1, "b"]])]))),
    module(section(0, Buffer.concat([text("name"), nameMap([[0, "a"], [0, "b"]])]))),
    module(section(14, Buffer.alloc(0))),
  ];
  for (const raw of invalid) assert.throws(() => moduleEvidence(empty, raw), /malformed WebAssembly/);
  assert.throws(() => moduleEvidence(Buffer.from("not a module"), module(names("a"))), /module header/);
});

test("coverage is a superset with normalised symbol names and exact path identities", () => {
  const result = evidence([
    `/rustc/${commit}/library/core/src/lib.rs`, `/rustc/${commit}/library/stdarch/src/lib.rs`,
    "/rust/deps/dlmalloc-0.2.14/src/lib.rs", "/cargo/registry/index/serde-1.0.0/src/lib.rs", "crates/coreav-types/src/lib.rs",
  ], ["_RC4core", "_RC12coreav_types", "_RC12wasm_bindgen"]);
  assert.doesNotThrow(() => validateEvidence(result, noticeSections(notices() + notice("unused 9.0.0")), context));
  assert.throws(() => validateEvidence(result, noticeSections(notices()), { ...context, commit: "b".repeat(40) }), /differs from the pinned commit/);
  assert.throws(() => validateEvidence(result, noticeSections(notices()), { ...context, version: "1.2.4" }), /Rust library core/);
  assert.throws(() => validateEvidence(result, noticeSections(notices()), { ...context, memberPaths: new Set() }), /unknown workspace source path/);
  assert.throws(() => validateEvidence(result, noticeSections(notices()), { ...context, library: new Set() }), /unknown Rust library source path/);
  assert.throws(() => validateEvidence(evidence([], ["_RC5rogue"]), noticeSections(notices()), context), /crate root rogue/);
  assert.throws(() => validateEvidence(evidence([], ["_RC6unwind"]), noticeSections(notices()), context), /crate root unwind/);
  const unknown = evidence([`/rustc/${commit}/library/sensitive_directory/src/file.rs`]);
  assert.throws(() => validateEvidence(unknown, noticeSections(notices()), context), error => {
    assert.equal(error.message, "module carries an unknown Rust library source path");
    return true;
  });
});

test("vendored and registry evidence cannot borrow another origin or version", () => {
  const vendored = evidence(["/rust/deps/dlmalloc-0.2.14/src/lib.rs"]);
  assert.throws(() => validateEvidence(vendored, noticeSections(notice("dlmalloc 0.2.14")), context), /vendored crate dlmalloc 0.2.14/);
  assert.throws(() => validateEvidence(vendored, noticeSections(notice("dlmalloc 0.2.15", "Rust toolchain, crates.io")), context), /vendored crate dlmalloc 0.2.14/);
  const registry = evidence(["/cargo/registry/index/serde-1.0.0/src/lib.rs"]);
  assert.throws(() => validateEvidence(registry, noticeSections(notice("serde 1.0.0", "Rust toolchain, crates.io")), context), /registry crate serde 1.0.0/);
  assert.doesNotThrow(() => validateEvidence(vendored, noticeSections(notices()), context));
});

test("compiler shims require the pinned standard-library section even if other names claim coverage", () => {
  const result = evidence([], ["_RC7___rustc"]);
  assert.throws(() => validateEvidence(result, noticeSections(notice("__rustc 1.0.0")), { ...context, members: new Set(["__rustc"]) }), /crate root __rustc/);
  assert.doesNotThrow(() => validateEvidence(result, noticeSections(standard()), context));
  assert.throws(() => validateEvidence(result, noticeSections(standard().replace(commit, "b".repeat(40))), context), /crate root __rustc/);
});

test("compiler-builtins source paths require their own toolchain section", () => {
  const result = evidence([`/rustc/${commit}/library/compiler-builtins/src/lib.rs`]);
  assert.throws(() => validateEvidence(result, noticeSections(standard()), context), /Rust library compiler-builtins/);
  assert.throws(() => validateEvidence(result, noticeSections(standard() + notice("compiler_builtins 0.1.160")), context), /Rust library compiler-builtins/);
  assert.doesNotThrow(() => validateEvidence(result, noticeSections(notice("compiler_builtins 0.1.160", "Rust toolchain")), context));
});

test("removing all toolchain sections exposes each previously uncovered artifact root", () => {
  const result = evidence(["/rust/deps/dlmalloc-0.2.14/src/lib.rs", "/rust/deps/hashbrown-0.17.1/src/raw.rs"], ["_RC4core", "_RC5alloc", "_RC3std", "_RC7___rustc", "_RC8dlmalloc", "_RC9hashbrown", "_RC17compiler_builtins"]);
  assert.throws(() => validateEvidence(result, noticeSections(notice("serde 1.0.0")), context), error => {
    for (const root of ["core", "alloc", "std", "__rustc", "dlmalloc", "hashbrown", "compiler_builtins"]) assert.ok(error.message.includes(`crate root ${root} `));
    return true;
  });
});

test("source traversal may stay within an owner but cannot leave it", () => {
  const prefix = "/rustc/" + commit + "/library/";
  const accepted = prefix + "std/src/sys/pal/wasi/../unix/time.rs";
  for (const pair of [
    [module(paths(accepted)), module(names("__udivti3"))],
    [module(), module(names("__udivti3"), paths(accepted))],
  ]) assert.deepEqual(moduleEvidence(...pair).library, new Set(["std"]));
  for (const path of [accepted, accepted.replaceAll("/", "\\")]) {
    assert.deepEqual(nativeEvidence(Buffer.from(path + "\0")).library, new Set(["std"]));
  }
  for (const path of [
    prefix + "core/../../vendor/dlmalloc-0.2.14/src/lib.rs",
    prefix + "core/../vendor/a-1.0.0/x.rs",
    prefix + "std/src/../../core/x.rs",
    prefix + "vendor/dlmalloc-0.2.14/src/../../hashbrown-0.17.1/x.rs",
    "/cargo/registry/index/serde-1.0.0/../ring-0.17.14/src/lib.rs",
    "/rust/deps/hashbrown-0.17.1/../dlmalloc-0.2.14/src/lib.rs",
    "crates/coreav-cli/../../etc/x.rs",
    prefix + "vendor/../std/x.rs",
    "/cargo/registry/../serde-1.0.0/src/lib.rs",
  ]) {
    for (const read of [() => evidence([path]), () => nativeEvidence(Buffer.from(path + "\0"))]) {
      assert.throws(read, error => {
        assert.equal(error.message, "module carries an unclassified source path");
        assert.ok(!error.message.includes(commit));
        return true;
      });
    }
  }
});

test("native owner traversal stops at a Rust filename, not concatenated message text", () => {
  const prefix = "/rustc/" + commit + "/library/";
  const path = prefix + "std/src/sys/pal/wasi/../unix/time.rs";
  assert.deepEqual(nativeEvidence(Buffer.from("xx" + path + "assertion failed: a/../b\0")).library, new Set(["std"]));
  assert.throws(() => nativeEvidence(Buffer.from(path + prefix + "core/../../vendor/a-1.0.0/x.rs\0")), /unclassified/);
});

test("only the included std backtrace source alias crosses a library directory", () => {
  const prefix = "/rustc/" + commit + "/library/";
  for (const file of ["symbolize/gimli.rs", "symbolize/gimli/lru.rs", "symbolize/mod.rs", "symbolize/gimli/libs_macos.rs", "backtrace/libunwind.rs"]) {
    const path = prefix + "std/src/../../backtrace/src/" + file;
    const observed = evidence([path]);
    assert.deepEqual(observed.library, new Set(["backtrace"]));
    assert.deepEqual(nativeEvidence(Buffer.from("rodata" + path + "message\0")).library, new Set(["backtrace"]));
    const known = { ...context, library: new Set([...context.library, "backtrace"]) };
    validateEvidence(observed, noticeSections(standard()), known);
    assert.throws(() => validateEvidence(observed, new Map(), known), /does not cover/);
  }
  for (const tail of [
    "std/src/../../vendor/dlmalloc-0.2.14/src/lib.rs",
    "std/src/../../compiler-builtins/src/lib.rs",
    "std/src/../../core/x.rs",
    "std/src/../../backtrace/x.rs",
    "std/src/../../backtrace/src/../../vendor/a-1.0.0/x.rs",
    "std/src/../../backtrace/src/../../core/x.rs",
    "std/src/../../backtrace/src/../../../x.rs",
    "std/../backtrace/src/x.rs",
    "core/src/../../backtrace/src/x.rs",
    "alloc/src/../../backtrace/src/x.rs",
    "std/src/./../../backtrace/src/x.rs",
    "std/a.rs/../../vendor/a-1.0.0/x.c",
  ]) {
    for (const read of [() => evidence([prefix + tail]), () => nativeEvidence(Buffer.from(prefix + tail + "\0"))]) {
      assert.throws(read, error => {
        assert.equal(error.message, "module carries an unclassified source path");
        return true;
      });
    }
  }
});

test("a URL-host rs suffix does not hide another unclassified source path", () => {
  const path = "/rustc/" + commit + "/library/std/src/lib.rs";
  for (const value of [
    "peer closed connection without sending TLS close_notify: https://docs.rs/rustls/latest/rustls/manual/_03_howto/index.html#unexpected-eof",
    "https://docs.rs/",
    "https://docs.rs",
    "https://docs.rs:443/x",
    "https://docs.rs/" + path,
  ]) nativeEvidence(Buffer.from(value + "\0"));
  for (const value of [
    "https://docs.rs/ x/y.rs",
    "https://docs.rs/x/y.rs",
    "/home/u/y.rs https://docs.rs/",
    "https://docs.rs/blob/x.rs",
    "https://docs.rs.example.com/x",
    "/home/u/https://evil.rs/x",
    "xhttps://docs.rs/x",
    "https://docs.rs ",
    "docs.rs/rustls",
  ]) assert.throws(() => nativeEvidence(Buffer.from(value + "\0")), /unclassified/);
});
