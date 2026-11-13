import { i18n } from "../../i18n";
import { api, onSchedulerUpdated, type FeedView, type RefreshOperation, type RefreshStarted } from "../../lib/api";

/** 需要提醒时返回提示文字：点名前几个失败源，没有的项不写；全部成功返回 null。 */
export function refreshProblem(result: RefreshOperation, feeds: readonly FeedView[]): string | null {
  if (!result.error && result.failed === 0 && result.skipped === 0) return null;
  const names = result.failed_feeds.map(id => feeds.find(feed => feed.id === id)?.title || i18n.t("refresh.feedFallback", { ns: "focus", id }));
  const count = result.failed;
  const joined = names.join(i18n.t("list.enumSeparator", { ns: "focus" }));
  const failed = count <= 0 ? ""
    : !names.length ? i18n.t("refresh.failed", { ns: "focus", count })
    : count > names.length ? i18n.t("refresh.failedNamedMore", { ns: "focus", count, names: joined })
    : i18n.t("refresh.failedNamed", { ns: "focus", count, names: joined });
  const comma = i18n.t("list.commaSeparator", { ns: "focus" });
  if (result.error) return [result.error, failed].filter(Boolean).join(comma);
  const skipped = result.skipped > 0 ? i18n.t("refresh.skipped", { ns: "focus", count: result.skipped }) : "";
  return i18n.t("refresh.doneWithProblems", { ns: "focus", parts: [failed, skipped].filter(Boolean).join(comma) });
}

/** Events invalidate; queries reconcile early/missing events and a restarted host. */
export function waitForRefresh(
  submit: () => Promise<RefreshStarted | null>,
  signal: AbortSignal,
  started: () => void = () => {},
): Promise<RefreshOperation | null> {
  return new Promise((resolve, reject) => {
    let operation: RefreshStarted | null = null;
    let disposed = false;
    let querying = false;
    let dirty = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let unlisten: (() => void) | undefined;
    let version = -1;
    const cleanup = () => {
      disposed = true;
      if (timer !== undefined) clearTimeout(timer);
      unlisten?.();
      signal.removeEventListener("abort", abort);
    };
    const fail = (error: unknown) => { if (!disposed) { cleanup(); reject(error); } };
    const abort = () => fail(new DOMException(i18n.t("refresh.listenEnded", { ns: "focus" }), "AbortError"));
    const read = async () => {
      timer = undefined;
      if (disposed || !operation) return;
      if (querying) { dirty = true; return; }
      querying = true;
      try {
        const snapshot = await api.schedulerStatus();
        if (disposed) return;
        if (snapshot.run_id !== operation.run_id || snapshot.stopped) throw new Error(i18n.t("refresh.runtimeStopped", { ns: "focus" }));
        if (snapshot.version >= version) {
          version = snapshot.version;
          const current = snapshot.operations.find(item => item.id === operation?.operation_id);
          if (!current) throw new Error(i18n.t("refresh.recordExpired", { ns: "focus" }));
          if (current.done) { cleanup(); resolve(current); return; }
        }
      } catch (error) { fail(error); }
      finally {
        querying = false;
        if (!disposed) { timer = setTimeout(() => { void read(); }, dirty ? 100 : 1000); dirty = false; }
      }
    };
    signal.addEventListener("abort", abort, { once: true });
    if (signal.aborted) { abort(); return; }
    void onSchedulerUpdated(snapshot => {
      if (disposed || !operation || snapshot.run_id !== operation.run_id || snapshot.version <= version) return;
      dirty = true;
      if (!querying && timer === undefined) timer = setTimeout(() => { void read(); }, 100);
    }).then(async off => {
      if (disposed) { off(); return; }
      unlisten = off;
      operation = await submit();
      if (disposed) return;
      if (!operation) { cleanup(); resolve(null); return; }
      started();
      await read(); // Also catches completion before command acknowledgement.
    }).catch(fail);
  });
}
