import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import type { CitationView } from "../../lib/api";
import { CitationChip } from "../../components/CitationChip";
import { Icon } from "../../components/Icon";
import { ReaderTooltip } from "../../components/ReaderTool";
import type { DraftStore } from "./draftStore";
import { useDraftSubmission } from "./useDraftSubmission";

export type ComposerProps = {
  drafts: DraftStore;
  /** 草稿上下文：每条会话一份，简报与其追问会话共用一份。 */
  context: string;
  ariaLabel: string;
  placeholder: string;
  /** 展开时显示在输入框上方的上下文说明（简报期号、加载状态）。 */
  contextLabel?: string;
  /** 另一会话正在回复：显示提示且不可发送。 */
  otherThreadBusy: boolean;
  /** 任意会话正在生成：显示停止按钮并保持展开。 */
  generating: boolean;
  sending: boolean;
  /** 输入框禁用（发送中、简报未就绪）。 */
  disabled: boolean;
  /** 除草稿非空以外的发送前提。 */
  ready: boolean;
  /** 空闲时收成底部小台子，浮在正文上（简报视图）；悬停、滚到底、聚焦或有草稿时展开。 */
  collapsible: boolean;
  /** 台子上的文字（「追问本期 · 1 条」）。 */
  dockLabel?: ReactNode;
  /** 正文已滚到底：台子展开成输入卡。 */
  atBottom?: boolean;
  /** 即使空也保持展开（生成中、还没有当期简报）。 */
  forceExpanded: boolean;
  /** 草稿为空时是否仍显示（焦点视图只在有引用时显示）。 */
  showWhenEmpty: boolean;
  /** 返回 true 表示已被接受，清空该上下文草稿；失败保留草稿和引用。 */
  onSend: (text: string, citations: CitationView[]) => Promise<boolean>;
  onStop: () => void;
  onOpenArticle: (id: number, anchor?: { blockId: number }) => void;
  /** 输入卡上方的过程条（进度、提示、确认），收起时也显示。 */
  activity?: ReactNode;
  /** 模型与思考按钮：对话里常驻在输入卡下方的模型行；简报台子里在输入行内，展开时显示。 */
  model?: ReactNode;
  /** 上下文用量圆环：对话里在模型行，简报台子里在发送键左侧、收起时也显示。 */
  meter?: ReactNode;
  /** 挂在输入区顶边上方的「回到最新」（对话里）；随输入区长高一起上移。 */
  jump?: ReactNode;
};

