import { parseDocument, tableText, type Inline } from "./blockIr";

function inlineText(nodes: Inline[]): string {
  return nodes.map(node => "text" in node ? node.text : "children" in node ? inlineText(node.children) : " ").join("");
}

/** 显示用估时（分钟）：只数已有正文的可见文字，汉字每分钟 400 字，其他文字每分钟 200 词；没有正文为 0。 */
export function readingMinutes(document: unknown): number {
  const text = parseDocument(document).blocks.map(block => "inlines" in block ? inlineText(block.inlines) : block.t === "pre" ? block.text : block.t === "table" ? tableText(block, inlineText) : "").join(" ");
  const han = text.match(/\p{Script=Han}/gu)?.length ?? 0;
  const words = text.replace(/\p{Script=Han}/gu, " ").match(/[\p{L}\p{N}]+/gu)?.length ?? 0;
  return han + words ? Math.max(1, Math.ceil(han / 400 + words / 200)) : 0;
}
