import { spawnSync } from "node:child_process";
import { appendFileSync, readFileSync } from "node:fs";
import { isAbsolute, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { failCapture } from "./ledger.mjs";
import { canonical, sha } from "./roster.mjs";

export function webpPixelParity(packageRecord) {
  const versions = packageRecord.evidence?.registry?.find(row => row.name === "image-webp")?.versions;
  if (!Array.isArray(versions) || versions.length !== 1 || typeof versions[0] !== "string") failCapture("WebP dependency identity");
  return versions[0] === "0.2.4";
}

export function pngAnchor(row) {
  const options = { ...row.options, format: "png" };
  return { id: `png-anchor-${sha(canonical([row.fixture, options]))}`, fixture: row.fixture, options, expectation: "success" };
}

export function compareSamples(actual, native, anchor) {
  for (const picture of [actual, native, anchor]) {
    if (!Number.isSafeInteger(picture?.width) || !Number.isSafeInteger(picture.height) || picture.width <= 0 || picture.height <= 0 || picture.width > 1280 || picture.height > 1280 || picture.width * picture.height > 921600 || picture.pixelFormat !== "rgba" || !Buffer.isBuffer(picture.samples) || picture.samples.length !== picture.width * picture.height * 4) failCapture("decoded pixel shape");
    if (picture.width !== actual.width || picture.height !== actual.height || picture.pixelFormat !== actual.pixelFormat) failCapture("decoded pixel geometry");
  }
  if (!actual.samples.equals(native.samples) || !actual.samples.equals(anchor.samples)) failCapture("decoded pixel samples");
}

export function createPixelOracle(directory, output, run = spawnSync) {
  const root = fileURLToPath(new URL("../../../..", import.meta.url));
  const checked = run("python3", ["-I", resolve(root, "scripts/test_oracle.py"), "--resolve", "--ffmpeg", join(directory, "ffmpeg"), "--ffprobe", join(directory, "ffprobe")], { timeout: 15000, encoding: "utf8" });
  appendFileSync(join(output, "pixel-oracle.log"), `${checked.stdout ?? ""}${checked.stderr ?? ""}`);
  if (checked.error || checked.status !== 0 || checked.signal) failCapture("pixel oracle identity");
  const paths = String(checked.stdout ?? "").replace(/\n$/, "").split("\n");
  if (paths.length !== 2 || paths.some(path => !isAbsolute(path) || path.includes("\r"))) failCapture("pixel oracle identity");
  const programs = Object.fromEntries(["ffmpeg", "ffprobe"].map((name, index) => [name, { path: paths[index], sha256: sha(readFileSync(paths[index])) }]));
  function execute(name, args, input, maxBuffer) {
    const program = programs[name];
    if (sha(readFileSync(program.path)) !== program.sha256) failCapture("pixel oracle changed");
    const result = run(program.path, args, { input, timeout: 15000, maxBuffer });
    appendFileSync(join(output, "pixel-oracle.log"), JSON.stringify({ program: name, programSha256: program.sha256, args, status: result.status, signal: result.signal, error: result.error?.code ?? null, stderr: String(result.stderr ?? "") }) + "\n");
    if (result.error?.code === "ETIMEDOUT") failCapture("pixel oracle timeout");
    if (result.error || result.status !== 0 || result.signal || !Buffer.isBuffer(result.stdout) || String(result.stderr ?? "").trim()) failCapture(name === "ffprobe" ? "pixel probe failed" : "pixel decode failed");
    if (sha(readFileSync(program.path)) !== program.sha256) failCapture("pixel oracle changed");
    return result.stdout;
  }
  function decode(record) {
    const encoded = readFileSync(record.image);
    const geometry = record.summary?.output;
    if (encoded.length !== record.bytes || sha(encoded) !== record.outputSha256) failCapture("pixel encoded binding");
    if (!Number.isSafeInteger(geometry?.width) || !Number.isSafeInteger(geometry.height) || geometry.width <= 0 || geometry.height <= 0 || geometry.width > 1280 || geometry.height > 1280 || geometry.width * geometry.height > 921600 || !["png", "webp"].includes(geometry.format)) failCapture("decoded pixel shape");
    let probe;
    try { probe = JSON.parse(execute("ffprobe", ["-v", "error", "-show_entries", "stream=codec_name,width,height,pix_fmt", "-of", "json", "-i", "pipe:0"], encoded, 1 << 20)); }
    catch (error) { if (error instanceof SyntaxError) failCapture("pixel probe malformed"); throw error; }
    if (probe?.streams?.length !== 1 || probe.streams[0]?.codec_name !== geometry.format || probe.streams[0].width !== geometry.width || probe.streams[0].height !== geometry.height) failCapture("decoded pixel geometry");
    if (!["rgb24", "rgba", "argb", "bgra", "abgr"].includes(probe.streams[0].pix_fmt)) failCapture("decoded pixel format");
    const size = geometry.width * geometry.height * 4;
    const samples = execute("ffmpeg", ["-nostdin", "-hide_banner", "-v", "error", "-xerror", "-noautorotate", "-i", "pipe:0", "-map", "0:v:0", "-an", "-sn", "-dn", "-c:v", "rawvideo", "-pix_fmt", "rgba", "-f", "rawvideo", "pipe:1"], encoded, size + 1);
    if (samples.length !== size) failCapture("decoded pixel count");
    return { width: geometry.width, height: geometry.height, pixelFormat: "rgba", bytes: size, sha256: sha(samples), encodedSha256: record.outputSha256, ffmpegSha256: programs.ffmpeg.sha256, ffprobeSha256: programs.ffprobe.sha256, samples };
  }
  return {
    compare(actual, native, anchor) {
      const a = decode(actual), n = decode(native), p = decode(anchor.actual);
      compareSamples(a, n, p);
      const metadata = ({ samples, ...record }) => record;
      return { actual: metadata(a), native: metadata(n), png: { actual: anchor.actual, native: anchor.native, pixels: metadata(p) } };
    },
    verify() {
      for (const program of Object.values(programs)) if (sha(readFileSync(program.path)) !== program.sha256) failCapture("pixel oracle changed");
    },
  };
}
