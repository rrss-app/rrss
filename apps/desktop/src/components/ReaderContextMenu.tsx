import type { ReaderView } from "../lib/api";
import { openOriginal } from "../lib/signals";
import { FloatingMenu, type MenuAnchor } from "./FloatingMenu";
import { Icon } from "./Icon";
import { Segmented } from "./Segmented";

type Mode = "original" | "bilingual" | "translated";
export const READER_MODES: { id: Mode; label: string }[] = [
  { id: "original", label: "原文" }, { id: "bilingual", label: "双语" }, { id: "translated", label: "译文" },
];

export function ReaderContextMenu({ reader, anchor, boundary, mode, translating, summarizing, onClose, onMode, onTranslate, onRegenerate, onSummarize, onCite, onStar, onTop, onContinueAsking, onCollect, collecting }: {
  reader: ReaderView; anchor: MenuAnchor; boundary: HTMLElement; mode: Mode;
  translating: boolean; summarizing: boolean; onClose: () => void; onMode: (mode: Mode) => void;
  onTranslate: () => void; onRegenerate: () => void; onSummarize: () => void;
  onContinueAsking?: () => void;
  onCollect?: () => void; collecting?: boolean;
  onCite: () => void; onStar?: () => void; onTop: () => void;
}) {
  const run = (action: () => void) => { onClose(); action(); };
  return <FloatingMenu anchor={anchor} boundary={boundary} label="本篇操作" className="reader-context-menu" onClose={onClose}
    onKeyDown={event => {
      const key = event.key.toLowerCase();
      if (key === "t" && !translating) { event.preventDefault(); run(onTranslate); }
      if (key === "s" && onStar) { event.preventDefault(); run(onStar); }
      if (key === "v" && reader.translation_ready) { event.preventDefault(); onMode(READER_MODES[(READER_MODES.findIndex(item => item.id === mode) + 1) % 3].id); }
    }}>
    {onContinueAsking ? <><button type="button" role="menuitem" onClick={() => run(onContinueAsking)}><Icon name="chat" />在对话中继续提问<kbd>Q</kbd></button><hr role="separator" /></> : null}
    {reader.translation_ready ? <>
      <div className="reader-context-caption"><span>视图</span><kbd>V</kbd></div>
      <Segmented className="reader-context-modes" role="group" aria-label="视图">
        {READER_MODES.map(item => <button key={item.id} type="button" role="menuitemradio" aria-checked={mode === item.id} onClick={() => onMode(item.id)}>{item.label}</button>)}
      </Segmented>
      <button type="button" role="menuitem" disabled={translating} onClick={() => run(onRegenerate)}><Icon name="refresh" />重新翻译全文</button>
    </> : <button type="button" role="menuitem" disabled={translating} onClick={() => run(onTranslate)}><Icon name="translate" />翻译本篇<kbd>T</kbd></button>}
    <button type="button" role="menuitem" disabled={summarizing} onClick={() => run(onSummarize)}><Icon name="sparkles" />{summarizing ? "摘要生成中…" : reader.summary ? "重新生成摘要" : "生成本篇摘要"}</button>
    {onCollect ? <button type="button" role="menuitem" disabled={collecting} onClick={() => run(onCollect)}><Icon name="bookmark" />收录</button> : null}
    <button type="button" role="menuitem" onClick={() => run(onCite)}><Icon name="quote" />引用全文到对话</button>
    <hr role="separator" />
    <button type="button" role="menuitem" disabled={!onStar} onClick={() => onStar && run(onStar)}><Icon name="star" filled={reader.starred} />{reader.starred ? "取消收藏" : "收藏"}<kbd>S</kbd></button>
    <button type="button" role="menuitem" disabled={!reader.url} onClick={() => run(() => void openOriginal(reader.id, reader.url))}><Icon name="external" />查看原文</button>
    <button type="button" role="menuitem" onClick={() => run(onTop)}><Icon name="arrowUp" />回到顶部</button>
  </FloatingMenu>;
}
