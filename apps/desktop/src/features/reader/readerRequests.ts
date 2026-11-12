/**
 * 阅读器请求归属：每次打开 / 关闭阅读器推进一代「阅读会话」，每次阅读工具操作推进一代「操作」。
 * 异步结果回来时用文章 ID + 会话代际（+ 操作代际）校验；迟到结果既不能改导航，也不能改当前文章，
 * busy 与 finally 的清理只属于发起它的那次操作。
 */

export type ReaderTicket = { readonly articleId: number; readonly session: number };

export type ReaderOpKind = "summary" | "translate" | "fulltext" | "media" | "collect";

export type ReaderOpTicket = ReaderTicket & { readonly kind: ReaderOpKind; readonly op: number };

export type ReaderRequestTracker = {
  /** 开始一次新的阅读会话；之前会话的所有在途结果随即失效。 */
  open: (articleId: number) => ReaderTicket;
  /** 关闭阅读器；在途的打开 / 工具结果全部失效。 */
  close: () => void;
  current: () => ReaderTicket | null;
  /** 该票据仍是当前会话且文章未变。 */
  owns: (ticket: ReaderTicket) => boolean;
  /** 为正在显示的会话开始一次工具操作；该会话已被换文 / 关闭取代时返回 null，调用方应忽略本次操作。 */
  beginOp: (kind: ReaderOpKind, ticket: ReaderTicket) => ReaderOpTicket | null;
  /** 会话仍归属且同类操作没有被更新一次的操作取代。 */
  ownsOp: (ticket: ReaderOpTicket) => boolean;
};

export function createReaderRequestTracker(): ReaderRequestTracker {
  let session = 0;
  let articleId: number | null = null;
  const ops: Record<ReaderOpKind, number> = { summary: 0, translate: 0, fulltext: 0, media: 0, collect: 0 };

  const owns = (ticket: ReaderTicket) => ticket.session === session && ticket.articleId === articleId;

  return {
    open(id) {
      session += 1;
      articleId = id;
      return { articleId: id, session };
    },
    close() {
      session += 1;
      articleId = null;
    },
    current() {
      return articleId === null ? null : { articleId, session };
    },
    owns,
    beginOp(kind, ticket) {
      if (!owns(ticket)) return null;
      ops[kind] += 1;
      return { ...ticket, kind, op: ops[kind] };
    },
    ownsOp(ticket) {
      return owns(ticket) && ops[ticket.kind] === ticket.op;
    },
  };
}
