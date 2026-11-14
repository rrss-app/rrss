import { formatError } from "../../lib/errors";
import { memo, useCallback, useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";
import type { ChatMessageView, ChatStreamEvent } from "../../lib/api";
import { BlockView } from "../../components/BlockView";
import { CitationChip } from "../../components/CitationChip";
import { ErrorBoundary } from "../../components/ErrorBoundary";
import { Icon } from "../../components/Icon";
import { useTranslation } from "../../i18n";
import { threadSlot, useConversation, useFailure, useOutgoing, type ChatFailure, type ConversationStore, type Outgoing } from "./conversationStore";
import { FailureNotice, type ChatFixTab } from "./ChatFailure";
import { useFollowBottom, type JumpController } from "./useFollowBottom";
import { TraceRow, WaitingLine, liveTrace } from "./TurnProgress";
import { createTurnStore, pausedSince, useLiveTurn, useTraces, type LiveTurn, type Trace, type TurnStore } from "./turnTrace";

export function localArticleId(href: string): number | null {
  const match = /^rrss:\/\/article\/([1-9]\d*)$/.exec(href);
  if (!match) return null;
  const id = Number(match[1]);
  return Number.isSafeInteger(id) ? id : null;
}

type OpenProps = {
  onOpenUrl: (href: string) => void;
  onOpenArticle?: (id: number, anchor?: { blockId: number }) => void;
};

function makeOpenUrl({ onOpenUrl, onOpenArticle }: OpenProps) {
  return (href: string) => {
    const id = localArticleId(href);
    if (id !== null) onOpenArticle?.(id);
    else onOpenUrl(href);
  };
}

function MessageFallback() {
  const { t } = useTranslation("chat");
  return (
    <p className="muted message-error" role="alert">
      <Icon name="info" size={13} />{t("messages.renderFailed")}
    </p>
  );
}

type BubbleState = {
  /** 自己刚发出、宿主还没接受：半透明。 */
  pending?: boolean;
  /** 这一次才出现（发出的提问、开始出字的回复）：播一次进场；从历史读回的不播。 */
  enter?: boolean;
};

const MessageBubble = memo(function MessageBubble({ message: m, onOpenUrl, onOpenArticle, pending, enter }: { message: ChatMessageView } & OpenProps & BubbleState) {
  const openUrl = makeOpenUrl({ onOpenUrl, onOpenArticle });
  return (
    <div className={`bubble ${m.role}`} data-message-id={m.id > 0 ? m.id : undefined} data-pending={pending ? "" : undefined} data-enter={enter ? "" : undefined}>
      <ErrorBoundary scope="chat" fallback={() => <MessageFallback />}>
        {m.citations?.length ? (
          <div className="message-citations">
            {m.citations.map((citation, index) => <CitationChip key={`${citation.article_id}-${index}`} citation={citation} onOpen={onOpenArticle} />)}
          </div>
        ) : m.cite_title ? <div className="message-citations"><CitationChip citation={{ kind: "article", article_id: m.cite_article_id, title: m.cite_title, feed_title: "", url: "", excerpt: "", block_id: -1 }} onOpen={onOpenArticle} /></div> : null}
        <div className="message-body">{m.document ? <BlockView document={m.document} onOpenUrl={openUrl} /> : <p>{m.text}</p>}</div>
      </ErrorBoundary>
    </div>
  );
});

/** 已落库的历史消息：数组引用不变时整段跳过，流式期间不重新遍历。本次运行里的回复带过程摘要。 */
const HistoryMessages = memo(function HistoryMessages({ messages, traces, onOpenUrl, onOpenArticle }: {
  messages: ChatMessageView[]; traces?: ReadonlyMap<number, Trace>;
} & OpenProps) {
  return (
    <>
      {messages.map((m) => {
        const trace = m.role === "assistant" ? traces?.get(m.id) : undefined;
        return trace
          ? <div key={m.id} className="chat-turn"><TraceRow trace={trace} /><MessageBubble message={m} onOpenUrl={onOpenUrl} onOpenArticle={onOpenArticle} /></div>
          : <MessageBubble key={m.id} message={m} onOpenUrl={onOpenUrl} onOpenArticle={onOpenArticle} />;
      })}
    </>
  );
});

/** 待发消息按落库后的同一形态画，换成历史里的那条时不跳。 */
function OutgoingMessage({ outgoing, ...open }: { outgoing: Outgoing } & OpenProps) {
  const message: ChatMessageView = {
    id: -outgoing.token, role: "user", text: outgoing.text, citations: outgoing.citations, document: null,
    streaming: false, cite_article_id: 0, cite_title: "",
  };
  return <MessageBubble message={message} pending={!outgoing.accepted} enter {...open} />;
}

/** 流式回复：整体按段落快照替换；新出现的段落淡入，末尾一枚闪烁的块状光标，结束时随这条一起换成历史。 */
function StreamingMessage({ stream, onOpenUrl, onOpenArticle }: { stream: ChatStreamEvent } & OpenProps) {
  const openUrl = makeOpenUrl({ onOpenUrl, onOpenArticle });
  return (
    <div className="bubble assistant" data-streaming="" data-enter="">
      <ErrorBoundary scope="chat" resetKey={stream.text} fallback={() => <MessageFallback />}>
        {stream.document ? <BlockView document={stream.document} onOpenUrl={openUrl} /> : <p>{stream.text}</p>}
      </ErrorBoundary>
    </div>
  );
}

export type LoadOlderProps = {
  hasMore?: boolean;
  /** 向前加载一页；返回是否真的拼接了更早的消息。 */
  onLoadOlder?: () => Promise<boolean>;
  /** 向前加载时保持可视位置不动的滚动容器；缺省取 `documentElement`。 */
  scrollContainer?: RefObject<HTMLElement | null> | (() => HTMLElement | null);
};

function resolveScroller(scrollContainer: LoadOlderProps["scrollContainer"]): HTMLElement {
  const el = typeof scrollContainer === "function" ? scrollContainer() : scrollContainer?.current;
  return el ?? document.documentElement;
}

/**
 * 在更早消息插到顶部前后保持视口内容不动：记录容器高度差并补回 `scrollTop`。
 * 只在本次加载真正拼接了消息时补偿，失败或被丢弃的结果不改滚动位置。
 */
function useLoadOlder({ onLoadOlder, scrollContainer }: LoadOlderProps, messages: ChatMessageView[]) {
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const anchor = useRef<{ height: number; top: number } | null>(null);
  const firstId = messages[0]?.id;

  useLayoutEffect(() => {
    const pending = anchor.current;
    if (!pending) return;
    const el = resolveScroller(scrollContainer);
    const grown = el.scrollHeight - pending.height;
    // 加载中的那次渲染还没把更早的消息插进来，锚点留到内容变高再消费。
    if (grown <= 0) return;
    anchor.current = null;
    el.scrollTop = pending.top + grown;
  }, [firstId, loading, scrollContainer]);

  const load = async () => {
    if (!onLoadOlder || loading) return;
    const el = resolveScroller(scrollContainer);
    setLoading(true);
    setError("");
    anchor.current = { height: el.scrollHeight, top: el.scrollTop };
    try {
      if (!(await onLoadOlder())) anchor.current = null;
    } catch (err) {
      anchor.current = null;
      setError(formatError(err));
    } finally {
      setLoading(false);
    }
  };
  return { loading, error, load };
}

export type ChatMessagesProps = OpenProps & LoadOlderProps & {
  messages: ChatMessageView[];
  stream: ChatStreamEvent | null;
  /** 刚发出、还没出现在历史里的自己的消息。 */
  outgoing?: Outgoing | null;
  /** 这一处正在进行的一轮：等待过程，开始出字后收成摘要。 */
  live?: LiveTurn | null;
  /** 本次运行里已结束的各轮摘要，按回复消息 id。 */
  traces?: ReadonlyMap<number, Trace>;
  /** 这一处上一轮没能正常结束：显示在回复位置。 */
  failure?: ChatFailure | null;
  /** 失败提示里的「去设置」。 */
  onFix?: (tab: ChatFixTab) => void;
  busy?: boolean;
  emptyText?: string;
  emptyTitle?: string;
};

export function ChatMessages({ messages, stream, outgoing, live, traces, failure, onFix, onOpenUrl, onOpenArticle, busy, emptyText, emptyTitle, hasMore, onLoadOlder, scrollContainer }: ChatMessagesProps) {
  const { t } = useTranslation("chat");
  const older = useLoadOlder({ onLoadOlder, scrollContainer }, messages);
  const answering = live ? liveTrace(live) : null;
  const paused = live && answering ? pausedSince(live) : null;
  return (
    <>
      {hasMore && onLoadOlder ? (
        <div className="chat-load-older">
          <button type="button" className="btn sm ghost" disabled={older.loading} onClick={() => void older.load()}>
            {older.loading ? <><Icon name="spinner" size={13} className="spin" />{t("messages.loadingOlder")}</> : t("messages.loadOlder")}
          </button>
          {older.error ? <span className="muted" role="alert">{t("messages.loadOlderFailed", { error: older.error })}</span> : null}
        </div>
      ) : null}
      {messages.length === 0 && !stream?.text && !busy && !outgoing && !live && !failure && emptyText ? (
        <div className="chat-empty">
          <span className="empty-icon"><Icon name="chat" size={22} /></span>
          {emptyTitle ? <strong>{emptyTitle}</strong> : null}
          <p className="muted">{emptyText}</p>
        </div>
      ) : null}
      <HistoryMessages messages={messages} traces={traces} onOpenUrl={onOpenUrl} onOpenArticle={onOpenArticle} />
      {outgoing ? <OutgoingMessage key={`outgoing-${outgoing.token}`} outgoing={outgoing} onOpenUrl={onOpenUrl} onOpenArticle={onOpenArticle} /> : null}
      {/* 回复位置：还没出字是一行等待过程；开始出字后收成摘要，挂在流式回复上方。 */}
      {live && !answering ? <WaitingLine turn={live} /> : null}
      {answering && stream?.text ? <TraceRow trace={answering} enter /> : null}
      {stream?.text ? <StreamingMessage stream={stream} onOpenUrl={onOpenUrl} onOpenArticle={onOpenArticle} /> : null}
      {/* 出字后又去调用工具：回复下方再显示一行等待过程，直到接着出字。 */}
      {live && paused !== null && stream?.text ? <WaitingLine turn={live} since={paused} /> : null}
      {/* 发送与新一轮开始时运行层会清掉；进行中的一轮不显示上一轮的失败。 */}
      {failure && !live ? <FailureNotice failure={failure} onFix={onFix} /> : null}
      {busy && !live && !stream?.text ? (
        <p className="muted chat-status" role="status">
          <Icon name="spinner" size={14} className="spin" />{t("messages.working")}
        </p>
      ) : null}
    </>
  );
}

export type ThreadMessagesProps = OpenProps & LoadOlderProps & {
  store: ConversationStore;
  threadId: number;
  /** 输入框上下文：还没有线程时用它找这次发出的待发消息。 */
  context?: string;
  /** 运行层的等待过程（`useConversationRuntime().turns`）。 */
  turns?: TurnStore;
  /** 失败提示里的「去设置」。 */
  onFix?: (tab: ChatFixTab) => void;
  busy: boolean;
  emptyText?: string;
  emptyTitle?: string;
};

/** 只订阅自己线程的消息区；其他线程的流式更新不会触发这里。 */
export function ThreadMessages({ store, threadId, context = "chat", turns, ...rest }: ThreadMessagesProps) {
  const conversation = useConversation(store, threadId);
  const slot = threadSlot(threadId, context);
  const outgoing = useOutgoing(store, slot);
  const live = useLiveTurn(turns ?? NO_TURNS, slot, threadId);
  const traces = useTraces(turns ?? NO_TURNS);
  const failure = useFailure(store, slot);
  return <ChatMessages messages={conversation.messages} stream={conversation.stream} outgoing={outgoing} live={live} traces={traces} failure={failure} hasMore={conversation.hasMore} {...rest} />;
}

/** 没接运行层的消息区（测试、预览）用的空等待过程。 */
const NO_TURNS = createTurnStore();

/**
 * 已发出的消息归到新线程后，「新对话」那一格不再显示它。换走时清掉已被接受的那条；还在发送中的留着，
 * 失败时由发送方撤回。
 */
export function useReleaseOutgoing(store: ConversationStore, slot: string) {
  useEffect(() => () => store.setOutgoing(slot, (current) => (current?.accepted ? null : current)), [store, slot]);
}

export type ChatPaneProps = Omit<ThreadMessagesProps, "onLoadOlder" | "scrollContainer"> & {
  onLoadOlder?: (threadId: number) => Promise<boolean>;
  /** 「回到最新」按钮（挂在输入区上方）与对话区之间的连接。 */
  jump?: JumpController;
};

export function ChatPane({ onLoadOlder, jump, store, threadId, context = "chat", turns, ...rest }: ChatPaneProps) {
  const { t } = useTranslation("chat");
  const scroller = useRef<HTMLDivElement>(null);
  const column = useRef<HTMLDivElement>(null);
  const slot = threadSlot(threadId, context);
  const conversation = useConversation(store, threadId);
  const outgoing = useOutgoing(store, slot);
  const live = useLiveTurn(turns ?? NO_TURNS, slot, threadId);
  const traces = useTraces(turns ?? NO_TURNS);
  const failure = useFailure(store, slot);
  useReleaseOutgoing(store, slot);
  // 从「新对话」发出后切到刚建的线程，是同一段对话的延续，不当作换对话。
  const fadeOnSwitch = useCallback((previous: string) => store.getOutgoing(previous)?.threadId !== threadId, [store, threadId]);
  const last = conversation.messages.at(-1)?.id ?? 0;
  useFollowBottom({
    scroller, content: column, threadKey: slot, controller: jump, fadeOnSwitch,
    tail: `${last}:${conversation.stream?.text.length ?? 0}:${failure ? 1 : 0}`,
    sent: outgoing && !outgoing.accepted ? outgoing.token : 0,
  });
  const loadOlder = onLoadOlder ? () => onLoadOlder(threadId) : undefined;
  return (
    <div className="scroll chat-pane" ref={scroller}>
      <div className="chat-column" ref={column}>
        <ChatMessages
          {...rest}
          messages={conversation.messages}
          stream={conversation.stream}
          outgoing={outgoing}
          live={live}
          traces={traces}
          failure={failure}
          hasMore={conversation.hasMore}
          onLoadOlder={loadOlder}
          scrollContainer={scroller}
          emptyTitle={t("messages.emptyTitle")}
          emptyText={t("messages.emptyText")}
        />
      </div>
    </div>
  );
}