/** 输入与引用草稿由这里持有并订阅；连续输入只重渲染本组件。 */
export function Composer(props: ComposerProps) {
  const { drafts, context, showWhenEmpty, collapsible, forceExpanded, ready, sending, generating, otherThreadBusy, disabled } = props;
  // 按下就清空输入、自己的消息立刻出现；失败时文字与引用放回来。发送期间输入框仍可用。
  const { draft, canSend, send, onKeyDown, pending } = useDraftSubmission({ drafts, context, ready, busy: sending || generating, onSend: props.onSend, clearOnSend: true });
  const [focused, setFocused] = useState(false);
  // 发送键与停止键互换时，新图标缩放旋入一次；首次出现不播。
  const [shownGenerating, setShownGenerating] = useState(generating);
  const [swapped, setSwapped] = useState(false);
  if (shownGenerating !== generating) {
    setShownGenerating(generating);
    setSwapped(true);
  }
  const cardRef = useRef<HTMLDivElement>(null);
  const hasContent = !!draft.text || draft.citations.length > 0;
  const dock = useDock(collapsible);

  // 焦点页只在带引用时出现；按下发送后引用已清空，发送完成（切到对话）之前仍留着。
  if (!showWhenEmpty && draft.citations.length === 0 && !pending) return null;

  const expanded = !collapsible || focused || hasContent || forceExpanded || dock.hovered || !!props.atBottom;

  return (
    <div className="composer" ref={dock.root} data-dock={collapsible ? (expanded ? "open" : "closed") : undefined}
      data-arming={collapsible && !expanded && dock.arming ? "" : undefined}>
      {props.jump ? <div className="composer-jump"><div>{props.jump}</div></div> : null}
      {props.activity}
      <div
        className="composer-card"
        ref={cardRef}
        onFocus={() => setFocused(true)}
        onBlur={(e) => {
          if (!(e.relatedTarget instanceof Node) || !cardRef.current?.contains(e.relatedTarget)) setFocused(false);
        }}
      >
        <div className="composer-content">
          {(expanded || collapsible) && props.contextLabel ? (
            <div className="composer-context">
              <Icon name="digest" size={13} />
              {props.contextLabel}
            </div>
          ) : null}
          {expanded && otherThreadBusy ? (
            <div className="composer-context" role="status"><Icon name="info" size={13} />另一会话正在回复，请等待完成或停止生成。</div>
          ) : null}
          {draft.citations.length > 0 && (
            <div className="cite-row">
              {draft.citations.map((c, i) => (
                <CitationChip
                  key={`${c.article_id}-${i}`}
                  citation={c}
                  onOpen={props.onOpenArticle}
                  onRemove={() => drafts.setCitations(context, (cs) => cs.filter((_, idx) => idx !== i))}
                />
              ))}
            </div>
          )}
          <div className="input-row" ref={dock.row} {...dock.handlers}
            onClick={collapsible && !expanded ? (e) => { if (e.target === e.currentTarget) e.currentTarget.querySelector("textarea")?.focus(); } : undefined}>
            {collapsible ? <span className="composer-dock-label" ref={dock.label} aria-hidden="true">{props.dockLabel}</span> : null}
            <textarea
              value={draft.text}
              onChange={(e) => drafts.setText(context, e.target.value)}
              aria-label={props.ariaLabel}
              disabled={disabled}
              placeholder={props.placeholder}
              rows={collapsible && expanded ? 2 : 1}
              onKeyDown={onKeyDown}
            />
            {collapsible ? <>
              <span className="composer-grow" inert={!expanded}>{props.model}</span>
              {props.model && props.meter ? <span className="composer-sep composer-grow" aria-hidden="true" /> : null}
              {props.meter}
              <span className="composer-dock-key" aria-hidden="true"><kbd>/</kbd></span>
            </> : null}
            <span className={collapsible ? "composer-grow" : "composer-send-slot"} inert={collapsible && !expanded}>{generating ? (
              <ReaderTooltip label="停止生成" placement="top">
                <button type="button" className="composer-send" aria-label="停止生成" data-swap={swapped ? "" : undefined} onClick={props.onStop}><Icon name="stop" size={16} /></button>
              </ReaderTooltip>
            ) : (
              <ReaderTooltip label="Enter 发送" shortcut="Shift+Enter 换行" placement="top">
                {/* 对话里草稿为空时是幽灵键，有可发送内容才变实底。 */}
                <button type="button" className="composer-send" aria-label="发送" aria-busy={sending || pending} disabled={!expanded || !canSend} data-swap={swapped ? "" : undefined}
                  data-ghost={!collapsible && !canSend ? "" : undefined} onClick={() => void send()}>
                  <Icon name={collapsible ? "send" : "enter"} size={16} />
                </button>
              </ReaderTooltip>
            )}</span>
          </div>
          {/* 模型行常驻输入卡正下方：不随聚焦、草稿、生成出现或消失，输入卡长高时它不动。 */}
          {!collapsible && (props.model || props.meter) ? <div className="composer-model-row">{props.model}{props.meter}</div> : null}
        </div>
      </div>
    </div>
  );
}

/** 指针停多久才展开、离开多久才收回：擦过不展开，移到菜单的空隙里不收回。 */
const ARM_MS = 120;
const LEAVE_MS = 300;

/**
 * 小台子的悬停与尺寸：停 120ms 展开（期间先浮起预告），离开 300ms 收回；台子宽度按标签实测，宽度过渡在两个确定值之间。
 * 展开时的高度写到父元素的 `--composer-h`，正文底部按它留白，滚到底时最后一行不被卡片压住。
 */
function useDock(enabled: boolean) {
  const [hovered, setHovered] = useState(false);
  const [arming, setArming] = useState(false);
  const timer = useRef(0);
  const root = useRef<HTMLDivElement>(null);
  const row = useRef<HTMLDivElement>(null);
  const label = useRef<HTMLSpanElement>(null);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  useLayoutEffect(() => {
    if (!enabled || !label.current || !row.current) return;
    // 标签 + 圆环（30）+ 快捷键（24）+ 内边距与间隙。
    row.current.style.setProperty("--dock-w", `${Math.ceil(label.current.scrollWidth + 72)}px`);
  });
  useEffect(() => {
    const node = root.current;
    if (!enabled || !node || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => {
      if (node.dataset.dock === "open") node.parentElement?.style.setProperty("--composer-h", `${Math.ceil(node.offsetHeight)}px`);
    });
    observer.observe(node);
    return () => observer.disconnect();
  }, [enabled]);
  const handlers = enabled ? {
    onPointerEnter: () => {
      window.clearTimeout(timer.current);
      if (hovered) return;
      setArming(true);
      timer.current = window.setTimeout(() => { setArming(false); setHovered(true); }, ARM_MS);
    },
    onPointerLeave: () => {
      window.clearTimeout(timer.current);
      setArming(false);
      timer.current = window.setTimeout(() => setHovered(false), LEAVE_MS);
    },
  } : {};
  return { hovered, arming, root, row, label, handlers };
}
