/**
 * 块 IR 的前端类型与入口校验。类型与 `crates/rrss-render/src/ir.rs` 对齐；
 * `parseDocument` 是渲染前唯一的校验入口：逐节点检查形状，丢弃无法识别的块 / 行内节点，
 * 顶层不是文档时降级为空文档。校验只做形状，不改内容，不重建 HTML。
 */

export const IR_VERSION = 1;

export type Inline =
  | { t: "t"; text: string }
  | { t: "s"; children: Inline[] }
  | { t: "e"; children: Inline[] }
  | { t: "c"; text: string }
  | { t: "a"; href: string; children: Inline[] }
  | { t: "br" };

export type Block =
  | { t: "h"; level: number; inlines: Inline[] }
  | { t: "p"; inlines: Inline[] }
  | { t: "li"; indent: number; marker: string; inlines: Inline[] }
  | { t: "pre"; lang?: string | null; text: string }
  | { t: "img"; src: string; alt: string }
  | { t: "q"; inlines: Inline[] }
  | { t: "embed"; kind: string; url: string; title: string }
  | TableBlock
  | { t: "hr" };

/** 表格：`align` 每列一项（`l` / `c` / `r` / 空），`head` 一行，`rows` 已补齐到表头列数。 */
export type TableBlock = { t: "table"; align: string[]; head: Inline[][]; rows: Inline[][][] };

/** 表格的可见文字：表头与各行单元格依次拼接，给阅读时长、阅读位置这类按字数的计算用。 */
export function tableText(block: TableBlock, inlineText: (nodes: Inline[]) => string): string {
  return [block.head, ...block.rows].flat().map(inlineText).join(" ");
}

function parseTable(node: Raw): TableBlock | null {
  if (!Array.isArray(node.head) || !Array.isArray(node.rows)) return null;
  const cells = (row: unknown): Inline[][] => Array.isArray(row) ? row.map(parseInlines) : [];
  const head = cells(node.head);
  if (!head.length) return null;
  const width = head.length;
  const fit = (row: Inline[][]) => row.length >= width ? row.slice(0, width) : [...row, ...Array.from({ length: width - row.length }, () => [])];
  const align = Array.from({ length: width }, (_, i) => {
    const value = Array.isArray(node.align) ? node.align[i] : "";
    return value === "l" || value === "c" || value === "r" ? value : "";
  });
  return { t: "table", align, head, rows: node.rows.map((row) => fit(cells(row))) };
}

export type Document = { v: number; blocks: Block[] };

const EMPTY: Document = { v: IR_VERSION, blocks: [] };

type Raw = Record<string, unknown>;

function isRecord(value: unknown): value is Raw {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function str(value: unknown, fallback = ""): string {
  return typeof value === "string" ? value : fallback;
}

function int(value: unknown, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) ? Math.trunc(value) : fallback;
}

function parseInlines(value: unknown): Inline[] {
  if (!Array.isArray(value)) return [];
  const out: Inline[] = [];
  for (const node of value) {
    const inline = parseInline(node);
    if (inline) out.push(inline);
  }
  return out;
}

function parseInline(node: unknown): Inline | null {
  if (!isRecord(node)) return null;
  switch (node.t) {
    case "t":
      return typeof node.text === "string" ? { t: "t", text: node.text } : null;
    case "c":
      return typeof node.text === "string" ? { t: "c", text: node.text } : null;
    case "s":
      return { t: "s", children: parseInlines(node.children) };
    case "e":
      return { t: "e", children: parseInlines(node.children) };
    case "a":
      return typeof node.href === "string" ? { t: "a", href: node.href, children: parseInlines(node.children) } : null;
    case "br":
      return { t: "br" };
    default:
      return null;
  }
}

