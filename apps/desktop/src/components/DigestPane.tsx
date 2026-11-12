import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { api, openExternal, type DigestSummary, type DigestView } from "../lib/api";
import { PREVIEW_DIGESTS, isTauri } from "../lib/preview";
import { BlockView, digestBlockPositionsIntact } from "./BlockView";
import { DigestSourceIndex, DigestSourcePreviewProvider } from "./DigestSourcePreview";
import { ErrorBoundary } from "./ErrorBoundary";
import { Icon } from "./Icon";
import type { SelectedBlock } from "../lib/selection";
import { Segmented } from "./Segmented";
import { useDelayed } from "./Pending";

function DigestSkeleton() {
  const late = useDelayed(250);
  return late ? <div className="digest-doc reader digest-skeleton" role="status" aria-label="正在加载简报" data-loading-skeleton="digest">
    <div className="reader-column" aria-hidden="true">
      <div className="digest-doc-head"><i className="digest-skeleton-period" /><i className="digest-skeleton-date" /></div>
      <i className="digest-skeleton-title" />
      {[0, 1, 2].map((n) => <div key={n} className="digest-skeleton-item"><i className="digest-skeleton-number" /><div><i /><i /></div></div>)}
    </div>
  </div> : null;
}

type Period = "daily" | "weekly";

/** 同一期、同一版本且内容一致时沿用原对象，正文与来源卡片不重挂。 */
function sameDigest(a: DigestView | null, b: DigestView) {
  return !!a && a.id === b.id && a.revision === b.revision && JSON.stringify(a) === JSON.stringify(b);
}

function fallbackDigests(period: Period): DigestView[] {
  return PREVIEW_DIGESTS.filter((d) => d.period === period);
}

function errorText(err: unknown): string {
  return String(err).replace(/^Error:\s*/, "");
}

function uniqueSources(sources: DigestView["source_index"]): DigestView["source_index"] {
  const seen = new Set<number>();
  return sources.filter((source) => !seen.has(source.article_id) && !!seen.add(source.article_id));
}

