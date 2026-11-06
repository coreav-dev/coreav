import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { compareSamples, createPixelOracle, pngAnchor, webpPixelParity } from "./pixels.mjs";
import { failureRecord, validateParity } from "./ledger.mjs";
import { optionsSha256, sha } from "./roster.mjs";

const hash = "a".repeat(64), other = "b".repeat(64), pngHash = "c".repeat(64);
const samples = Buffer.from([12, 34, 56, 255, 78, 90, 123, 255]);
const picture = bytes => ({ width: 2, height: 1, pixelFormat: "rgba", samples: Buffer.from(bytes) });

test("pixel executions retain both resolver paths instead of reconstructing the requested directory", () => {
  const directory = mkdtempSync(join(tmpdir(), "thumbnail-selection-"));
  try {
    const ffmpeg = join(directory, "qualified-decoder"), ffprobe = join(directory, "qualified-probe");
    for (const path of [ffmpeg, ffprobe]) writeFileSync(path, "synthetic executable identity");
    const image = join(directory, "image"); writeFileSync(image, "synthetic image identity");
    const bytes = Buffer.from("synthetic image identity"), calls = [];
    const record = { image, bytes: bytes.length, outputSha256: sha(bytes), summary: { output: { width: 2, height: 1, format: "png" } } };
    const run = (program, args) => {
      calls.push(program);
      if (program === "python3") return { status: 0, stdout: `${ffmpeg}\n${ffprobe}\n` };
      return { status: 0, stdout: program === ffprobe ? Buffer.from(JSON.stringify({ streams: [{ codec_name: "png", width: 2, height: 1, pix_fmt: "rgba" }] })) : samples };
    };
    createPixelOracle(join(directory, "requested"), directory, run).compare(record, record, { actual: record, native: record });
    assert.deepEqual(calls, ["python3", ffprobe, ffmpeg, ffprobe, ffmpeg, ffprobe, ffmpeg]);
    for (const stdout of ["", `${ffmpeg}\n`, `relative\n${ffprobe}\n`, `${ffmpeg}\n${ffprobe}\nextra\n`, `${ffmpeg}\r\n${ffprobe}\n`]) {
      assert.throws(() => createPixelOracle(directory, directory, () => ({ status: 0, stdout })), /identity/);
    }
  } finally { rmSync(directory, { recursive: true, force: true }); }
});

test("every changed WebP sample fails, including alpha and two equally wrong outputs", () => {
  compareSamples(picture(samples), picture(samples), picture(samples));
  for (let index = 0; index < samples.length; index += 1) {
    const changed = Buffer.from(samples); changed[index] ^= 1;
    assert.throws(() => compareSamples(picture(changed), picture(samples), picture(samples)), /samples/);
    assert.throws(() => compareSamples(picture(changed), picture(changed), picture(samples)), /samples/);
  }
  for (const change of [{ width: 1 }, { height: 2 }, { pixelFormat: "bgra" }, { samples: samples.subarray(1) }, { samples: Buffer.concat([samples, Buffer.from([0])]) }]) {
    assert.throws(() => compareSamples({ ...picture(samples), ...change }, picture(samples), picture(samples)), /shape|geometry/);
  }
});

test("only the audited registry version selects pixel parity", () => {
  const record = version => ({ evidence: { registry: [{ name: "image-webp", versions: [version] }] } });
  assert.equal(webpPixelParity(record("0.2.4")), true);
  for (const version of ["0.2.5", "0.3.0", "0.2.4-fixed"]) assert.equal(webpPixelParity(record(version)), false);
  assert.throws(() => webpPixelParity({}), /dependency identity/);
  assert.throws(() => webpPixelParity({ evidence: { registry: [{ name: "image-webp", versions: ["0.2.4", "0.2.5"] }] } }), /dependency identity/);
});

