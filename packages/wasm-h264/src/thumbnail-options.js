const MEDIA = new Map([
  ["jpeg", "image/jpeg"],
  ["jpg", "image/jpeg"],
  ["png", "image/png"],
  ["webp", "image/webp"],
]);

export function thumbnailRequest(options) {
  if (options === null || typeof options !== "object" || Array.isArray(options)) {
    throw new TypeError("options must be an object");
  }
  const { at: at, width: width, height: height, format: format, ...rest } = options;
  const request = { ...rest, at: seconds(at, "at") };
  if (request.at === undefined) {
    throw new TypeError('at must be decimal seconds, such as 12 or "12.5"');
  }
  if (width !== undefined) request.width = width;
  if (height !== undefined) request.height = height;
  if (width === undefined && height === undefined) request.width = 320;
  if (format !== undefined) request.format = format;
  return { request, contentType: contentTypeFor(format) };
}

export function contentTypeFor(format) {
  if (format === undefined) return "image/jpeg";
  return MEDIA.get(format) ?? "image/jpeg";
}

function seconds(value, name) {
  if (value === undefined || value === null) return undefined;
  if (typeof value === "string") return value;
  if (typeof value === "number" && Number.isFinite(value) && value >= 0) {
    const text = String(value);
    if (!/e/i.test(text)) return text;
  }
  throw new TypeError(`${name} must be decimal seconds, such as 12 or "12.5"`);
}
