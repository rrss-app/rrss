import { common, createLowlight } from "lowlight";
import type { CodeToken } from "./codeHighlight";

/**
 * 只由 codeHighlight.ts 动态导入：语法包单独成块，第一次遇到代码块时才加载。
 * 有语言标注时只认标注（不认识的语言不上色）；没有标注才在常用语言里猜，相关度不够就不上色。
 */
const lowlight = createLowlight(common);

/** 自动检测的候选：去掉容易把普通文字误认成代码的 CSS / SCSS / INI 等，以及少见语言。 */
const AUTO_SUBSET = ["bash", "c", "cpp", "csharp", "diff", "go", "java", "javascript", "json", "kotlin", "php", "python", "ruby", "rust", "sql", "swift", "typescript", "xml", "yaml"];
/** 短代码段猜错时相关度一般不超过 4，普通文字最多约 3；宁可不上色也不上错色。 */
const AUTO_MIN_RELEVANCE = 5;

/** highlight.js 的类名归到八个语义色，其余不上色。 */
const KIND: Record<string, string> = {
  keyword: "keyword", "selector-tag": "keyword", doctag: "keyword",
  string: "string", regexp: "string", symbol: "string", link: "string", "selector-attr": "string",
  number: "number", literal: "number",
  comment: "comment", quote: "comment",
  title: "title", section: "title",
  type: "type", built_in: "type", "template-tag": "type",
  attr: "attr", attribute: "attr", property: "attr", variable: "attr", "template-variable": "attr",
  "selector-class": "attr", "selector-id": "attr", params: "attr",
  meta: "meta", tag: "meta", name: "meta", "selector-pseudo": "meta", bullet: "meta",
  addition: "add", deletion: "del",
};

type HastNode = { type: string; value?: string; properties?: { className?: unknown }; children?: HastNode[] };

function kindOf(node: HastNode): string | undefined {
  const names = Array.isArray(node.properties?.className) ? node.properties.className as string[] : [];
  // 「hljs-title class_」这类组合：类名修饰（class_）优先归到类型色。
  if (names.includes("class_")) return "type";
  for (const name of names) {
    const kind = KIND[name.replace(/^hljs-/, "")];
    if (kind) return kind;
  }
  return undefined;
}

function convert(nodes: HastNode[] = []): CodeToken[] {
  const out: CodeToken[] = [];
  for (const node of nodes) {
    if (node.type === "text") out.push(node.value ?? "");
    else if (node.type === "element") {
      const children = convert(node.children);
      const kind = kindOf(node);
      if (kind) out.push({ kind, children });
      else out.push(...children);
    }
  }
  return out;
}

function looksLikeJson(text: string): boolean {
  const trimmed = text.trim();
  if (!/^[[{]/.test(trimmed)) return false;
  try { JSON.parse(trimmed); return true; } catch { return false; }
}

export function highlight(text: string, lang: string | null): CodeToken[] | null {
  if (lang) {
    if (!lowlight.registered(lang)) return null;
    return convert(lowlight.highlight(lang, text).children as HastNode[]);
  }
  if (looksLikeJson(text)) return convert(lowlight.highlight("json", text).children as HastNode[]);
  const result = lowlight.highlightAuto(text, { subset: AUTO_SUBSET });
  if (!result.data?.language || (result.data.relevance ?? 0) < AUTO_MIN_RELEVANCE) return null;
  return convert(result.children as HastNode[]);
}