function parity() {
  const options = { at: "1", format: "webp", width: 2, stream: 1 };
  const summary = { schema: "coreav.thumbnail/1", packet: { offset: 1 }, time: { frame: "1" }, decoder: { name: "target", backend: "native" }, output: { bytes: 5, width: 2, height: 1, format: "webp" } };
  const actual = { artifactSha256: hash, complete: true, runtimeExit: false, outcome: "success", inputSha256: hash, optionsSha256: optionsSha256(options), bytes: 5, outputSha256: other, summary };
  const native = structuredClone(actual); native.outputSha256 = hash;
  const anchorRow = pngAnchor({ fixture: "fixture", options });
  const anchor = { ...structuredClone(actual), optionsSha256: optionsSha256(anchorRow.options), bytes: 7, outputSha256: pngHash, summary: { ...structuredClone(summary), output: { ...summary.output, bytes: 7, format: "png" } } };
  const decoded = encoded => ({ width: 2, height: 1, pixelFormat: "rgba", bytes: 8, sha256: sha(samples), encodedSha256: encoded, ffmpegSha256: hash, ffprobeSha256: other });
  return { actual, native, expected: { options, inputSha256: hash, expectation: "success" }, evidence: { actual: decoded(other), native: decoded(hash), png: { actual: anchor, native: structuredClone(anchor), pixels: decoded(pngHash) } } };
}

test("WebP accepts different encoded bytes only with complete bound pixels and a byte-exact PNG anchor", () => {
  const { actual, native, expected, evidence } = parity();
  assert.throws(() => validateParity(actual, native, expected), /image bytes/);
  validateParity(actual, native, expected, evidence);
  for (const mutate of [
    e => { e.actual.sha256 = hash; }, e => { e.native.sha256 = hash; }, e => { e.png.pixels.sha256 = hash; },
    e => { e.actual.encodedSha256 = hash; }, e => { e.native.ffmpegSha256 = other; }, e => { e.png.pixels.ffprobeSha256 = hash; },
    e => { e.actual.width = 1; }, e => { e.actual.pixelFormat = "rgb24"; }, e => { e.actual.bytes = 7; },
    e => { e.png.actual.inputSha256 = other; }, e => { e.png.actual.optionsSha256 = other; }, e => { e.png.actual.artifactSha256 = other; },
    e => { e.png.native.outputSha256 = other; }, e => { e.png.native.complete = false; },
    e => { e.png.actual.summary.packet.offset = 2; e.png.native.summary.packet.offset = 2; },
  ]) {
    const changed = structuredClone(evidence); mutate(changed);
    assert.throws(() => validateParity(actual, native, expected, changed), /invalid/);
  }
  const changed = structuredClone(actual); changed.summary.time.frame = "0";
  assert.throws(() => validateParity(changed, native, expected, evidence), /picture|summary/);
  for (const format of ["jpeg", "png"]) {
    const a = structuredClone(actual), n = structuredClone(native), wanted = { ...expected, options: { ...expected.options, format } };
    a.optionsSha256 = n.optionsSha256 = optionsSha256(wanted.options);
    a.summary.output.format = n.summary.output.format = format;
    assert.throws(() => validateParity(a, n, wanted), /image bytes/);
    assert.throws(() => validateParity(a, n, wanted, evidence), /pixel exception format/);
  }
});

test("PNG anchor identity preserves every option other than format", () => {
  const row = { fixture: "one", options: { at: "1", format: "webp", stream: 1, width: 123, chroma: "full", filter: "area" } };
  const anchor = pngAnchor(row);
  assert.deepEqual(anchor.options, { ...row.options, format: "png" });
  assert.equal(pngAnchor({ ...row, options: { ...row.options, format: "jpeg" } }).id, anchor.id);
  for (const changed of [{ ...row, fixture: "two" }, { ...row, options: { ...row.options, stream: 0 } }, { ...row, options: { ...row.options, width: 124 } }]) assert.notEqual(pngAnchor(changed).id, anchor.id);
});

