import { useEffect, useRef } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "../../i18n";
import { api } from "../../lib/api";
import { Icon } from "../../components/Icon";
import { RelativeTime } from "../../components/RelativeTime";

export function ReaderSourcePanel({ feedId, title, currentId, onClose, onOpen, onAll }: {
  feedId: number; title: string; currentId: number; onClose: () => void;
  onOpen: (id: number) => void; onAll: () => void;
}) {
  const { t } = useTranslation("reader");
  const close = useRef<HTMLButtonElement>(null);
  useEffect(() => { close.current?.focus({ preventScroll: true }); }, []);
  const recent = useQuery({
    queryKey: ["reader-source", feedId],
    queryFn: async () => {
      const scope = `feed:${feedId}`;
      const page = await api.feedsPage(scope, "all", null);
      return page.scope === scope ? page.articles.slice(0, 3) : [];
    },
    // 换篇再返回会重挂侧栏；短时间内复用同一份列表，不每次重读整页时间线。
    staleTime: 30_000,
    retry: false,
  });
  return <aside className="reader-article-drawer reader-source-panel" aria-label={title}>
    <div className="reader-article-drawer-head"><strong>{title}</strong>
      <button ref={close} type="button" className="tool-btn icon-only" onClick={onClose} aria-label={t("drawer.collapse")}><Icon name="close" /></button></div>
    <div className="reader-article-drawer-list">
      {recent.isPending ? <p role="status">{t("source.loading")}</p> : recent.isError ? <button type="button" className="tool-btn" onClick={() => void recent.refetch()}>{t("source.retry")}</button> :
        recent.data.length === 0 ? <p>{t("source.empty")}</p> : recent.data.map(row =>
          <div key={row.id} className={`reader-article-drawer-row${row.id === currentId ? " current" : ""}`}>
            <button type="button" className="reader-source-row" aria-current={row.id === currentId ? "page" : undefined}
              onClick={() => row.id === currentId ? onClose() : onOpen(row.id)}>
              <strong>{row.title}</strong>{row.published_at != null && <small><RelativeTime ms={row.published_at} /></small>}
            </button>
          </div>)}
      <button type="button" className="tool-btn reader-source-all" onClick={onAll}>{t("source.all")}<Icon name="arrowRight" size={14} /></button>
    </div>
  </aside>;
}
