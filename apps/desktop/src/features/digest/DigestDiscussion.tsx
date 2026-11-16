import { useEffect, useLayoutEffect, useRef, type RefObject } from "react";
import { Icon } from "../../components/Icon";
import { useTranslation } from "../../i18n";
import { reducedMotion } from "../../lib/motion";
import { ChatMessages, createTurnStore, threadSlot, useConversation, useFailure, useLiveTurn, useOutgoing, useReleaseOutgoing, useTraces, type ChatFixTab, type ConversationStore, type TurnStore } from "../chat";

export type DigestDiscussionProps = {
  store: ConversationStore;
  /** 运行层的等待过程；缺省（测试）时没有。 */
  turns?: TurnStore;
  /** 当期简报对应的追问线程；0 表示尚无线程。 */
  threadId: number;
  /** 这一期的输入框上下文：还没有线程时用它找刚发出的追问。 */
  context: string;
  busy: boolean;
  /** 向前加载更早的追问记录；滚动补偿作用在简报页自己的滚动容器上。 */
  onLoadOlder?: (threadId: number) => Promise<boolean>;
  onOpenUrl: (href: string) => void;
  onOpenArticle: (id: number, anchor?: { blockId: number }) => void;
  /** 失败提示里的「去设置」。 */
  onFix?: (tab: ChatFixTab) => void;
};

const NO_TURNS = createTurnStore();

/** 当期追问区：只订阅自己线程；没有消息、没有流式正文也不在生成时整段不渲染。 */
export function DigestDiscussion({ store, turns, threadId, context, busy, onLoadOlder, onOpenUrl, onOpenArticle, onFix }: DigestDiscussionProps) {
  const { t } = useTranslation("digest");
  const conversation = useConversation(store, threadId);
  const slot = threadSlot(threadId, context);
  const outgoing = useOutgoing(store, slot);
  const live = useLiveTurn(turns ?? NO_TURNS, slot, threadId);
  const traces = useTraces(turns ?? NO_TURNS);
  const failure = useFailure(store, slot);
  useReleaseOutgoing(store, slot);
  const section = useRef<HTMLElement>(null);
  const active = conversation.messages.length > 0 || !!conversation.stream?.text || busy || !!outgoing || !!live || !!failure;
  useFollowAfterSend(section, active, {
    sent: outgoing && !outgoing.accepted ? outgoing.token : 0,
    tail: `${conversation.messages.at(-1)?.id ?? 0}:${conversation.stream?.text.length ?? 0}:${live ? 1 : 0}:${failure ? 1 : 0}`,
  });

  if (!active) return null;
  return (
    <section ref={section} className="digest-discussion reader-column" aria-label={t("discussion.region")}>
      <div className="chat-column">
        <h2><Icon name="chat" size={15} />{t("discussion.title")}</h2>
        <ChatMessages
          messages={conversation.messages}
          stream={conversation.stream}
          outgoing={outgoing}
          live={live}
          traces={traces}
          failure={failure}
          onFix={onFix}
          hasMore={conversation.hasMore}
          onLoadOlder={onLoadOlder ? () => onLoadOlder(threadId) : undefined}
          scrollContainer={() => section.current?.closest<HTMLElement>(".scroll") ?? null}
          busy={busy}
          onOpenUrl={onOpenUrl}
          onOpenArticle={onOpenArticle}
        />
      </div>
    </section>
  );
}

/** 离简报底部多近算贴底：内容增长时跟着到底。 */
const FOLLOW_PX = 48;

const scrollerOf = (section: RefObject<HTMLElement | null>) => section.current?.closest<HTMLElement>(".scroll") ?? null;

/**
 * 追问发出后滚到简报底部，并在回复增长时贴底跟随；用户往上翻就停，回到底部再跟。
 * 简报是先读正文的页面，打开时不落到底，只有自己发出追问才开始跟随（对话页的 `useFollowBottom` 打开就贴底，不合用）。
 */
function useFollowAfterSend(section: RefObject<HTMLElement | null>, active: boolean, { sent, tail }: { sent: number; tail: string }) {
  const following = useRef(false);

  // 滚动容器随追问区出现才找得到，所以跟着 active 重新挂。
  useEffect(() => {
    const el = scrollerOf(section);
    if (!active || !el) return;
    let lastTop = el.scrollTop;
    const onScroll = () => {
      const distance = el.scrollHeight - el.clientHeight - el.scrollTop;
      if (distance <= FOLLOW_PX) following.current = true;
      else if (el.scrollTop < lastTop) following.current = false;
      lastTop = el.scrollTop;
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    // 末尾以外的长高也要跟上：输入区展开 / 过程条出现会改滚动容器的底部留白，消息区本身也会重排。
    // 观察容器的内容框（留白变了它就变）和追问区。
    const pin = () => {
      if (following.current && el.clientHeight > 0) el.scrollTop = el.scrollHeight;
    };
    const resize = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(pin);
    resize?.observe(el);
    if (section.current) resize?.observe(section.current);
    return () => {
      el.removeEventListener("scroll", onScroll);
      resize?.disconnect();
    };
  }, [section, active]);

  const lastSent = useRef(sent);
  useLayoutEffect(() => {
    if (sent === lastSent.current) return;
    lastSent.current = sent;
    const el = scrollerOf(section);
    if (!sent || !el) return;
    following.current = true;
    el.scrollTo?.({ top: el.scrollHeight, behavior: reducedMotion() ? "auto" : "smooth" });
  }, [section, sent]);

  const lastTail = useRef(tail);
  useLayoutEffect(() => {
    if (tail === lastTail.current) return;
    lastTail.current = tail;
    const el = scrollerOf(section);
    if (following.current && el && el.clientHeight > 0) el.scrollTop = el.scrollHeight;
  }, [section, tail]);
}
