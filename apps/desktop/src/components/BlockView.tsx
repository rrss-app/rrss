import { memo, useContext, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type MouseEvent as ReactMouseEvent, type ReactNode, type RefObject } from "react";
import { createPortal } from "react-dom";
import type { DigestLineView, MediaRef } from "../bindings";
import { WebLinksContext } from "../lib/webLinks";
import { api, copyText, isSafeHttpUrl, openExternal } from "../lib/api";
import { formatNotice } from "../lib/notice";
import { parseDocument, type Block, type Document, type Inline } from "../lib/blockIr";
import { inlineText } from "../lib/readingTime";
import { readSelection, type SelectedBlock, type SelectionPick } from "../lib/selection";
import { EmbedBlock } from "./EmbedBlock";
import { DigestSourceMarks } from "./DigestSourcePreview";
import { FloatingMenu, keepsNativeContextMenu } from "./FloatingMenu";
import { Icon } from "./Icon";
import type { EmbedCoverView } from "../bindings";
export { readSelection } from "../lib/selection";
export type { SelectionPick } from "../lib/selection";

export type { Block, Document, Inline } from "../lib/blockIr";
export { parseDocument } from "../lib/blockIr";

/** 阅读器正文区：划词条在这里用浮层菜单，右键交给 contextmenu；简报和对话不在此列。 */
const readerBody = (el: Element | null) => el?.closest<HTMLElement>(".drawer.reader .drawer-body") ?? null;

type SelMenu = SelectionPick & { x: number; y: number; boundary: HTMLElement | null; selection?: DOMRect };
export type ReaderSelection = SelectionPick & { boundary: HTMLElement; selection: DOMRect; opener: Element | null };

function renderInlines(nodes: Inline[], onOpenUrl?: (href: string) => void): ReactNode {
  return nodes.map((n, i) => {
    switch (n.t) {
      case "t":
        return <span key={i}>{n.text}</span>;
      case "s":
        return <strong key={i}>{renderInlines(n.children, onOpenUrl)}</strong>;
      case "e":
        return <em key={i}>{renderInlines(n.children, onOpenUrl)}</em>;
      case "c":
        return <code key={i}>{n.text}</code>;
      case "a":
        return <ContentLink key={i} href={n.href} onOpenUrl={onOpenUrl}>{renderInlines(n.children, onOpenUrl)}</ContentLink>;
      case "br":
        return <br key={i} />;
      default:
        return null;
    }
  });
}

function ContentLink({ href, onOpenUrl, children }: { href: string; onOpenUrl?: (href: string) => void; children: ReactNode }) {
  const actions = useContext(WebLinksContext);
  // 外部链接带站点名，对话里标在链接后（样式只在对话消息里生效）。
  const site = isSafeHttpUrl(href) ? siteName(href) : undefined;
  return <a href={href} data-site={site || undefined} onClick={event => {
    event.preventDefault();
    const selection = window.getSelection();
    if (selection && !selection.isCollapsed) return;
    if (actions && isSafeHttpUrl(href)) actions.open(href, event.currentTarget.textContent ?? "");
    else onOpenUrl?.(href);
  }} onContextMenu={event => {
    if (!actions || !isSafeHttpUrl(href)) return;
    event.preventDefault();
    event.stopPropagation();
    actions.menu({ href, text: event.currentTarget.textContent ?? "" }, { x: event.clientX, y: event.clientY }, event.currentTarget);
  }}>{children}</a>;
}

function siteName(href: string): string {
  try {
    return new URL(href).hostname.replace(/^www\./, "");
  } catch {
    return "";
  }
}

// 仅用于段落行距：没有语言字段，不把排版判断当成文章语言。
function latinTypography(block: Block): boolean {
  const value = "inlines" in block ? inlineText(block.inlines) : "";
  return /\p{Script=Latin}/u.test(value) && !/[\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}\p{Script=Hangul}]/u.test(value);
}

type DigestDecoration = { number?: string; summary?: "start" | "continued" };

