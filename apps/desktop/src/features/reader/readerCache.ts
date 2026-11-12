import type { ReaderView } from "../../lib/api";
import type { ReaderTicket } from "./readerRequests";

export const READER_CACHE_SIZE = 8;

type Entry = { view: ReaderView; used: number; written: number };

/** 只保留数据；使用顺序取自打开票据，不让迟到响应把旧篇提升为最近使用。 */
export function createReaderCache() {
  const entries = new Map<number, Entry>();
  const write = (view: ReaderView, ticket: ReaderTicket) => {
    const old = entries.get(view.id);
    if (old && old.written > ticket.session) return old.view;
    entries.set(view.id, { view, used: Math.max(old?.used ?? 0, ticket.session), written: ticket.session });
    if (entries.size > READER_CACHE_SIZE) {
      const oldest = [...entries].reduce((a, b) => a[1].used < b[1].used ? a : b);
      entries.delete(oldest[0]);
    }
    return view;
  };
  return {
    read(ticket: ReaderTicket) {
      const entry = entries.get(ticket.articleId);
      if (entry) entry.used = ticket.session;
      return entry?.view;
    },
    write,
    /** 只看有没有，不算一次使用（预取判断用）。 */
    has(id: number) {
      return entries.has(id);
    },
    update(id: number, update: (view: ReaderView) => ReaderView) {
      const entry = entries.get(id);
      if (entry) entry.view = update(entry.view);
    },
    reconcile(next: ReaderView, ticket: ReaderTicket, initial?: ReaderView) {
      const entry = entries.get(next.id);
      if (entry && entry.written > ticket.session) return entry.view;
      const current = entry?.view;
      if (initial && current) {
        // 等待核对时已抓取新正文 / 接到新版本，旧的打开快照不能撤回它。
        if (current.source_hash !== initial.source_hash) next = current;
        else if (next.source_hash === current.source_hash) {
          // 同版本只保留请求发出后修改的字段，其余接受宿主快照。
          next = { ...next, ...Object.fromEntries(
            (Object.keys(current) as Array<keyof ReaderView>)
              .filter(key => current[key] !== initial[key]).map(key => [key, current[key]]),
          ) };
        }
      }
      return write(next, ticket);
    },
  };
}
