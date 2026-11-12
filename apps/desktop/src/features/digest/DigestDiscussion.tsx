import { useEffect, useRef } from "react";
import { Icon } from "../../components/Icon";
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
  /** 递增时把追问区滚到视口顶部（发送后）。 */
  scrollRequest: number;
  /** 向前加载更早的追问记录；滚动补偿作用在简报页自己的滚动容器上。 */
  onLoadOlder?: (threadId: number) => Promise<boolean>;
  onOpenUrl: (href: string) => void;
  onOpenArticle: (id: number, anchor?: { blockId: number }) => void;
  /** 失败提示里的「去设置」。 */
  onFix?: (tab: ChatFixTab) => void;
};

const NO_TURNS = createTurnStore();

/** 当期追问区：只订阅自己线程；没有消息、没有流式正文也不在生成时整段不渲染。 */
export function DigestDiscussion({ store, turns, threadId, context, busy, scrollRequest, onLoadOlder, onOpenUrl, onOpenArticle, onFix }: DigestDiscussionProps) {
  const conversation = useConversation(store, threadId);
  const slot = threadSlot(threadId, context);
  const outgoing = useOutgoing(store, slot);
  const live = useLiveTurn(turns ?? NO_TURNS, slot, threadId);
  const traces = useTraces(turns ?? NO_TURNS);
  const failure = useFailure(store, slot);
  useReleaseOutgoing(store, slot);
  const section = useRef<HTMLElement>(null);
  const lastScrollRequest = useRef(scrollRequest);
  const active = conversation.messages.length > 0 || !!conversation.stream?.text || busy || !!outgoing || !!live || !!failure;

  useEffect(() => {
    if (scrollRequest !== lastScrollRequest.current) {
      lastScrollRequest.current = scrollRequest;
      section.current?.scrollIntoView?.({ block: "start", behavior: "smooth" });
    }
  }, [scrollRequest]);

  if (!active) return null;
  return (
    <section ref={section} className="digest-discussion reader-column" aria-label="当期简报追问">
      <div className="chat-column">
        <h2><Icon name="chat" size={15} />当期追问</h2>
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