/** 只给已有块附加外观，不插入 / 合并块，以保留来源与划词使用的块下标。 */
function digestDecorations(blocks: Block[]): DigestDecoration[] {
  const lastItem = blocks.reduce((last, block, index) => block.t === "li" ? index : last, -1);
  const hasClosing = lastItem >= 0 && blocks.length > lastItem + 1 && blocks.slice(lastItem + 1).every(block => block.t === "p");
  let item = 0;
  return blocks.map((block, index) => {
    // 分节的简报（如周报）每个小标题下重新编号。
    if (block.t === "h") item = 0;
    if (block.t === "li") {
      item += 1;
      return { number: block.marker.match(/^\s*(\d+)/)?.[1] ?? String(item) };
    }
    return hasClosing && index > lastItem ? { summary: index === lastItem + 1 ? "start" : "continued" } : {};
  });
}

function renderBlock(
  block: Block,
  i: number,
  mediaOf: (src: string) => MediaRef | undefined,
  mediaLoading: boolean | undefined,
  onOpenUrl?: (href: string) => void,
  nested = false,
  source?: DigestLineView,
  onOpenArticle?: (id: number) => void,
  embedCovers?: EmbedCoverView[],
  articleId?: number,
  readerAppearance = false,
  digest?: DigestDecoration,
  onMediaRef?: (media: MediaRef) => void,
): ReactNode {
  const id = { ...(nested ? {} : { "data-block-id": String(i) }), ...(readerAppearance && latinTypography(block) ? { "data-latin": true } : {}) };
  const key = nested ? undefined : i;
  switch (block.t) {
    case "h": {
      const level = Math.min(6, Math.max(1, block.level || 1));
      const Tag = `h${level}` as "h1";
      return (
        <Tag key={key} {...id}>
          {renderInlines(block.inlines, onOpenUrl)}
        </Tag>
      );
    }
    case "p":
      return (
        <p key={key} {...id} className={digest?.summary ? "digest-closing" : undefined}>
          {digest?.summary === "start" ? <span className="digest-closing-label" data-selection-ignore>小结</span> : null}
          {renderInlines(block.inlines, onOpenUrl)}
        </p>
      );
    case "li": {
      const pad: CSSProperties = { paddingLeft: (block.indent || 0) * 16 + (digest?.number ? 36 : 0) };
      return (
        <p key={key} {...id} style={pad} className={digest?.number ? "digest-list-item" : readerAppearance ? "reader-list-item" : undefined}>
          <span data-selection-ignore className={digest?.number ? "digest-list-number" : readerAppearance ? `reader-list-marker${/^\d/.test(block.marker) ? " is-ordered" : ""}` : undefined}>{digest?.number ?? block.marker} </span>{renderInlines(block.inlines, onOpenUrl)}
          {source && onOpenArticle ? <DigestSourceMarks sources={source.sources} onOpenArticle={onOpenArticle} /> : null}
        </p>
      );
    }
    case "pre":
      return (
        <pre key={key} {...id}>
          {readerAppearance ? <span className="reader-code-tools" data-selection-ignore><span>{block.lang}</span><button type="button" aria-label="复制代码" onClick={() => void copyText(block.text)}>复制</button></span> : <button type="button" style={{ float: "right" }} onClick={() => copyText(block.text)}>
            复制
          </button>}
          <code>{block.text}</code>
        </pre>
      );
    case "img": {
      const cached = mediaOf(block.src);
      if (cached?.href) {
        const src = cached.href;
        if (readerAppearance) return <figure key={key} {...id} className="reader-figure">
          <CachedImage src={src} alt={block.alt} fallback={block.alt || block.src} width={cached.width} height={cached.height} />
          {block.alt ? <figcaption data-selection-ignore>{block.alt}</figcaption> : null}
        </figure>;
        return (
          <p key={key} {...id}>
            <CachedImage src={src} alt={block.alt} fallback={block.alt || block.src} width={cached.width} height={cached.height} />
          </p>
        );
      }
      return (
        <p key={key} {...id} className="embed">
          <ImageFallback
            alt={block.alt}
            src={block.src}
            error={cached?.error ?? ""}
            loading={!!mediaLoading}
            articleId={articleId}
            onMediaRef={onMediaRef}
          />
        </p>
      );
    }
    case "q":
      return (
        <blockquote key={key} {...id}>
          {renderInlines(block.inlines, onOpenUrl)}
        </blockquote>
      );
    case "embed":
      return (
        <div key={key} {...id}>
          <EmbedBlock block={block} covers={embedCovers} articleId={articleId} onOpenUrl={onOpenUrl} />
        </div>
      );
    case "table": {
      // 宽表在块内横向滚动；整表是一个块，划词按块定位。
      const align = (column: number) => CELL_ALIGN[block.align[column] ?? ""];
      return (
        <div key={key} {...id} className="block-table">
          <table>
            <thead>
              <tr>{block.head.map((cell, c) => <th key={c} scope="col" style={align(c)}>{renderInlines(cell, onOpenUrl)}</th>)}</tr>
            </thead>
            {block.rows.length ? (
              <tbody>
                {block.rows.map((row, r) => <tr key={r}>{row.map((cell, c) => <td key={c} style={align(c)}>{renderInlines(cell, onOpenUrl)}</td>)}</tr>)}
              </tbody>
            ) : null}
          </table>
        </div>
      );
    }
    case "hr":
      return <hr key={key} {...id} />;
    default:
      return null;
  }
}

