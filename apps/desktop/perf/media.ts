import { PERF_MEDIA_PATH, PERF_ORIGIN } from "./scenes";

const PNG_1x1_BASE64 =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

export type MediaKind = "svg" | "png";

export function mediaKindForId(id: number): MediaKind {
  return id % 2 === 0 ? "png" : "svg";
}

export function mediaToken(id: number): string {
  return `perf-img-${id}`;
}

export function mediaHref(id: number, origin = PERF_ORIGIN): string {
  const kind = mediaKindForId(id);
  return `${origin.replace(/\/$/, "")}${PERF_MEDIA_PATH}/${id}.${kind}`;
}

export function isLoopbackMediaHref(href: string): boolean {
  try {
    const url = new URL(href);
    return (
      url.protocol === "http:" &&
      url.hostname === "127.0.0.1" &&
      url.pathname.startsWith(`${PERF_MEDIA_PATH}/`)
    );
  } catch {
    return false;
  }
}

export function looksLikeRemoteImageUrl(value: string): boolean {
  if (!/^https?:\/\//i.test(value)) return false;
  try {
    const url = new URL(value);
    if (url.hostname === "127.0.0.1" || url.hostname === "localhost") return false;
    return true;
  } catch {
    return true;
  }
}

export function svgBody(id: number): string {
  return `<svg xmlns="http://www.w3.org/2000/svg" width="48" height="32"><rect width="48" height="32" fill="#4d7c8a"/><text x="24" y="20" text-anchor="middle" fill="#fff" font-size="12">${id}</text></svg>`;
}

export function pngBytes(): Uint8Array {
  const binary = globalThis.atob(PNG_1x1_BASE64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

export function mediaResponse(pathname: string): { contentType: string; body: string | Uint8Array } | null {
  const match = pathname.match(new RegExp(`${PERF_MEDIA_PATH}/(\\d+)\\.(svg|png)$`));
  if (!match) return null;
  const id = Number(match[1]);
  const kind = match[2] as MediaKind;
  if (kind === "svg") {
    return { contentType: "image/svg+xml; charset=utf-8", body: svgBody(id) };
  }
  return { contentType: "image/png", body: pngBytes() };
}
