import { useEffect, useRef, useState } from "react";
import type { ReaderView, TranslationStartMode } from "../../bindings";
import type { ReaderTicket } from "./readerRequests";
import { createTranslationSession, type TranslationSessionView } from "./translationSession";
import { observeReaderViewport } from "./readerViewport";

const EMPTY: TranslationSessionView = { data: null, autoBilingual: false, busy: false, disconnected: false, issue: false, failure: null, note: "" };
export function readerHasSelection() {
  const selection = window.getSelection();
  return !!selection && !selection.isCollapsed && !!selection.anchorNode?.parentElement?.closest(".drawer.reader [data-reader-document]");
}

export function useReaderTranslation(reader: ReaderView | null, ticket: ReaderTicket | null, owns: (ticket: ReaderTicket) => boolean,
  onReader: (reader: ReaderView) => void, onFirstBody: () => void, settingsUpdatedAt: number) {
  const [view, setView] = useState<{ ticket: ReaderTicket; sourceHash: string; value: TranslationSessionView } | null>(null);
  const controller = useRef<ReturnType<typeof createTranslationSession> | null>(null);
  const [body, setBody] = useState<HTMLDivElement | null>(null);
  const viewport = useRef<ReturnType<typeof observeReaderViewport> | null>(null);
  const latest = useRef({ reader, owns, onReader, onFirstBody });
  latest.current = { reader, owns, onReader, onFirstBody };
  const sourceHash = reader?.source_hash;
  useEffect(() => {
    const source = latest.current.reader;
    if (!ticket || !source) return;
    let pending: TranslationSessionView | null = null, disposed = false, switched = false, selecting = false;
    let frame: number | undefined, timeout: ReturnType<typeof setTimeout> | undefined;
    const flush = () => {
      if (disposed || !latest.current.owns(ticket) || !pending) return;
      // 暂存的是已合并的一份最新覆盖；选区再久也不会积累事件 / 文档副本。
      if ((selecting || readerHasSelection()) && pending.data && pending.data.snapshot.phase !== "invalidated") return;
      const value = pending; pending = null;
      if (!switched && value.autoBilingual && value.data?.ready) {
        switched = true;
        latest.current.onFirstBody();
      }
      setView({ ticket, sourceHash: source.source_hash, value });
    };
    const changed = (value: TranslationSessionView) => {
      pending = value;
      if (frame !== undefined) cancelAnimationFrame(frame);
      clearTimeout(timeout);
      frame = requestAnimationFrame(flush);
      timeout = setTimeout(flush, 80);
    };
    const session = createTranslationSession(source, changed, next => {
      if (!disposed && latest.current.owns(ticket)) latest.current.onReader(next);
    });
    controller.current = session;
    const down = (event: PointerEvent) => { selecting = event.button === 0 && event.target instanceof Element && !!event.target.closest(".drawer.reader [data-reader-document]"); };
    const up = () => { selecting = false; flush(); };
    document.addEventListener("pointerdown", down);
    document.addEventListener("pointerup", up);
    document.addEventListener("pointercancel", up);
    document.addEventListener("selectionchange", flush);
    return () => {
      disposed = true; pending = null;
      if (frame !== undefined) cancelAnimationFrame(frame);
      clearTimeout(timeout); session.dispose();
      document.removeEventListener("pointerdown", down);
      document.removeEventListener("pointerup", up);
      document.removeEventListener("pointercancel", up);
      document.removeEventListener("selectionchange", flush);
      if (controller.current === session) controller.current = null;
    };
  }, [ticket, sourceHash]);
  useEffect(() => {
    if (!body || !ticket) return;
    const observer = observeReaderViewport(body, value => controller.current?.setViewport(value));
    viewport.current = observer;
    return () => { observer.dispose(); if (viewport.current === observer) viewport.current = null; };
  }, [body, ticket, sourceHash]);
  const lastSettingsRead = useRef(settingsUpdatedAt);
  useEffect(() => {
    const previous = lastSettingsRead.current;
    lastSettingsRead.current = settingsUpdatedAt;
    // 设置保存会统一刷新查询；已落库的终态也需重查配置身份，不发起翻译。
    if (previous && settingsUpdatedAt !== previous) void controller.current?.recover();
  }, [settingsUpdatedAt]);
  return {
    state: view?.ticket === ticket && view.sourceHash === sourceHash ? view.value : EMPTY,
    onBody: setBody,
    start: (mode: TranslationStartMode = "default") => { viewport.current?.read(); return controller.current?.start(mode); },
    recover: () => controller.current?.recover(),
    retrySave: () => controller.current?.retrySave(),
    detach: () => { viewport.current?.dispose(); viewport.current = null; controller.current?.dispose(); controller.current = null; },
  };
}