const CELL_ALIGN: Record<string, CSSProperties | undefined> = { l: { textAlign: "left" }, c: { textAlign: "center" }, r: { textAlign: "right" } };

type Props = {
  document: unknown;
  translated?: unknown | null;
  mode?: "original" | "bilingual" | "translated";
  media?: MediaRef[];
  mediaLoading?: boolean;
  selectable?: boolean;
  sources?: Array<DigestLineView>;
  onOpenArticle?: (id: number) => void;
  onCite?: (excerpt: string, blockId: number) => void;
  onTranslateSelection?: (selection: ReaderSelection) => void;
  onCiteBlocks?: (blocks: SelectedBlock[]) => void;
  onOpenUrl?: (href: string) => void;
  onMediaRef?: (media: MediaRef) => void;
  embedCovers?: EmbedCoverView[];
  articleId?: number;
  /** 为 false 时收起划词菜单（所在主栏目被隐藏）；缺省为 true，不影响阅读器与对话。 */
  active?: boolean;
  blockNotes?: ReadonlyMap<number, string>;
  /** 仅文章阅读器启用文内装饰；简报 / 对话沿用原结构与样式。 */
  readerAppearance?: boolean;
  /** 仅简报正文启用；追问回答、文章阅读器与独立对话不启用。 */
  digestAppearance?: boolean;
};

function isSharedBlock(block: Block): boolean {
  // 表格不进翻译单元，双语时只画一次。
  return block.t === "pre" || block.t === "img" || block.t === "embed" || block.t === "table" || block.t === "hr";
}

export function digestBlockPositionsIntact(raw: unknown): boolean {
  if (!raw || typeof raw !== "object" || !Array.isArray((raw as { blocks?: unknown }).blocks)) return false;
  const blocks = (raw as { blocks: unknown[] }).blocks;
  return (raw as { v?: unknown }).v === 1 && parseDocument(raw).blocks.length === blocks.length;
}

const EMPTY_MEDIA: MediaRef[] = [];

const RenderedBlock = memo(function RenderedBlock({ block, original, alternate, index, mediaOf, mediaLoading, onOpenUrl, onMediaRef, source, onOpenArticle, embedCovers, articleId, note, readerAppearance, digest }: {
  block: Block; original?: Block; alternate?: Block; index: number; mediaOf: (src: string) => MediaRef | undefined;
  mediaLoading?: boolean; onOpenUrl?: (href: string) => void; onMediaRef?: (media: MediaRef) => void; source?: DigestLineView;
  onOpenArticle?: (id: number) => void; embedCovers?: EmbedCoverView[]; articleId?: number; note?: string;
  readerAppearance?: boolean;
  digest?: DigestDecoration;
}) {
  const draw = (value: Block, nested = false) => renderBlock(value, index, mediaOf, mediaLoading, onOpenUrl, nested, source, onOpenArticle, embedCovers, articleId, readerAppearance, digest, onMediaRef);
  if (original && alternate && !isSharedBlock(original)) return (
    <div className="block-pair" data-block-id={String(index)} data-heading={readerAppearance && original.t === "h" ? original.level : undefined}>
      <div className="block-orig">{draw(original, true)}</div>
      <div className="block-tr">{draw(alternate, true)}</div>
      {note ? <small className="reader-unit-note" data-selection-ignore>{note}</small> : null}
    </div>
  );
  if (note) return <div className="block-with-note" data-block-id={String(index)}>{draw(block, true)}<small className="reader-unit-note" data-selection-ignore>{note}</small></div>;
  return draw(original ?? block);
});

