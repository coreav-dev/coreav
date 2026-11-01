/**
 * Small, deterministic ISO fixtures for stream-copy operations, not decoding.
 * Box layouts follow ISO/IEC 14496-12/14/15 and 3GPP TS 26.245, using this
 * repository's Rust movies and text_movie test builders. Sample payloads
 * contain H.264 parameter sets and the slice headers the cut rules inspect,
 * or opaque AAC. B-frame reordering is represented by the composition table.
 */
const utf8 = new TextEncoder();
const text = (value) => utf8.encode(value);
const zeros = (length) => new Uint8Array(length);

function cat(...parts) {
  const bytes = new Uint8Array(parts.reduce((sum, part) => sum + part.length, 0));
  let at = 0;
  for (const part of parts) {
    bytes.set(part, at);
    at += part.length;
  }
  return bytes;
}

function u16(...values) {
  const bytes = zeros(values.length * 2);
  const view = new DataView(bytes.buffer);
  values.forEach((value, i) => view.setUint16(i * 2, value));
  return bytes;
}

function u32(...values) {
  const bytes = zeros(values.length * 4);
  const view = new DataView(bytes.buffer);
  values.forEach((value, i) => view.setUint32(i * 4, value));
  return bytes;
}

function u64(value) {
  const bytes = zeros(8);
  new DataView(bytes.buffer).setBigUint64(0, value);
  return bytes;
}

function box(kind, ...parts) {
  const payload = cat(...parts);
  return cat(u32(payload.length + 8), text(kind), payload);
}

function full(kind, version, flags, ...parts) {
  return box(kind, u32((version << 24) | flags), ...parts);
}

function ue(value) {
  const bits = (value + 1).toString(2);
  return "0".repeat(bits.length - 1) + bits;
}

function rbsp(...fields) {
  const bits = fields.join("") + "1";
  return Uint8Array.from({ length: Math.ceil(bits.length / 8) }, (_, i) =>
    Number.parseInt(bits.slice(i * 8, i * 8 + 8).padEnd(8, "0"), 2));
}

/** ITU-T H.264 7.3.2.1/2: High, 8-bit 4:2:0, 20 by 15 frame macroblocks. */
function parameterSets() {
  const sps = cat(Uint8Array.of(0x67, 100, 0, 30), rbsp(
    ue(0),
    ue(1), ue(0), ue(0), "00",
    ue(0), ue(0), ue(0),
    ue(2), "0",
    ue(19), ue(14), "11",
    "00",
  ));
  const pps = cat(Uint8Array.of(0x68), rbsp(
    ue(0), ue(0), "00", ue(0),
    ue(0), ue(0), "000",
    ue(0), ue(0), ue(0), "100",
  ));
  return { sps, pps };
}

function video() {
  const { sps, pps } = parameterSets();
  const samples = Array.from({ length: 100 }, (_, n) => {
    const inGop = n % 25;
    const sync = inGop === 0;
    const predicted = inGop % 3 === 1;
    const bytes = new Uint8Array(40 + n % 13).fill(0x55);
    const prefix = sync ? cat(u32(sps.length), sps, u32(pps.length), pps) : zeros(0);
    bytes.set(prefix);
    bytes.set(u32(bytes.length - prefix.length - 4), prefix.length);
    bytes[prefix.length + 4] = sync ? 0x65 : 0x41;
    bytes[prefix.length + 5] = sync ? 0x88 : 0xc0;
    return { bytes, duration: 512, offset: sync ? 1024 : predicted ? 2048 : 512, sync };
  });
  return {
    id: 1, handler: "vide", timescale: 12800, enabled: true,
    edits: [[4000, 1024]], samples,
    entry: box("avc1", zeros(6), u16(1), zeros(16), u16(320, 240), zeros(50),
      box("avcC", Uint8Array.of(1, 100, 0, 30, 0xff, 0xe1), u16(sps.length), sps,
        Uint8Array.of(1), u16(pps.length), pps, Uint8Array.of(0xfd, 0xf8, 0xf8, 0)),
      box("pasp", u32(1, 1)), box("fiel", Uint8Array.of(1, 0))),
  };
}

function audio(id) {
  return {
    id, handler: "soun", timescale: 48000, enabled: true,
    edits: [[4000, 2112]],
    samples: Array.from({ length: 190 }, (_, n) => ({
      bytes: Uint8Array.from({ length: 20 + n % 5 }, (_, b) => (b + n * 7 + 101) & 255),
      duration: 1024, offset: 0, sync: true,
    })),
    entry: box("mp4a", zeros(6), u16(1), zeros(8), u16(2, 16), zeros(4), u32(48000 * 65536),
      full("esds", 0, 0, Uint8Array.of(
        3, 25, 0, 1, 0, 4, 17, 0x40, 0x15, 0, 0, 0, 0, 0, 0xfa, 0,
        0, 0, 0xfa, 0, 5, 2, 0x11, 0x90, 6, 1, 2))),
  };
}

