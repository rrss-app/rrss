import { useTranslation } from "../i18n";
import type { ReaderView } from "../lib/api";
import { openOriginal } from "../lib/signals";
import { FloatingMenu, type MenuAnchor } from "./FloatingMenu";
import { Icon } from "./Icon";
import { Segmented } from "./Segmented";

type Mode = "original" | "bilingual" | "translated";
export const READER_MODES: { id: Mode }[] = [{ id: "original" }, { id: "bilingual" }, { id: "translated" }];

export function ReaderContextMenu({ reader, anchor, boundary, mode, translating, summarizing, onClose, onMode, onTranslate, onRegenerate, onSummarize, onCite, onStar, onTop, onContinueAsking, onCollect, collecting }: {
  reader: ReaderView; anchor: MenuAnchor; boundary: HTMLElement; mode: Mode;
  translating: boolean; summarizing: boolean; onClose: () => void; onMode: (mode: Mode) => void;
  onTranslate: () => void; onRegenerate: () => void; onSummarize: () => void;
  onContinueAsking?: () => void;
  onCollect?: () => void; collecting?: boolean;
  onCite: () => void; onStar?: () => void; onTop: () => void;
}) {
  const { t } = useTranslation("reader");
  const run = (action: () => void) => { onClose(); action(); };
  return <FloatingMenu anchor={anchor} boundary={boundary} label={t("menu.label")} className="reader-context-menu" onClose={onClose}
    onKeyDown={event => {
      const key = event.key.toLowerCase();
      if (key === "t" && !translating) { event.preventDefault(); run(onTranslate); }
      if (key === "s" && onStar) { event.preventDefault(); run(onStar); }
      if (key === "v" && reader.translation_ready) { event.preventDefault(); onMode(READER_MODES[(READER_MODES.findIndex(item => item.id === mode) + 1) % 3].id); }
    }}>
    {onContinueAsking ? <><button type="button" role="menuitem" onClick={() => run(onContinueAsking)}><Icon name="chat" />{t("menu.continueAsking")}<kbd>Q</kbd></button><hr role="separator" /></> : null}
    {reader.translation_ready ? <>
      <div className="reader-context-caption"><span>{t("menu.view")}</span><kbd>V</kbd></div>
      <Segmented className="reader-context-modes" role="group" aria-label={t("menu.view")}>
        {READER_MODES.map(item => <button key={item.id} type="button" role="menuitemradio" aria-checked={mode === item.id} onClick={() => onMode(item.id)}>{t(`mode.${item.id}`)}</button>)}
      </Segmented>
      <button type="button" role="menuitem" disabled={translating} onClick={() => run(onRegenerate)}><Icon name="refresh" />{t("menu.retranslate")}</button>
    </> : <button type="button" role="menuitem" disabled={translating} onClick={() => run(onTranslate)}><Icon name="translate" />{t("menu.translate")}<kbd>T</kbd></button>}
    <button type="button" role="menuitem" disabled={summarizing} onClick={() => run(onSummarize)}><Icon name="sparkles" />{summarizing ? t("menu.summarizing") : reader.summary ? t("menu.resummarize") : t("menu.summarize")}</button>
    {onCollect ? <button type="button" role="menuitem" disabled={collecting} onClick={() => run(onCollect)}><Icon name="bookmark" />{t("menu.collect")}</button> : null}
    <button type="button" role="menuitem" onClick={() => run(onCite)}><Icon name="quote" />{t("menu.citeFull")}</button>
    <hr role="separator" />
    <button type="button" role="menuitem" disabled={!onStar} onClick={() => onStar && run(onStar)}><Icon name="star" filled={reader.starred} />{reader.starred ? t("menu.unstar") : t("menu.star")}<kbd>S</kbd></button>
    <button type="button" role="menuitem" disabled={!reader.url} onClick={() => run(() => void openOriginal(reader.id, reader.url))}><Icon name="external" />{t("menu.viewOriginal")}</button>
    <button type="button" role="menuitem" onClick={() => run(onTop)}><Icon name="arrowUp" />{t("menu.top")}</button>
  </FloatingMenu>;
}