export function DigestPane({
  onChange,
  onOpenArticle,
  onOpenUrl = openExternal,
  onCiteSelection,
  initialDigest,
  onCurrentChange,
  visible = true,
  topmost = true,
  children,
}: {
  onChange?: () => void;
  onOpenArticle: (id: number) => void;
  onOpenUrl?: (href: string) => void;
  onCiteSelection?: (digest: DigestView, blocks: SelectedBlock[]) => void;
  initialDigest?: DigestView | null;
  onCurrentChange?: (digest: DigestView | null) => void;
  /** 主栏目保活：重新显示时静默重读期列表，同一期不重载正文；隐藏时收起来源卡片与划词菜单。 */
  visible?: boolean;
  topmost?: boolean;
  /** 当期追问区等随简报正文一起滚动的内容；由调用方决定是否渲染。 */
  children?: ReactNode;
}) {
  const [period, setPeriod] = useState<Period>(initialDigest?.period === "weekly" ? "weekly" : "daily");
  const scrollRoot = useRef<HTMLDivElement>(null);
  const [rows, setRows] = useState<DigestSummary[]>([]);
  const [index, setIndex] = useState(0);
  const [current, setCurrent] = useState<DigestView | null>(null);
  const [loading, setLoading] = useState(false);
  const [fetching, setFetching] = useState(true);
  const [loadError, setLoadError] = useState("");
  const [reloadKey, setReloadKey] = useState(0);
  const [documentKey, setDocumentKey] = useState(0);
  const [error, setError] = useState("");
  const [refreshError, setRefreshError] = useState("");
  const initialId = useRef(initialDigest?.id);
  // 切换日报 / 周报时先用看过的期列表、正文和选中期即时显示，再静默核对；同一版本不换对象，避免整页重挂。
  const lists = useRef(new Map<Period, DigestSummary[]>());
  const docs = useRef(new Map<number, DigestView>());
  const picked = useRef(new Map<Period, number>());
  // 期列表的写入者（首次 / 换周期加载、生成本期、静默重读）按代际取最后一次。
  const listGen = useRef(0);
  const latest = useRef({ period, rows, index, fetching, loading, loadError });

  useEffect(() => {
    latest.current = { period, rows, index, fetching, loading, loadError };
  });

  useEffect(() => {
    let cancelled = false;
    const gen = ++listGen.current;
    const cached = lists.current.get(period);
    setFetching(!cached);
    setLoadError("");
    setRefreshError("");
    const apply = (list: DigestSummary[]) => {
      if (cancelled || gen !== listGen.current) return;
      lists.current.set(period, list);
      const { rows: before, index: at } = latest.current;
      const keep = initialId.current ?? (cached ? before[at]?.id : undefined);
      setRows(list);
      if (list.length === 0) setCurrent(null);
      const savedIndex = list.findIndex((row) => row.id === keep);
      setIndex(savedIndex >= 0 ? savedIndex : 0);
      initialId.current = undefined;
    };
    api.digests(period).then(apply).catch((err) => {
      if (cancelled || gen !== listGen.current) return;
      if (!isTauri()) apply(fallbackDigests(period));
      else if (cached) setRefreshError(`期列表刷新失败：${errorText(err)}`);
      else setLoadError(`加载简报失败：${errorText(err)}`);
    }).finally(() => { if (!cancelled && gen === listGen.current) setFetching(false); });
    return () => { cancelled = true; };
  }, [period, reloadKey]);

  /** 静默重读期列表：发现后台新生成的期，选中期按 id 找回；失败保留原列表。 */
  const refreshList = useCallback(() => {
    const { period: listPeriod, rows: before, index: at, fetching: busyList, loading: generating, loadError: failed } = latest.current;
    // 首次加载失败时仍走显式「重试加载」，不用静默重读盖掉错误。
    if (busyList || generating || failed || !isTauri()) return;
    const gen = ++listGen.current;
    const selectedId = before[at]?.id;
    setRefreshError("");
    api.digests(listPeriod).then((list) => {
      if (gen !== listGen.current) return;
      const next = list.findIndex((row) => row.id === selectedId);
      lists.current.set(listPeriod, list);
      setRows(list);
      setIndex(next >= 0 ? next : 0);
      if (list.length === 0) setCurrent(null);
    }).catch((err) => {
      if (gen === listGen.current) setRefreshError(`期列表刷新失败：${errorText(err)}`);
    });
  }, []);

  const wasVisible = useRef(visible);
  useEffect(() => {
    const shown = visible && !wasVisible.current;
    wasVisible.current = visible;
    if (shown) refreshList();
  }, [visible, refreshList]);

  const selected = rows[index] ?? null;
  const selectedId = selected?.id ?? 0;
  const showing = current;
  const reading = fetching || (!!selected && current?.id !== selected.id);
  const positionSafe = showing?.document ? digestBlockPositionsIntact(showing.document) : false;
  // 只随选中 id 变化加载正文；期列表重读换了数组引用不算翻期。
  useEffect(() => {
    if (!selectedId) {
      return;
    }
    if (!isTauri()) {
      setCurrent(fallbackDigests(period).find((item) => item.id === selectedId) ?? null);
      return;
    }
    let cancelled = false;
    setLoadError("");
    const cached = docs.current.get(selectedId);
    if (cached) setCurrent(cached);
    api.digest(selectedId).then((view) => {
      if (cancelled) return;
      docs.current.set(view.id, view);
      setCurrent((cur) => sameDigest(cur, view) ? cur : view);
    }).catch((err) => {
      if (!cancelled && !cached) setLoadError(`加载简报失败：${errorText(err)}`);
    });
    return () => { cancelled = true; };
  }, [selectedId, period, documentKey, reloadKey]);

  useEffect(() => { onCurrentChange?.(showing); }, [showing, onCurrentChange]);

  const changePeriod = (next: Period) => {
    if (next === period) return;
    if (selectedId) picked.current.set(period, selectedId);
    const cached = lists.current.get(next) ?? [];
    const at = cached.findIndex((row) => row.id === picked.current.get(next));
    setRows(cached);
    setIndex(Math.max(0, at));
    setFetching(cached.length === 0 && !lists.current.has(next));
    setError("");
    setRefreshError("");
    initialId.current = picked.current.get(next);
    setPeriod(next);
  };

  const handleGenerate = async () => {
    setLoading(true);
    setCurrent(null);
    onCurrentChange?.(null);
    setError("");
    setRefreshError("");
    const gen = ++listGen.current;
    try {
      await api.generateDigest(period);
      const list = await api.digests(period);
      if (gen !== listGen.current) return;
      docs.current.clear();
      lists.current.set(period, list);
      setRows(list);
      setIndex(0);
      setDocumentKey((key) => key + 1);
      onChange?.();
      if (list.length === 0) {
        setError("这期没有生成：暂时没有符合条件的未读文章。");
      }
    } catch (err) {
      setError(`生成失败：${errorText(err)}`);
    } finally {
      setLoading(false);
    }
  };

  const atNewest = fetching || loading || index <= 0;
  const atOldest = fetching || loading || rows.length === 0 || index >= rows.length - 1;

  return (
    <div className="digest">
      <div className="toolbar" data-tauri-drag-region="deep">
        <Segmented className="toolbar-group segmented" role="group" aria-label="简报周期">
          <button
            type="button"
            className="tab"
            data-on={period === "daily" ? "1" : "0"}
            disabled={loading}
            onClick={() => changePeriod("daily")}
          >
            日报
          </button>
          <button
            type="button"
            className="tab"
            data-on={period === "weekly" ? "1" : "0"}
            disabled={loading}
            onClick={() => changePeriod("weekly")}
          >
            周报
          </button>
        </Segmented>
        <div className="digest-pager" role="group" aria-label="翻期">
          <button
            type="button"
            className="tool-btn icon-only"
            aria-label="上期"
            title="上一期"
            disabled={atOldest}
            onClick={() => setIndex((i) => Math.min(i + 1, rows.length - 1))}
          >
            <Icon name="chevronLeft" size={16} />
          </button>
          <span className="digest-date">{selected?.date_label ?? (fetching ? "" : "这期还没有")}</span>
          <button
            type="button"
            className="tool-btn icon-only"
            aria-label="下期"
            title="下一期"
            disabled={atNewest}
            onClick={() => setIndex((i) => Math.max(i - 1, 0))}
          >
            <Icon name="chevronRight" size={16} />
          </button>
        </div>
        <span className="toolbar-spacer" />
        <button type="button" className="btn sm" disabled={loading || fetching || !!loadError} onClick={() => void handleGenerate()}>
          <Icon name={loading ? "spinner" : "sparkles"} size={14} className={loading ? "spin" : undefined} />{loading ? "生成本期…" : "生成本期"}
        </button>
      </div>

      {error ? (
        <div className="notice error digest-error" role="alert">
          <Icon name="info" size={15} />
          <span>{error}</span>
        </div>
      ) : null}
      {refreshError && !loadError ? (
        <div className="notice error digest-error" role="alert">
          <Icon name="info" size={15} />
          <span>{refreshError}</span>
          <button type="button" className="btn sm" onClick={refreshList}>重试</button>
        </div>
      ) : null}

      <div className="scroll" ref={scrollRoot} aria-busy={reading && !loadError}>
        {reading && !showing && !loadError ? <DigestSkeleton /> : null}
        {loadError ? (
          <div className="digest-notice" role="alert">
            <p>{loadError}</p>
            <button type="button" className="btn sm" onClick={() => setReloadKey((key) => key + 1)}>重试加载</button>
          </div>
        ) : null}
        {!selected && !loading && !fetching && !loadError ? (
          <div className="empty digest-empty">
            <span className="empty-icon"><Icon name="digest" size={22} /></span>
            <p>这期还没有。点「生成本期」就会生成。</p>
          </div>
        ) : null}

        {showing ? (
          <DigestSourcePreviewProvider key={`${showing.id}:${showing.revision}`} active={visible}
            exposureActive={visible && topmost} scrollRoot={scrollRoot} sources={[...showing.source_index, ...showing.unlocated_sources]}>
            <article className="digest-doc reader" data-digest-id={showing.id}>
              <div className="reader-column">
                <div className="digest-doc-head">
                  <span className="digest-kicker">{showing.period_label}</span>
                  <span className="digest-doc-date">{showing.date_label}</span>
                  {/* period_start 与后端 digest_date_label 一样是毫秒；不指定时区，跟随本机日历。 */}
                  {showing.period === "daily" ? <span className="digest-doc-weekday">{new Date(showing.period_start).toLocaleDateString("zh-CN", { weekday: "long" })}</span> : null}
                </div>
                {showing.document ? (
                  <ErrorBoundary scope="digest"
                    resetKey={`${showing.id}:${showing.revision}`}
                    fallback={(_, reset) => (
                      <div className="notice error digest-doc-error" role="alert">
                        <Icon name="info" size={15} />
                        <span>这期正文显示不出来，来源和翻页仍可使用。</span>
                        <button type="button" className="btn sm" onClick={reset}>重试</button>
                      </div>
                    )}
                  >
                    <BlockView
                      key={`${showing.id}:${showing.revision}`}
                      document={showing.document}
                      digestAppearance
                      sources={showing.items}
                      selectable={positionSafe && !!onCiteSelection}
                      onCiteBlocks={(blocks) => onCiteSelection?.(showing, blocks)}
                      onOpenArticle={onOpenArticle}
                      onOpenUrl={onOpenUrl}
                      active={visible}
                    />
                  </ErrorBoundary>
                ) : null}
                {showing.source_note ? <p className="digest-notice muted" role="status">{showing.source_note}</p> : null}
                <DigestSourceIndex
                  sources={positionSafe ? showing.source_index : []}
                  unlocated={positionSafe ? showing.unlocated_sources : uniqueSources([...showing.source_index, ...showing.unlocated_sources])}
                  legacy={showing.source_state === "legacy"}
                  onOpenArticle={onOpenArticle}
                />
              </div>
            </article>
          </DigestSourcePreviewProvider>
        ) : null}
        {showing ? children : null}
      </div>
    </div>
  );
}
