import { useCallback, useEffect, useRef, useState } from "react";
import { DigestPane } from "../../components/DigestPane";
import { Icon } from "../../components/Icon";
import { MainViewFrame } from "../../components/MainViewFrame";
import { api, openExternal, type CitationView, type DigestView } from "../../lib/api";
import { formatNotice } from "../../lib/notice";
import { i18n, useTranslation } from "../../i18n";
import { isTauri } from "../../lib/preview";
import type { SelectedBlock } from "../../lib/selection";
import { ChatActivity, ChatContextMeter, ChatModelChip, Composer, useConversation, type ChatFixTab, type DraftStore, type useConversationRuntime } from "../chat";
import { DigestDiscussion } from "./DigestDiscussion";

type Notice = { message: string };

/**
 * 简报的选期、追问加载、引用与发送归属在本域处理，根壳只装配跨域入口。
 * 第一次打开后一直挂着，离开时只隐藏；追问只随简报 id 加载和回收，不随可见性重拉。
 */
export function DigestScreen({ active, topmost = true, chat, drafts, onContext, onChange, onOpenArticle, onOpenUrl = openExternal, onManagePool, onFixChat, notify, discussionRevision = 0 }: {
  onOpenUrl?: (href: string) => void;
  active: boolean;
  topmost?: boolean;
  /** 阅读器向本期追问发送后，重读线程归属（其余切页保持保活）。 */
  discussionRevision?: number;
  chat: ReturnType<typeof useConversationRuntime>;
  drafts: DraftStore;
  onContext: (context: string) => void;
  onChange: () => Promise<void>;
  onOpenArticle: (id: number, anchor?: { blockId: number }) => void;
  /** 模型选择列表里的「管理模型池」。 */
  onManagePool: () => void;
  /** 追问失败提示里的「去设置」。 */
  onFixChat?: (tab: ChatFixTab) => void;
  notify: (notice: Notice) => void;
}) {
  const { t } = useTranslation("digest");
  const [digest, setDigest] = useState<DigestView | null>(null);
  const [discussion, setDiscussion] = useState({ digestId: 0, threadId: 0, loading: false, error: "" });
  const [retry, setRetry] = useState(0);
  const [scroll, setScroll] = useState(0);
  const [citing, setCiting] = useState(false);
  const citingRef = useRef(false);
  const generation = useRef(0);
  const threads = useRef(new Map<number, Awaited<ReturnType<typeof api.digestThread>>>());
  const latest = useRef({ active, digest });
  latest.current = { active, digest };
  const lastDigest = useRef<DigestView | null>(null);
  const root = useRef<HTMLDivElement>(null);
  const digestId = digest?.id ?? 0;
  const { hydrate, release, seedThread } = chat;
  const ready = !!digest && discussion.digestId === digest.id && !discussion.loading && !discussion.error;
  const threadId = ready ? discussion.threadId : 0;
  const onCurrentChange = useCallback((next: DigestView | null) => {
    latest.current = { ...latest.current, digest: next };
    setDigest(next);
    if (next) lastDigest.current = next;
    onContext(`digest:${next?.id ?? 0}`);
  }, [onContext]);

  useEffect(() => {
    if (!digestId) return;
    const gen = ++generation.current;
    let cancelled = false;
    // 加载过的那期切回来时先用上次取到的追问记录（离开时对话仓库可能已回收），追问区不卸载再出现；后台照常核对。
    const known = threads.current.get(digestId);
    if (known) hydrate(known.thread_id, known.messages, known.has_more);
    setDiscussion({ digestId, threadId: known?.thread_id ?? 0, loading: !known, error: "" });
    void api.digestThread(digestId).then((data) => {
      if (cancelled || gen !== generation.current) return;
      threads.current.set(digestId, data);
      drafts.bindThread(data.thread_id, `digest:${digestId}`);
      hydrate(data.thread_id, data.messages, data.has_more);
      seedThread(data.thread_id, data);
      setDiscussion({ digestId, threadId: data.thread_id, loading: false, error: "" });
    }).catch((err) => {
      if (cancelled || gen !== generation.current) return;
      setDiscussion({ digestId, threadId: 0, loading: false, error: isTauri() ? formatNotice(i18n.t("discussion.loadFailed", { ns: "digest" }), err) : "" });
    });
    return () => { cancelled = true; };
  }, [digestId, retry, hydrate, seedThread, drafts, discussionRevision]);
  useEffect(() => {
    if (threadId <= 0) return;
    return () => { release(threadId); };
  }, [threadId, release]);

  // 正文滚到底（离底 40px 内，或短到不用滚）时，底部的追问台子展开成输入卡。
  const [atBottom, setAtBottom] = useState(false);
  useEffect(() => {
    const scroller = root.current?.querySelector<HTMLElement>(".digest .scroll");
    if (!scroller) return;
    const check = () => setAtBottom(scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 40);
    check();
    scroller.addEventListener("scroll", check, { passive: true });
    const resize = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(check);
    resize?.observe(scroller);
    if (scroller.firstElementChild) resize?.observe(scroller.firstElementChild);
    return () => { scroller.removeEventListener("scroll", check); resize?.disconnect(); };
  }, [digestId, ready]);
  // 「/」聚焦追问：只在简报可见、焦点不在输入控件、前面没有叠层时。
  useEffect(() => {
    if (!active) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "/" || event.ctrlKey || event.metaKey || event.altKey || event.defaultPrevented || event.isComposing) return;
      const target = event.target as HTMLElement | null;
      if (target?.closest("input, textarea, select, [contenteditable='true'], [role='dialog'], [role='menu']")) return;
      const box = root.current;
      if (!box || box.closest("[inert]")) return;
      const input = box.querySelector<HTMLTextAreaElement>(".composer textarea");
      if (!input || input.disabled) return;
      event.preventDefault();
      input.focus();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [active]);
  const asked = useConversation(chat.store, threadId).messages.filter((m) => m.role === "user").length;

  const citeSelection = async (source: DigestView, blocks: SelectedBlock[]) => {
    if (citingRef.current || chat.sending) return;
    citingRef.current = true;
    setCiting(true);
    try {
      const incoming = await api.citeDigestSelection(source.id, source.revision, blocks);
      // 引用结果只按简报 id / revision 归属；切走期间完成的也写进这一期的草稿。
      if (latest.current.digest?.id !== source.id || latest.current.digest.revision !== source.revision) return;
      drafts.setCitations(`digest:${source.id}`, (existing) => [...existing, ...incoming.filter((citation) =>
        !existing.some((item) => item.kind === citation.kind && item.article_id === citation.article_id && item.excerpt === citation.excerpt))]);
      notify({ message: i18n.t("cite.added", { ns: "digest", count: new Set(incoming.map((item) => item.article_id)).size }) });
      if (latest.current.active) root.current?.querySelector<HTMLTextAreaElement>(".composer textarea")?.focus();
    } catch (err) {
      if (latest.current.digest?.id === source.id && latest.current.digest.revision === source.revision) {
        notify({ message: formatNotice(i18n.t("cite.failed", { ns: "digest" }), err) });
      }
    }
    finally { citingRef.current = false; setCiting(false); }
  };
  const send = async (text: string, citations: CitationView[]) => {
    if (!digest || !ready || citingRef.current) return false;
    const sentDigestId = digest.id;
    try {
      const id = await chat.sendMessage(threadId, text, citations, sentDigestId, `digest:${sentDigestId}`);
      drafts.bindThread(id, `digest:${sentDigestId}`);
      if (latest.current.digest?.id === sentDigestId) {
        ++generation.current;
        setDiscussion({ digestId: sentDigestId, threadId: id, loading: false, error: "" });
        setScroll((value) => value + 1);
      }
      return true;
    } catch (err) { notify({ message: formatNotice(i18n.t("send.failed", { ns: "digest" }), err) }); return false; }
  };

  return <MainViewFrame visible={active} rootRef={root}>
    <DigestPane onOpenUrl={onOpenUrl} visible={active} topmost={topmost} onChange={onChange} onOpenArticle={onOpenArticle}
      onCiteSelection={chat.sending || citing ? undefined : (source, blocks) => void citeSelection(source, blocks)}
      initialDigest={lastDigest.current} onCurrentChange={onCurrentChange}>
      {discussion.error && discussion.digestId === digestId ? (
        <section className="digest-discussion reader-column" aria-label={t("discussion.region")}>
          <div className="notice error digest-thread-error" role="alert"><Icon name="info" size={15} /><span>{discussion.error}</span>
            <button type="button" className="btn sm" onClick={() => setRetry((value) => value + 1)}>{t("discussion.retry")}</button>
          </div>
        </section>
      ) : ready ? <DigestDiscussion store={chat.store} turns={chat.turns} threadId={threadId} context={`digest:${digestId}`} busy={chat.busyThreadId !== null && chat.busyThreadId === threadId}
        scrollRequest={scroll} onLoadOlder={chat.loadOlder} onOpenUrl={onOpenUrl} onOpenArticle={onOpenArticle} onFix={onFixChat} /> : null}
    </DigestPane>
    <Composer drafts={drafts} context={`digest:${digestId}`} ariaLabel={t("composer.aria")} placeholder={t("composer.placeholder")}
      contextLabel={!digest ? t("composer.noDigest") : discussion.error && discussion.digestId === digestId
        ? t("composer.contextNotLoaded", { date: digest.date_label, period: digest.period_label })
        : !ready ? t("composer.contextLoading", { date: digest.date_label, period: digest.period_label })
        : t("composer.context", { date: digest.date_label, period: digest.period_label })}
      otherThreadBusy={chat.busyThreadId !== null && chat.busyThreadId !== threadId} generating={chat.busyThreadId !== null}
      sending={chat.sending} disabled={!digest} ready={!citing && ready} collapsible atBottom={atBottom}
      dockLabel={<><Icon name="chat" size={14} />{t("composer.dock")}{asked ? <span className="composer-dock-count"> · {t("composer.dockCount", { count: asked })}</span> : null}</>}
      forceExpanded={chat.busyThreadId !== null || !digest} showWhenEmpty onSend={send}
      activity={active ? <ChatActivity store={chat.activity} /> : null}
      model={<ChatModelChip value={chat.modelFor(threadId, `digest:${digestId}`)} thinking={chat.thinkingFor(threadId, `digest:${digestId}`)}
        disabled={chat.sending} onManagePool={onManagePool}
        onChange={(choice, label) => chat.chooseModel(threadId, `digest:${digestId}`, choice, label)}
        onThinking={(level, label) => chat.chooseThinking(threadId, `digest:${digestId}`, level, label)} />}
      meter={digest ? <ChatContextMeter value={chat.modelFor(threadId, `digest:${digestId}`)} usage={chat.contextFor(threadId)} /> : null}
      onStop={() => void api.abort().catch((err) => notify({ message: formatNotice(i18n.t("send.stopFailed", { ns: "digest" }), err) }))} onOpenArticle={onOpenArticle} />
  </MainViewFrame>;
}