/** 划词条：阅读器用浮层菜单（带图标、夹在正文区）；简报和对话保持固定定位、不夺焦点。 */
function SelectionMenu({ menu, menuRef, onCopy, onCite, onClose, onTranslate }: {
  menu: SelMenu; menuRef: RefObject<HTMLDivElement | null>; onCopy: () => void; onCite: () => void; onClose: () => void;
  onTranslate?: () => void;
}) {
  const icons = !!menu.boundary;
  const items = <>
    <button type="button" role="menuitem" onClick={onCopy}>{icons ? <Icon name="copy" size={16} /> : null}复制</button>
    <button type="button" role="menuitem" onClick={onCite}>{icons ? <Icon name="quote" size={16} /> : null}引用</button>
    {icons && onTranslate ? <button type="button" role="menuitem" onClick={onTranslate}><Icon name="translate" size={16} />翻译</button> : null}
  </>;
  if (menu.boundary) {
    return <FloatingMenu anchor={{ x: menu.x, y: menu.y + 12, selection: menu.selection }} boundary={menu.boundary} label="选区" className="sel-menu reader-selection-menu" onClose={onClose}>
      {items}
    </FloatingMenu>;
  }
  return createPortal(
    <div ref={menuRef} className="sel-menu" role="menu"
      style={{ left: Math.max(8, Math.min(menu.x, window.innerWidth - 168)), top: Math.max(8, Math.min(menu.y + 12, window.innerHeight - 44)) }}
      onMouseDown={event => event.preventDefault()}>
      {items}
    </div>,
    window.document.body,
  );
}

