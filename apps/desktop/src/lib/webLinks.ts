import type { FulltextError, WebVerifyStateEvent } from "../bindings";
import { createContext } from "react";
import { i18n } from "../i18n";

export type WebOpenMode = "reader" | "web" | "browser" | "cite";
export type WebLinkTarget = { href: string; text: string };
export type WebLinkNotice = { message: string; holdMs?: number; actionLabel?: string; onAction?: () => void };
/** 文案在读取时才取，换语言后立即生效。 */
const failureReason = (kind: FulltextError["kind"]) => ({
  get summary() { return i18n.t(`webFailure.${kind}.summary`, { ns: "shell" }); },
  get detail() { return i18n.t(`webFailure.${kind}.detail`, { ns: "shell" }); },
});
export const webFailureReasons: Record<FulltextError["kind"], { summary: string; detail: string }> = {
  Waf: failureReason("Waf"),
  Network: failureReason("Network"),
  Empty: failureReason("Empty"),
  NotArticle: failureReason("NotArticle"),
};

export const WebLinksContext = createContext<{
  hasWindow?: (url: string) => boolean;
  extract?: (url: string) => Promise<WebVerifyStateEvent | null>;
  editRule?: (domain: string, url: string) => void;
  open: (href: string, text?: string, mode?: WebOpenMode) => void;
  menu: (target: WebLinkTarget, anchor: { x: number; y: number }, opener: HTMLAnchorElement) => void;
} | null>(null);
