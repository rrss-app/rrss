import { useEffect, useRef, useState } from "react";
import type { CitationView, ThreadView } from "../../lib/api";
import { FloatingMenu, type MenuAnchor } from "../../components/FloatingMenu";
import { Icon } from "../../components/Icon";
import { CitationChip } from "../../components/CitationChip";
import { useDraftSubmission, type DraftStore } from "../chat";

/** 打开阅读器时的提问来源；换篇保留托盘及其目标。 */
export type ReaderCitationSource = { kind: "new" } | { kind: "digest"; digestId: number } | { kind: "thread"; threadId: number; title: string };
export const citationTargetLabel = (target: ReaderCitationSource) => target.kind === "new" ? "新对话" : target.kind === "digest" ? "本期追问" : target.title;
export const citationTargetContext = (drafts: DraftStore, target: ReaderCitationSource) => target.kind === "digest" ? `digest:${target.digestId}` : drafts.threadContext(target.kind === "thread" ? target.threadId : 0);

export function ReaderCitationTray({ drafts, context, focusRequest, target, source, busy, sending, latestCitation, getThreads, onTarget, onSend, onClear, inert }: {
  drafts: DraftStore; context: string; target: ReaderCitationSource; source: ReaderCitationSource;
  focusRequest?: number;
  busy: boolean; sending: boolean; latestCitation: CitationView | null; inert: boolean;
  getThreads: () => ThreadView[];
  onTarget: (target: ReaderCitationSource) => void;
  onSend: (text: string, citations: CitationView[]) => Promise<boolean>;
  onClear: () => void;
}) {
  const { draft, canSend, send, onKeyDown, pending } = useDraftSubmission({ drafts, context, ready: true, busy, onSend });
  const [menu, setMenu] = useState<MenuAnchor | null>(null);
  const [threads, setThreads] = useState<ThreadView[]>([]);
  const root = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLTextAreaElement>(null);
  useEffect(() => { if (focusRequest) input.current?.focus({ preventScroll: true }); }, [focusRequest]);
  const trigger = useRef<HTMLButtonElement>(null);
  const latest = useRef<HTMLSpanElement>(null);
  const locked = sending || pending;
  useEffect(() => { if (inert) setMenu(null); }, [inert]);
  useEffect(() => { latest.current?.scrollIntoView?.({ block: "nearest", inline: "nearest" }); }, [latestCitation]);
  // 跟随实际托盘高度避让；窄屏换行与多行输入也不会遮住正文末尾、回顶按钮。
  useEffect(() => {
    const node = root.current, reader = node?.closest<HTMLElement>(".drawer.reader");
    if (!node || !reader) return;
    const measure = () => reader.style.setProperty("--citation-tray-height", `${node.offsetHeight}px`);
    measure();
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measure);
    observer?.observe(node);
    return () => { observer?.disconnect(); reader.style.removeProperty("--citation-tray-height"); };
  }, []);
  const targets: ReaderCitationSource[] = [
    ...(source.kind !== "new" ? [source] : []),
    { kind: "new" },
    ...threads.slice(0, 6).filter(t => source.kind !== "thread" || t.id !== source.threadId)
      .map(t => ({ kind: "thread" as const, threadId: t.id, title: t.title })),
  ];
  // 当前目标即使已不在最近六条内，也保持可见。
  if (!targets.some(t => citationTargetContext(drafts, t) === context)) targets.unshift(target);
  const contexts = new Set<string>();
  return <div ref={root} className="reader-citation-tray" role="region" aria-label="引用托盘" aria-busy={locked}>
    {draft.citations.length > 0 ? <div className="reader-citation-row">
      <span className="reader-citation-count"><Icon name="quote" size={13} />引用 {draft.citations.length}</span>
      <div className="reader-citation-chips">
        {draft.citations.map((citation, index) => <CitationChip key={`${citation.article_id}-${index}`} ref={citation === latestCitation ? latest : undefined}
          citation={citation} className="reader-citation-chip" isNew={citation === latestCitation}
          removeLabel={`移除引用 ${index + 1}`} removeDisabled={locked}
          onRemove={() => drafts.setCitations(context, cs => cs.filter((_, i) => i !== index))} />)}
      </div>
      <button type="button" className="reader-citation-clear" disabled={locked} onClick={onClear}>清空</button>
    </div> : null}
    <div className="reader-citation-input">
      <textarea ref={input} aria-label="针对引用提问" rows={1} placeholder={source.kind === "thread" ? `继续问「${source.title}」…` : "针对这些引用提问…"}
        value={draft.text} disabled={locked} onChange={event => drafts.setText(context, event.target.value)} onKeyDown={onKeyDown} />
      <button ref={trigger} type="button" className="reader-citation-target" aria-label={`发到 ${citationTargetLabel(target)}`} aria-haspopup="menu" aria-expanded={!!menu} disabled={locked}
        onClick={() => {
          if (menu) { setMenu(null); return; }
          setThreads(getThreads());
          const selection = trigger.current!.getBoundingClientRect();
          setMenu({ x: selection.x, y: selection.y, selection });
        }}>发到 <b>{citationTargetLabel(target)}</b><Icon name="chevronDown" size={12} /></button>
      <button type="button" className="reader-citation-send" aria-label={locked ? "发送中…" : "发送"} disabled={!canSend} onClick={() => void send()}><Icon name="send" size={16} /></button>
    </div>
    {busy && !locked ? <div className="reader-citation-status" role="status">正在生成回复，请等待完成后发送。</div> : null}
    {menu && !inert ? <FloatingMenu anchor={menu} boundary={root.current?.closest<HTMLElement>(".drawer.reader")} label="发到" className="reader-context-menu reader-citation-target-menu" restoreFocusTo={trigger.current} onClose={() => setMenu(null)}>
      <div className="reader-context-caption">发到</div>
      {targets.map(t => {
        const key = citationTargetContext(drafts, t);
        if (contexts.has(key)) return null;
        contexts.add(key);
        return <button key={key} type="button" role="menuitemradio" aria-checked={key === context} onClick={() => { setMenu(null); onTarget(t); }}>
          <Icon name={t.kind === "new" ? "plus" : t.kind === "digest" ? "digest" : "chat"} size={14} /><span>{citationTargetLabel(t)}</span>{key === context ? <Icon name="check" size={13} /> : null}
        </button>;
      })}
    </FloatingMenu> : null}
  </div>;
}
