import { useRef, useState } from "react";
import { type CitationView, type ThreadView } from "../lib/api";
import { formatNotice } from "../lib/notice";
import { i18n } from "../i18n";
import type { DraftStore, useConversationRuntime } from "../features/chat";
import { ReaderCitationTray, citationTargetContext, type ReaderCitationSource } from "../features/reader";

/** 根壳的跨域适配：阅读器只收集引用，对话运行层仍负责发送，唯一草稿在 drafts。 */
export function useReaderCitations({ drafts, chat, getThreads, getSourceArticle, onSending, onDigestSent, notify, inert }: {
  drafts: DraftStore; chat: ReturnType<typeof useConversationRuntime>; getThreads: () => ThreadView[];
  getSourceArticle: () => Readonly<{ id: number; title: string }> | null;
  onSending: (id: number, context: string) => (threadId: number) => void;
  onDigestSent: () => void;
  notify: (notice: { message: string }) => void;
  inert: boolean;
}) {
  const [origins, setOrigins] = useState<ReadonlyMap<number | string, Readonly<{ id: number; title: string }>>>(new Map());
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
  const adoptOrigin = (from: number | string, id: number) => {
    if (from === id) return;
    setOrigins(previous => {
      const origin = previous.get(from);
      if (!origin) return previous;
      const next = new Map(previous);
      next.delete(from);
      next.set(id, origin);
      return next;
    });
  };
  const send = async (text: string, citations: CitationView[]) => {
    if (sendLock.current || chat.sending || chat.busyThreadId !== null) return false;
    const origin = getSourceArticle();
    sendLock.current = true; setResolving(true);
    const targetId = target.kind === "thread" ? target.threadId : 0;
    const digestId = target.kind === "digest" ? target.digestId : null;
    const slot = targetId || context;
    // 发送失败时把这次改动过的来源还原，不让没发出去的问题留下「回到文章」。
    const before = origins;
    const touched = new Set<number | string>([slot]);
    if (origin) setOrigins(previous => new Map(previous).set(slot, origin));
    setVisible(false);
    const follow = onSending(targetId, context);
    const adoptThread = (id: number) => {
      if (digestId !== null) drafts.bindThread(id, context);
      touched.add(id);
      adoptOrigin(slot, id);
      follow(id);
    };
    try {
      await chat.sendMessage(targetId, text, citations, digestId, context, {
        resolveDigestThread: digestId !== null,
        onResolved: adoptThread,
        onAccepted: id => {
          adoptThread(id);
          if (digestId !== null) onDigestSent();
        },
      });
      return true;
    } catch (error) {
      if (origin) setOrigins(previous => {
        const next = new Map(previous);
        for (const key of touched) {
          const kept = before.get(key);
          if (kept) next.set(key, kept);
          else next.delete(key);
        }
        return next;
      });
      notify({ message: formatNotice(i18n.t("citation.sendFailed", { ns: "reader" }), error) });
      return false;
    } finally { sendLock.current = false; setResolving(false); }
  };
  return {
    begin, end, add, origins, adoptOrigin,
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
