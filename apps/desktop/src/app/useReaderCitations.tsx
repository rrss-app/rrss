import { useRef, useState } from "react";
import { api, type CitationView, type ThreadView } from "../lib/api";
import { formatNotice } from "../lib/notice";
import { i18n } from "../i18n";
import type { DraftStore, useConversationRuntime } from "../features/chat";
import { ReaderCitationTray, citationTargetContext, type ReaderCitationSource } from "../features/reader";

/** 根壳的跨域适配：阅读器只收集引用，对话运行层仍负责发送，唯一草稿在 drafts。 */
export function useReaderCitations({ drafts, chat, getThreads, getSourceArticle, onSent, notify, inert }: {
  drafts: DraftStore; chat: ReturnType<typeof useConversationRuntime>; getThreads: () => ThreadView[];
  getSourceArticle: () => Readonly<{ id: number; title: string }> | null;
  onSent: (id: number, digestId: number | null, sameReader: boolean) => void; notify: (notice: { message: string }) => void; inert: boolean;
}) {
  const [origins, setOrigins] = useState<ReadonlyMap<number, Readonly<{ id: number; title: string }>>>(new Map());
  const [focusRequest, setFocusRequest] = useState(0);
  const [target, setTarget] = useState<ReaderCitationSource>({ kind: "new" });
  const [source, setSource] = useState<ReaderCitationSource>({ kind: "new" });
  const [visible, setVisible] = useState(false);
  const [latestCitation, setLatestCitation] = useState<CitationView | null>(null);
  const [resolving, setResolving] = useState(false);
  const epoch = useRef(0);
  const targetRef = useRef(target);
  targetRef.current = target;
  const sendLock = useRef(false);
  const callbacks = useRef({ onSent, notify });
  callbacks.current = { onSent, notify };
  const context = citationTargetContext(drafts, target);
  const version = epoch.current;
  const begin = (next: ReaderCitationSource) => {
    ++epoch.current;
    setFocusRequest(0);
    targetRef.current = next;
    setTarget(next); setSource(next);
    setVisible(drafts.get(citationTargetContext(drafts, next)).citations.length > 0);
    setLatestCitation(null);
    if (next.kind === "thread") chat.seedThread(next.threadId, getThreads().find(t => t.id === next.threadId));
  };
  const end = () => { ++epoch.current; setVisible(false); };
  const add = (citation: CitationView, expected = version) => {
    if (expected !== epoch.current) return;
    const key = citationTargetContext(drafts, targetRef.current);
    drafts.setCitations(key, cs => {
      if (citation.kind === "article" && cs.some(c => c.kind === "article" && c.article_id === citation.article_id)) return cs;
      return [...cs, citation];
    });
    setFocusRequest(0); setLatestCitation(citation); setVisible(true);
  };
  const changeTarget = (next: ReaderCitationSource) => {
    drafts.move(context, citationTargetContext(drafts, next));
    targetRef.current = next; setTarget(next);
    if (next.kind === "thread") chat.seedThread(next.threadId, getThreads().find(t => t.id === next.threadId));
  };
  const send = async (text: string, citations: CitationView[]) => {
    if (sendLock.current) return false;
    const origin = getSourceArticle();
    sendLock.current = true; setResolving(true);
    try {
      let id = target.kind === "thread" ? target.threadId : 0;
      const digestId = target.kind === "digest" ? target.digestId : null;
      if (digestId !== null) {
        const thread = await api.digestThread(digestId);
        id = thread.thread_id;
        chat.seedThread(id, thread);
        drafts.bindThread(id, context);
      }
      id = await chat.sendMessage(id, text, citations, digestId, context);
      if (digestId !== null) drafts.bindThread(id, context);
      const sameReader = version === epoch.current;
      if (sameReader) {
        setVisible(false);
        if (origin) setOrigins(previous => new Map(previous).set(id, origin));
      }
      callbacks.current.onSent(id, digestId, sameReader);
      return true;
    } catch (error) {
      callbacks.current.notify({ message: formatNotice(i18n.t("citation.sendFailed", { ns: "reader" }), error) });
      return false;
    } finally { sendLock.current = false; setResolving(false); }
  };
  return {
    begin, end, add, origins,
    captureAdd: () => { const expected = epoch.current; return (chip: CitationView) => add(chip, expected); },
    ask: () => { setVisible(true); setFocusRequest(n => n + 1); },
    escape: () => {
      if (!visible) return false;
      if (drafts.get(context).citations.length === 0) setVisible(false);
      return true;
    },
    tray: visible ? <ReaderCitationTray drafts={drafts} context={context} focusRequest={focusRequest} target={target} source={source} inert={inert}
      busy={resolving || chat.sending || chat.busyThreadId !== null} sending={resolving || chat.sending}
      latestCitation={latestCitation} getThreads={getThreads} onTarget={changeTarget} onSend={send}
      onClear={() => { drafts.setCitations(context, () => []); if (!drafts.get(context).text) setVisible(false); }} /> : null,
  };
}
