export function registeredBars(markdown) {
  const begin = "<!-- wasm-memory-bars:begin -->", end = "<!-- wasm-memory-bars:end -->";
  if (markdown.split(begin).length !== 2 || markdown.split(end).length !== 2 || markdown.indexOf(begin) > markdown.indexOf(end)) throw new Error("memory registry markers are invalid");
  const block = markdown.slice(markdown.indexOf(begin) + begin.length, markdown.indexOf(end));
  const match = /^\s*```json\r?\n([\s\S]*?)\r?\n```\s*$/.exec(block);
  if (!match) throw new Error("memory registry must contain one JSON block");
  const raw = match[1];
  let at = 0;
  const fail = () => { throw new Error("memory registry JSON is malformed"); };
  const space = () => { while (/[\t\n\r ]/.test(raw[at] ?? "x")) at += 1; };
  function string() {
    const token = /^"(?:[^"\\\u0000-\u001f]|\\(?:["\\/bfnrt]|u[0-9a-fA-F]{4}))*"/.exec(raw.slice(at));
    if (!token) fail();
    at += token[0].length;
    return JSON.parse(token[0]);
  }
  function value(depth) {
    if (depth > 64) fail();
    space();
    if (raw[at] === "{") {
      at += 1; space();
      const names = new Set();
      if (raw[at] !== "}") while (true) {
        space();
        const name = string();
        if (names.has(name)) throw new Error("memory registry has a duplicate property name");
        names.add(name); space();
        if (raw[at++] !== ":") fail();
        value(depth + 1); space();
        if (raw[at] !== ",") break;
        at += 1;
      }
      if (raw[at++] !== "}") fail();
    } else if (raw[at] === "[") {
      at += 1; space();
      if (raw[at] !== "]") while (true) {
        value(depth + 1); space();
        if (raw[at] !== ",") break;
        at += 1;
      }
      if (raw[at++] !== "]") fail();
    } else if (raw[at] === '"') string();
    else {
      const token = /^(?:true|false|null|-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?)/.exec(raw.slice(at));
      if (!token) fail();
      at += token[0].length;
    }
  }
  value(0); space();
  if (at !== raw.length) fail();
  const registry = JSON.parse(raw);
  if (!registry || Object.keys(registry).sort().join(",") !== "gates,predicate,signal" || registry.signal !== "wasm_linear_bytes" || registry.predicate !== "value <= bar") throw new Error("memory registry schema is invalid");
  if (!registry.gates || typeof registry.gates !== "object" || Array.isArray(registry.gates) || !Object.keys(registry.gates).length) throw new Error("memory registry gates are missing");
  const bars = new Map(Object.entries(registry.gates));
  for (const [step, bar] of bars) if (!/^[a-z0-9-]{1,64}$/.test(step) || !Number.isSafeInteger(bar) || bar <= 0 || bar % 65536 !== 0) throw new Error("memory registry gate is invalid");
  return bars;
}
