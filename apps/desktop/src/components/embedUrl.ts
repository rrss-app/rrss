/** 与 `crates/rrss-render/src/embed.rs` 同一套主机名 / 路径规则。不要用子串匹配 youtube.com。 */

import { i18n } from "../i18n";

export type EmbedPresentation =
  | { kind: "youtube" | "bilibili"; openUrl: string }
  | { kind: "video" | "audio"; openUrl: string }
  | { kind: "external"; openUrl: string; label: string };

const VIDEO_EXT = new Set(["mp4", "webm", "ogv"]);
const AUDIO_EXT = new Set(["ogg", "mp3", "m4a", "aac", "wav", "opus"]);
const REJECTED_EXT = new Set(["mkv", "avi", "m3u8"]);

export function classifyEmbed(raw: string, storedKind = ""): EmbedPresentation {
  const normalized = normalize(raw);
  const url = parseHttp(normalized);
  if (!url) return external(normalized || raw);
  const official = officialOf(url);
  if (official) return official;
  const ext = pathExt(url);
  if (REJECTED_EXT.has(ext)) return external(url.toString());
  if (VIDEO_EXT.has(ext)) return { kind: "video", openUrl: url.toString() };
  if (AUDIO_EXT.has(ext)) return { kind: "audio", openUrl: url.toString() };
  if (storedKind === "video") return { kind: "video", openUrl: url.toString() };
  if (storedKind === "audio") return { kind: "audio", openUrl: url.toString() };
  return external(url.toString());
}

export function isLocalMediaHref(href: string): boolean {
  return href.startsWith("rrss://")
    || href.startsWith("http://rrss.localhost/")
    || href.startsWith("https://rrss.localhost/");
}

export function formatDuration(secs: number): string {
  const s = Math.max(0, Math.floor(secs));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const r = s % 60;
  const ss = String(r).padStart(2, "0");
  if (h > 0) return `${h}:${String(m).padStart(2, "0")}:${ss}`;
  return `${m}:${ss}`;
}

export function coverMeta(site: string, author: string, durationSecs: number | null | undefined): string {
  const name = site === "bilibili" ? i18n.t("embed.bilibili", { ns: "reader" }) : "YouTube";
  const parts = [name];
  if (author.trim()) parts.push(author.trim());
  if (site === "bilibili" && durationSecs != null && Number.isFinite(durationSecs)) {
    parts.push(formatDuration(durationSecs));
  }
  return parts.join(" · ");
}

export function siteLabel(raw: string): string {
  const url = parseHttp(normalize(raw));
  if (!url) return i18n.t("embed.openInBrowser", { ns: "reader" });
  const host = url.hostname.replace(/^www\./i, "");
  return host || i18n.t("embed.openInBrowser", { ns: "reader" });
}

function officialOf(url: URL): EmbedPresentation | null {
  const host = url.hostname.toLowerCase();
  if (hostIs(host, "youtube.com")) {
    const id = youtubeId(url);
    return id ? { kind: "youtube", openUrl: `https://www.youtube.com/watch?v=${id}` } : null;
  }
  if (hostIs(host, "youtu.be")) {
    const id = validYt(url.pathname.split("/").filter(Boolean)[0] ?? "");
    return id ? { kind: "youtube", openUrl: `https://www.youtube.com/watch?v=${id}` } : null;
  }
  if (hostIs(host, "youtube-nocookie.com")) {
    const parts = url.pathname.split("/").filter(Boolean);
    if (parts[0] !== "embed") return null;
    const id = validYt(parts[1] ?? "");
    return id ? { kind: "youtube", openUrl: `https://www.youtube.com/watch?v=${id}` } : null;
  }
  if (host === "player.bilibili.com") {
    if (url.pathname !== "/player.html") return null;
    const id = normalizeBv(url.searchParams.get("bvid") ?? "");
    return id ? { kind: "bilibili", openUrl: `https://www.bilibili.com/video/${id}/` } : null;
  }
  if (hostIs(host, "bilibili.com")) {
    const parts = url.pathname.split("/").filter(Boolean);
    if (parts[0] !== "video") return null;
    const id = normalizeBv(parts[1] ?? "");
    return id ? { kind: "bilibili", openUrl: `https://www.bilibili.com/video/${id}/` } : null;
  }
  return null;
}

function youtubeId(url: URL): string | null {
  const parts = url.pathname.split("/").filter(Boolean);
  if (parts[0] === "watch") return validYt(url.searchParams.get("v") ?? "");
  if (parts[0] === "embed" || parts[0] === "shorts") return validYt(parts[1] ?? "");
  return null;
}

function validYt(raw: string): string | null {
  if (!raw || raw.length > 64 || !/^[A-Za-z0-9_-]+$/.test(raw)) return null;
  return raw;
}

function normalizeBv(raw: string): string | null {
  const id = raw.trim().replace(/\/$/, "");
  if (!/^bv[0-9A-Za-z]+$/i.test(id)) return null;
  return `BV${id.slice(2)}`;
}

function hostIs(host: string, domain: string): boolean {
  return host === domain || host.endsWith(`.${domain}`);
}

function pathExt(url: URL): string {
  const seg = url.pathname.split("/").pop() ?? "";
  const dot = seg.lastIndexOf(".");
  if (dot <= 0) return "";
  const ext = seg.slice(dot + 1).toLowerCase();
  return /^[a-z0-9]{1,8}$/.test(ext) ? ext : "";
}

function normalize(raw: string): string {
  const trimmed = raw.trim();
  if (trimmed.startsWith("//")) return `https:${trimmed}`;
  return trimmed;
}

function parseHttp(raw: string): URL | null {
  try {
    const url = new URL(raw);
    if (url.protocol !== "http:" && url.protocol !== "https:") return null;
    return url;
  } catch {
    return null;
  }
}

function external(openUrl: string): EmbedPresentation {
  return { kind: "external", openUrl, label: siteLabel(openUrl) };
}
