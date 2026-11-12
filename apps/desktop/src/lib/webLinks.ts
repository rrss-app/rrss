import type { FulltextError, WebVerifyStateEvent } from "../bindings";
import { createContext } from "react";

export type WebOpenMode = "reader" | "web" | "browser" | "cite";
export type WebLinkTarget = { href: string; text: string };
export type WebLinkNotice = { message: string; holdMs?: number; actionLabel?: string; onAction?: () => void };
export const webFailureReasons: Record<FulltextError["kind"], { summary: string; detail: string }> = {
  Waf: { summary: "站点有访问防护", detail: "站点做了访问防护，RRSS 拿到的是验证页，不是文章。" },
  Network: { summary: "网络请求失败", detail: "网络请求失败，暂时无法读取这个网页。" },
  Empty: { summary: "没有提取到正文", detail: "网页里没有提取到正文，可能靠脚本生成内容，或需要登录。" },
  NotArticle: { summary: "这个网页不是文章", detail: "这个地址可能指向首页或列表页，没有识别到像文章的正文。" },
};

export const WebLinksContext = createContext<{
  hasWindow?: (url: string) => boolean;
  extract?: (url: string) => Promise<WebVerifyStateEvent | null>;
  editRule?: (domain: string, url: string) => void;
  open: (href: string, text?: string, mode?: WebOpenMode) => void;
  menu: (target: WebLinkTarget, anchor: { x: number; y: number }, opener: HTMLAnchorElement) => void;
} | null>(null);