function parseBlock(node: unknown): Block | null {
  if (!isRecord(node)) return null;
  switch (node.t) {
    case "h":
      return { t: "h", level: int(node.level, 1), inlines: parseInlines(node.inlines) };
    case "p":
      return { t: "p", inlines: parseInlines(node.inlines) };
    case "li":
      return { t: "li", indent: int(node.indent, 0), marker: str(node.marker), inlines: parseInlines(node.inlines) };
    case "q":
      return { t: "q", inlines: parseInlines(node.inlines) };
    case "pre":
      return typeof node.text === "string" ? { t: "pre", lang: str(node.lang, "") || null, text: node.text } : null;
    case "img":
      return typeof node.src === "string" ? { t: "img", src: node.src, alt: str(node.alt) } : null;
    case "embed":
      return typeof node.url === "string" ? { t: "embed", kind: str(node.kind), url: node.url, title: str(node.title) } : null;
    case "table":
      return parseTable(node);
    case "hr":
      return { t: "hr" };
    default:
      return null;
  }
}

/** 同一份宿主载荷只校验一次；重复渲染拿到同一引用，不重新分配。 */
const validated = new WeakMap<object, Document>();

export function parseDocument(raw: unknown): Document {
  if (!isRecord(raw) || !Array.isArray(raw.blocks)) return EMPTY;
  const cached = validated.get(raw);
  if (cached) return cached;
  const blocks: Block[] = [];
  for (const node of raw.blocks) {
    const block = parseBlock(node);
    if (block) blocks.push(block);
  }
  const doc: Document = { v: int(raw.v, IR_VERSION), blocks };
  validated.set(raw, doc);
  validated.set(doc, doc);
  return doc;
}

function validInline(node: unknown): boolean {
  if (!isRecord(node)) return false;
  switch (node.t) {
    case "t": case "c": return typeof node.text === "string";
    case "br": return true;
    case "s": case "e": return Array.isArray(node.children) && node.children.every(validInline);
    case "a": return typeof node.href === "string" && Array.isArray(node.children) && node.children.every(validInline);
    default: return false;
  }
}

function validPatchBlock(node: unknown): boolean {
  if (!isRecord(node)) return false;
  const inlines = () => Array.isArray(node.inlines) && node.inlines.every(validInline);
  switch (node.t) {
    case "h": return Number.isInteger(node.level) && Number(node.level) >= 1 && Number(node.level) <= 6 && inlines();
    case "p": case "q": return inlines();
    case "li": return Number.isInteger(node.indent) && typeof node.marker === "string" && inlines();
    case "pre": return typeof node.text === "string" && (node.lang == null || typeof node.lang === "string");
    case "img": return typeof node.src === "string" && typeof node.alt === "string";
    case "embed": return typeof node.kind === "string" && typeof node.url === "string" && typeof node.title === "string";
    case "table": {
      const row = (value: unknown) => Array.isArray(value) && value.every((cell) => Array.isArray(cell) && cell.every(validInline));
      return Array.isArray(node.align) && node.align.every((a) => typeof a === "string")
        && row(node.head) && Array.isArray(node.rows) && node.rows.every(row);
    }
    case "hr": return true;
    default: return false;
  }
}

/** 补丁必须完整有效；只校验新块，保留未改变的已校验对象，不能丢块后重编号。 */
export function mergeDocumentPatches(base: unknown, patches: ReadonlyArray<{ index: number; ir: unknown }>): Document {
  const doc = parseDocument(base);
  if (!isRecord(base) || base.v !== IR_VERSION || !Array.isArray(base.blocks) || base.blocks.length !== doc.blocks.length) {
    throw new Error("正文块位置无效");
  }
  const seen = new Set<number>();
  const updates = patches.map(({ index, ir }) => {
    if (!Number.isInteger(index) || index < 0 || index >= doc.blocks.length || seen.has(index) || !validPatchBlock(ir)) {
      throw new Error("译文补丁无效");
    }
    seen.add(index);
    return { index, block: parseBlock(ir)! };
  });
  if (!updates.length) return doc;
  const blocks = doc.blocks.slice();
  for (const { index, block } of updates) blocks[index] = block;
  const next = { v: doc.v, blocks };
  validated.set(next, next);
  return next;
}