function timedText(id, chapter) {
  const titles = chapter ? ["Opening", "Closing"] : ["Hello, world!", "A second subtitle."];
  const entry = chapter
    ? box("text", zeros(6), u16(1), zeros(4 + 4 + 6 + 8 + 8 + 2 + 2 + 3 + 6 + 1))
    : box("tx3g", zeros(6), u16(1), u32(0), Uint8Array.of(1, 255, 0, 0, 0, 255), zeros(8),
      Uint8Array.of(0, 0, 0, 0, 0, 1, 0, 18, 255, 255, 255, 255),
      box("ftab", u16(1, 1), Uint8Array.of(5), text("Serif")));
  return {
    id, handler: chapter ? "text" : "sbtl", timescale: 1000, enabled: !chapter,
    edits: [], entry,
    samples: titles.map((title) => {
      const bytes = text(title);
      return { bytes: cat(u16(bytes.length), bytes), duration: 2000, offset: 0, sync: true };
    }),
  };
}

function track(spec, chunkOffset) {
  const { samples, timescale, handler } = spec;
  const count = samples.length;
  const mediaDuration = samples.reduce((sum, sample) => sum + sample.duration, 0);
  const duration = spec.edits.length
    ? spec.edits.reduce((sum, edit) => sum + edit[0], 0)
    : Math.floor(mediaDuration * 1000 / timescale);
  const matrix = u32(0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x40000000);
  const tkhd = full("tkhd", 0, spec.enabled ? 3 : 2,
    u32(0, 0, spec.id, 0, duration), zeros(16), matrix,
    handler === "vide" ? u32(320 * 65536, 240 * 65536) : u32(0, 0));
  const tables = [
    full("stsd", 0, 0, u32(1), spec.entry),
    full("stts", 0, 0, u32(count), ...samples.map((sample) => u32(1, sample.duration))),
    full("stsc", 0, 0, u32(1, 1, count, 1)),
    full("stsz", 0, 0, u32(0, count), ...samples.map((sample) => u32(sample.bytes.length))),
    full("stco", 0, 0, u32(1, chunkOffset)),
  ];
  if (samples.some((sample) => sample.offset !== 0)) {
    tables.push(full("ctts", 0, 0, u32(count), ...samples.map((sample) => u32(1, sample.offset))));
  }
  if (samples.some((sample) => !sample.sync)) {
    const sync = samples.flatMap((sample, i) => sample.sync ? [i + 1] : []);
    tables.push(full("stss", 0, 0, u32(sync.length, ...sync)));
  }
  const mediaHeader = handler === "vide" ? full("vmhd", 0, 1, zeros(8))
    : handler === "soun" ? full("smhd", 0, 0, zeros(4)) : full("nmhd", 0, 0);
  const mdia = box("mdia",
    full("mdhd", 0, 0, u32(0, 0, timescale, mediaDuration), Uint8Array.of(0x55, 0xc4, 0, 0)),
    full("hdlr", 0, 0, u32(0), text(handler), zeros(13)),
    box("minf", mediaHeader,
      box("dinf", full("dref", 0, 0, u32(1), full("url ", 0, 1))), box("stbl", ...tables)));
  return box("trak", tkhd,
    ...(spec.chapters ? [box("tref", box("chap", u32(...spec.chapters)))] : []),
    ...(spec.edits.length ? [box("edts", full("elst", 0, 0, u32(spec.edits.length),
      ...spec.edits.map(([length, start]) => u32(length, start, 0x10000))))] : []), mdia);
}

function movie(tracks, largeDuration = false) {
  const ftyp = box("ftyp", text("isom"), u32(512), text("isom"));
  const header = largeDuration
    ? full("mvhd", 1, 0, u64(0n), u64(0n), u32(1000), u64(2n ** 53n + 1n), zeros(80))
    : full("mvhd", 0, 0, u32(0, 0, 1000, 4000), zeros(80));
  const build = (offsets) => box("moov", header, ...tracks.map((spec, i) => track(spec, offsets[i])));
  const placeholder = build(tracks.map(() => 0));
  let offset = ftyp.length + placeholder.length + 8;
  const offsets = tracks.map((spec) => {
    const start = offset;
    offset += spec.samples.reduce((sum, sample) => sum + sample.bytes.length, 0);
    return start;
  });
  return cat(ftyp, build(offsets), box("mdat", ...tracks.flatMap((spec) => spec.samples.map((sample) => sample.bytes))));
}

/** @returns {Map<string, Uint8Array>} Fresh deterministic bytes, without files or codec tools. */
export function fixtures() {
  const withText = video();
  withText.chapters = [4];
  return new Map([
    ["av.mp4", movie([video(), audio(2)])],
    ["av-text.mp4", movie([withText, audio(2), timedText(3, false), timedText(4, true)])],
    ["audio.m4a", movie([audio(1)])],
    ["large-duration.mp4", movie([video(), audio(2)], true)],
    ["not-a-movie.bin", text("RIFF....AVI LIST")],
  ]);
}