export function BlockView({
  document,
  translated,
  mode = "original",
  media = EMPTY_MEDIA,
  mediaLoading,
  selectable,
  sources,
  onOpenArticle,
  onCite,
  onTranslateSelection,
  onCiteBlocks,
  onOpenUrl,
  onMediaRef,
  embedCovers,
  articleId,
  active = true,
  blockNotes,
  readerAppearance = false,
  digestAppearance = false,
}: Props) {
  const digestUnsafe = sources !== undefined && !digestBlockPositionsIntact(document);
  const digestSources = digestUnsafe ? undefined : sources;
  const digestSelectable = !digestUnsafe && selectable;
  const doc = parseDocument(document);
  const tr = parseDocument(translated);
  const mediaIndex = useMemo(() => new Map(media.map(item => [item.src, item])), [media]);
  const mediaOf = useCallback((src: string) => mediaIndex.get(src), [mediaIndex]);
  const [menu, setMenu] = useState<SelMenu | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const documentRef = useRef<HTMLDivElement>(null);
  const selectionOpener = useRef<Element | null>(null);

  useEffect(() => { setMenu(null); }, [document, mode]);
  useEffect(() => { if (!active) setMenu(null); }, [active]);
  useEffect(() => {
    if (!menu || menu.boundary) return;
    const onDown = (event: MouseEvent) => {
      if (menuRef.current?.contains(event.target as Node)) return;
      setMenu(null);
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopImmediatePropagation();
      setMenu(null);
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey, true);
    };
  }, [menu]);

  const submitCitation = (pick: SelectionPick) => {
    if (onCiteBlocks) onCiteBlocks(pick.blocks);
    else onCite?.(pick.excerpt, pick.blockId);
    window.getSelection()?.removeAllRanges();
    setMenu(null);
  };

  const showSelectionMenu = (e: ReactMouseEvent, context = false) => {
    if (!digestSelectable || (!onCite && !onCiteBlocks)) return;
    if (menuRef.current?.contains(e.target as Node)) return;
    if ((e.target as Element).closest?.("button, .sel-menu")) return;
    const boundary = readerBody(documentRef.current);
    if (context && !boundary) return;
    const pick = readSelection(documentRef.current);
    if (!pick) {
      setMenu(null);
      return;
    }
    if (context) { e.preventDefault(); e.stopPropagation(); }
    if (e.altKey && !context) {
      submitCitation(pick);
      return;
    }
    const selection = boundary ? window.getSelection()?.getRangeAt(0).getBoundingClientRect() : undefined;
    selectionOpener.current = window.document.activeElement;
    setMenu({ ...pick, x: e.clientX, y: e.clientY, boundary, selection });
  };

  const cite = () => {
    if (menu) submitCitation(menu);
  };

  const copy = async () => {
    if (!menu) return;
    await copyText(menu.excerpt);
    setMenu(null);
  };

  const showBilingual = mode === "bilingual" && tr.blocks.length > 0;
  const display = mode === "translated" && tr.blocks.length > 0 ? tr : doc;
  const decorations = useMemo(() => digestAppearance ? digestDecorations(display.blocks) : [], [digestAppearance, display]);
  const byBlockId = new Map(digestSources?.map((line) => [line.block_id, line]));

  return (
    <div className="reader" data-reader-document data-reader-prose={readerAppearance || undefined} data-digest-prose={digestAppearance || undefined} ref={documentRef}
      onMouseUp={event => {
        // 阅读器右键交给 contextmenu。简报和对话仍在松手时弹出，含右键松手。
        if (event.button !== 0 && readerBody(event.currentTarget)) return;
        showSelectionMenu(event);
      }}
      onContextMenu={event => {
        // 正文的链接、媒体和编辑控件必须先让位给系统菜单，即使有选区。
        if (keepsNativeContextMenu(event.target)) return;
        showSelectionMenu(event, true);
      }}>
      {digestUnsafe ? <p className="notice error" role="alert">这期简报的排版有问题，来源标记和划词引用暂时不可用。</p> : null}
      {display.blocks.map((block, i) => <RenderedBlock key={i} block={block} index={i}
        original={showBilingual ? doc.blocks[i] : undefined} alternate={showBilingual ? tr.blocks[i] ?? doc.blocks[i] : undefined}
        mediaOf={mediaOf} mediaLoading={mediaLoading} onOpenUrl={onOpenUrl} onMediaRef={onMediaRef} source={block.t === "li" ? byBlockId.get(i) : undefined}
        onOpenArticle={onOpenArticle} embedCovers={embedCovers} articleId={articleId} note={blockNotes?.get(i)} readerAppearance={readerAppearance} digest={decorations[i]} />)}
      {menu && active ? <SelectionMenu menu={menu} menuRef={menuRef} onCopy={copy} onCite={cite} onClose={() => setMenu(null)}
        onTranslate={onTranslateSelection && menu.boundary && menu.selection ? () => {
          onTranslateSelection({ ...menu, boundary: menu.boundary!, selection: menu.selection!, opener: selectionOpener.current });
          setMenu(null);
        } : undefined} /> : null}
    </div>
  );
}

/** 配图没缓存下来时的占位：写明原因，可重试或交给外部浏览器。重试只在阅读器里有（要文章 id 与回写入口）。 */
function ImageFallback({ alt, src, error, loading, articleId, onMediaRef }: {
  alt: string; src: string; error: string; loading: boolean; articleId?: number; onMediaRef?: (media: MediaRef) => void;
}) {
  const [busy, setBusy] = useState(false);
  const [retryError, setRetryError] = useState("");
  const canRetry = typeof articleId === "number" && !!onMediaRef;
  const retry = () => {
    if (typeof articleId !== "number" || !onMediaRef || busy) return;
    setBusy(true);
    setRetryError("");
    void api.retryImage(articleId, src).then((ref) => {
      onMediaRef(ref);
    }).catch((err) => setRetryError(formatNotice("重试失败：", err))).finally(() => setBusy(false));
  };
  if (loading) return <>配图加载中…</>;
  const label = alt || src;
  const reason = retryError || error;
  return (
    <>
      {reason ? `图片未能缓存（${reason}）：${label}` : `图片未能缓存：${label}`}
      <span className="image-cache-actions" data-selection-ignore>
        {canRetry ? <button type="button" className="tool-btn" aria-busy={busy || undefined} onClick={retry}>重试</button> : null}
        <button type="button" className="tool-btn" onClick={() => void openExternal(src)}>在浏览器打开图片</button>
      </span>
    </>
  );
}

