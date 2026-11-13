import { useEffect, useRef } from "react";
import type { QueryClient } from "@tanstack/react-query";
import { api, onFeedUpdated, onSchedulerUpdated, type SchedulerSnapshot } from "../lib/api";
import { isTauri } from "../lib/preview";
import { i18n } from "../i18n";
import { feedKeys, feedQueryOptions } from "../lib/feedQuery";
import type { FocusSession, FocusNotice } from "../features/focus";
import type { FeedsSession } from "../features/feeds";

/** One subscription in the root; merge bursts and preserve drafts owned by settings. */
export function useFeedUpdates(queries: QueryClient, focus: FocusSession, column: FocusSession,
  feeds: FeedsSession, columnOpen: boolean, notify: (notice: FocusNotice) => void) {
  const latest = useRef({ columnOpen, notify });
  latest.current = { columnOpen, notify };
  useEffect(() => {
    if (!isTauri()) return;
    let disposed = false;
    let dirty = false;
    let running = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let unlisten: (() => void) | undefined;
    let unlistenStatus: (() => void) | undefined;
    let statusRun: string | undefined;
    let statusVersion = -1;
    let pause: string | null = null;
    let statusRead = false;
    const statusChanged = (snapshot: SchedulerSnapshot) => {
      if (disposed || snapshot.stopped) return;
      if (statusRun && snapshot.run_id !== statusRun) {
        if (!statusRead) void readStatus(true);
        return;
      }
      if (snapshot.version < statusVersion) return;
      statusRun = snapshot.run_id;
      statusVersion = snapshot.version;
      if (snapshot.feed_pause && snapshot.feed_pause !== pause) latest.current.notify({ message: snapshot.feed_pause });
      pause = snapshot.feed_pause;
    };
    const readStatus = async (restarted = false) => {
      if (statusRead || disposed) return;
      statusRead = true;
      const version = statusVersion;
      try {
        const snapshot = await api.schedulerStatus();
        if (disposed || statusVersion !== version) return;
        if (restarted) { statusRun = undefined; statusVersion = -1; }
        statusChanged(snapshot);
      } catch { /* Feed updates and explicit refresh retain their own recovery. */ }
      finally { statusRead = false; }
    };
    const flush = async () => {
      timer = undefined;
      if (disposed || running || !dirty) return;
      if (queries.isMutating({ mutationKey: feedKeys.write })) {
        timer = setTimeout(() => { void flush(); }, 100);
        return;
      }
      dirty = false;
      running = true;
      try {
        // Cancel older IPC snapshots before rereading; no settings-wide invalidation.
        await queries.cancelQueries({ queryKey: feedKeys.tree, exact: true });
        await queries.invalidateQueries({ queryKey: feedKeys.tree, exact: true, refetchType: "none" });
        const tree = await queries.fetchQuery(feedQueryOptions());
        // Background data changes keep the focus viewport; only user actions rebuild the list.
        if (!disposed) await Promise.all([
          focus.backgroundChanged(tree), feeds.invalidate(),
          latest.current.columnOpen ? column.backgroundChanged(tree) : undefined,
        ]);
      } catch {
        if (!disposed) latest.current.notify({ message: i18n.t("updates.readFailed", { ns: "feeds" }) });
      } finally {
        running = false;
        if (!disposed && dirty) timer = setTimeout(() => { void flush(); }, 100);
      }
    };
    const changed = () => {
      if (disposed) return;
      dirty = true;
      if (!running && timer === undefined) timer = setTimeout(() => { void flush(); }, 100);
    };
    void onFeedUpdated(changed).then(off => {
      if (disposed) off();
      else { unlisten = off; changed(); }
    }).catch(() => {
      if (!disposed) latest.current.notify({ message: i18n.t("updates.listenFailed", { ns: "feeds" }) });
    });
    void onSchedulerUpdated(statusChanged).then(off => {
      if (disposed) off();
      else { unlistenStatus = off; void readStatus(); }
    }).catch(() => { /* Explicit refresh queries still report the current pause. */ });
    return () => { disposed = true; if (timer !== undefined) clearTimeout(timer); unlisten?.(); unlistenStatus?.(); };
  }, [queries, focus, column, feeds]);
}