test("the oracle verifies supplied binaries and compares complete unquantized RGBA samples", () => {
  const directory = mkdtempSync(join(tmpdir(), "thumbnail-pixels-"));
  try {
    const qualified = join(directory, "qualified");
    mkdirSync(qualified);
    for (const name of ["ffmpeg", "ffprobe"]) writeFileSync(join(qualified, name), `fake ${name}, never executed`);
    const record = (name, format) => {
      const image = join(directory, name), bytes = Buffer.from(name); writeFileSync(image, bytes);
      return { image, bytes: bytes.length, outputSha256: sha(bytes), summary: { output: { width: 2, height: 1, format } } };
    };
    const actual = record("webp-actual", "webp"), native = record("webp-native", "webp"), anchor = record("png-anchor", "png");
    let mutation = "", calls = [];
    const run = (program, args, options) => {
      calls.push({ program, args, options });
      assert.equal(options.timeout, 15000);
      if (program === "python3") return { status: mutation === "identity" ? 1 : 0, stdout: `${join(qualified, "ffmpeg")}\n${join(qualified, "ffprobe")}\n`, stderr: "" };
      if (program.endsWith("ffprobe")) return { status: mutation === "probe-failure" ? 1 : 0, stdout: Buffer.from(mutation === "malformed" ? "{" : JSON.stringify(mutation === "null" ? null : { streams: [{ codec_name: options.input.toString().startsWith("png") ? "png" : "webp", width: mutation === "geometry" ? 3 : 2, height: 1, pix_fmt: mutation === "format" ? "rgb48le" : "rgba" }] })) };
      const decoded = Buffer.from(samples);
      if (mutation === "sample" && options.input.toString() === "webp-actual") decoded[0] ^= 1;
      if (mutation === "timeout") return { status: null, error: { code: "ETIMEDOUT" } };
      if (mutation === "failure") return { status: 1, stdout: decoded, stderr: Buffer.from("private-marker") };
      return { status: 0, stdout: mutation === "extra" ? Buffer.concat([decoded, decoded]) : mutation === "truncated" ? decoded.subarray(1) : decoded, stderr: mutation === "diagnostic" ? Buffer.from("private-marker") : Buffer.alloc(0) };
    };
    const oracle = createPixelOracle(directory, directory, run);
    const result = oracle.compare(actual, native, { actual: anchor, native: anchor });
    assert.equal(result.actual.sha256, sha(samples));
    assert.deepEqual(calls[0].args.slice(-5), ["--resolve", "--ffmpeg", join(directory, "ffmpeg"), "--ffprobe", join(directory, "ffprobe")]);
    assert.ok(calls.slice(1).every(call => [join(qualified, "ffmpeg"), join(qualified, "ffprobe")].includes(call.program)));
    assert.ok(calls.filter(c => c.program.endsWith("ffmpeg")).every(c => c.args.includes("-xerror") && c.args.includes("-noautorotate") && c.args.includes("rgba") && !c.args.includes("-frames:v")));
    for (const change of ["sample", "geometry", "format", "extra", "truncated", "malformed", "null", "probe-failure", "timeout", "failure", "diagnostic"]) {
      mutation = change;
      let failure;
      try { oracle.compare(actual, native, { actual: anchor, native: anchor }); } catch (error) { failure = error; }
      assert.ok(failure, change);
      assert.ok(!JSON.stringify(failureRecord("pixel-oracle", 1, failure)).includes("private-marker"));
    }
    mutation = "";
    assert.throws(() => oracle.compare({ ...actual, outputSha256: hash }, native, { actual: anchor, native: anchor }), /binding/);
    mutation = "identity";
    assert.throws(() => createPixelOracle(directory, directory, run), /identity/);
    writeFileSync(join(qualified, "ffmpeg"), "changed binary");
    assert.throws(() => oracle.verify(), /changed/);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});