/**
 * 宿主给出真实宽高时写进属性：浏览器在解码前就按这个比例占位，正文高度不随图片加载变化。
 * 已预热（解码好）的图片挂上就显示；还要读盘解码的到了再淡入，不从空白一下跳出来。
 */
function CachedImage({ src, alt, fallback, width, height }: { src: string; alt: string; fallback: string; width?: number | null; height?: number | null }) {
  const [failed, setFailed] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const image = useRef<HTMLImageElement>(null);
  useLayoutEffect(() => {
    setFailed(false);
    // 绘制前判断：预热过的图片此刻已完成解码，直接显示，不走淡入。
    const node = image.current;
    setLoaded(!!node && node.complete && node.naturalWidth > 0);
  }, [src]);
  if (failed) {
    return <span className="embed">图片未能显示：{fallback}</span>;
  }
  return (
    <img
      ref={image}
      className="reader-img"
      data-loaded={loaded ? "true" : "false"}
      src={src}
      alt={alt}
      width={width ?? undefined}
      height={height ?? undefined}
      onLoad={() => setLoaded(true)}
      onError={() => setFailed(true)}
    />
  );
}

export const LIBRARY_FIXTURE: Document = {
  v: 1,
  blocks: [
    { t: "h", level: 2, inlines: [{ t: "t", text: "开馆日" }] },
    {
      t: "p",
      inlines: [
        { t: "t", text: "新图书馆在市中心开放，开放时间 " },
        { t: "s", children: [{ t: "t", text: "9–21" }] },
        { t: "t", text: " 点。周末有 " },
        { t: "e", children: [{ t: "t", text: "故事会" }] },
        { t: "t", text: " 和 " },
        { t: "a", href: "https://library.example/hours", children: [{ t: "t", text: "时刻表" }] },
        { t: "t", text: "。" },
      ],
    },
    { t: "p", inlines: [{ t: "t", text: "儿童区有矮书架 📚，志愿者帮忙还书 🎉。" }] },
    { t: "img", src: "cover.png", alt: "馆舍外观" },
    { t: "li", indent: 0, marker: "•", inlines: [{ t: "t", text: "最多借 20 册" }] },
    { t: "li", indent: 0, marker: "•", inlines: [{ t: "t", text: "线上续借，除非有人预约" }] },
    { t: "q", inlines: [{ t: "t", text: "图书馆不是奢侈品，而是生活必需。" }] },
    { t: "pre", lang: null, text: "open 09:00\nclose 21:00" },
    { t: "p", inlines: [{ t: "t", text: "末段：中文、emoji ⭐ 与拉丁字母 ABC 排在一起，用来看缺字形。" }] },
    { t: "embed", kind: "iframe", url: "https://example.com/v", title: "外开卡片" },
  ],
};

export const LIBRARY_FIXTURE_EN: Document = {
  v: 1,
  blocks: [
    { t: "h", level: 2, inlines: [{ t: "t", text: "Opening Day" }] },
    {
      t: "p",
      inlines: [
        { t: "t", text: "The new library opened downtown, hours " },
        { t: "s", children: [{ t: "t", text: "9–21" }] },
        { t: "t", text: ". Weekends have " },
        { t: "e", children: [{ t: "t", text: "story time" }] },
        { t: "t", text: " and a " },
        { t: "a", href: "https://library.example/hours", children: [{ t: "t", text: "schedule" }] },
        { t: "t", text: "." },
      ],
    },
    { t: "p", inlines: [{ t: "t", text: "The children's area has low shelves 📚, volunteers help return books 🎉." }] },
    { t: "img", src: "cover.png", alt: "Building exterior" },
    { t: "li", indent: 0, marker: "•", inlines: [{ t: "t", text: "Borrow up to 20 items" }] },
    { t: "li", indent: 0, marker: "•", inlines: [{ t: "t", text: "Renew online unless on hold" }] },
    { t: "q", inlines: [{ t: "t", text: "A library is not a luxury, but a necessity." }] },
    { t: "pre", lang: null, text: "open 09:00\nclose 21:00" },
    { t: "p", inlines: [{ t: "t", text: "Closing: Chinese, emoji ⭐ and Latin ABC together." }] },
    { t: "embed", kind: "iframe", url: "https://example.com/v", title: "外开卡片" },
  ],
};
