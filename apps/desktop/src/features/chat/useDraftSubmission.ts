import { useRef, useState, type KeyboardEvent } from "react";
import type { CitationView } from "../../lib/api";
import { useDraft, type DraftStore } from "./draftStore";

/**
 * Composer 与阅读器托盘共用的提交边界：同步互斥、输入法让位、成功后只消费已提交快照。
 * `clearOnSend`（对话输入区）：按下就清空输入，发送失败再把文字和引用放回去；发送期间仍可接着输入。
 */
export function useDraftSubmission({ drafts, context, ready, busy, onSend, clearOnSend = false }: {
  drafts: DraftStore; context: string; ready: boolean; busy: boolean;
  onSend: (text: string, citations: CitationView[]) => Promise<boolean>;
  clearOnSend?: boolean;
}) {
  const draft = useDraft(drafts, context);
  const inflight = useRef(false);
  const [pending, setPending] = useState(false);
  const canSend = ready && !busy && !pending && (!!draft.text.trim() || draft.citations.length > 0);
  const send = async () => {
    if (!canSend || inflight.current) return;
    inflight.current = true;
    setPending(true);
    const snapshot = drafts.get(context);
    if (clearOnSend) drafts.consume(context, snapshot);
    let ok = false;
    try {
      ok = await onSend(snapshot.text.trim(), snapshot.citations);
      if (!clearOnSend && ok) drafts.consume(context, snapshot);
    } finally {
      if (clearOnSend && !ok) drafts.restore(context, snapshot);
      inflight.current = false;
      setPending(false);
    }
  };
  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing && event.keyCode !== 229) {
      event.preventDefault();
      void send();
    }
  };
  return { draft, canSend, send, onKeyDown, pending };
}
