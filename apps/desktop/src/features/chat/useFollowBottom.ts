import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";
import { ancestorEntering, easeOut, playEnter, reducedMotion } from "../../lib/motion";

/** 离底部多近算贴底（回复增长时保持在底部）。 */
export const FOLLOW_PX = 48;
/** 离底部多远出现「回到最新」。 */
export const AWAY_PX = 120;
/** 发送后、点「回到最新」时平滑到底的时长。 */
const GLIDE_MS = 360;

export type JumpState = { away: boolean; fresh: boolean };
const IDLE: JumpState = { away: false, fresh: false };

/**
 * 对话区与「回到最新」之间的小 store：对话区写入离底部的状态，按钮订阅并在点按时请对话区滚到底。
 * 两者在 DOM 上不相邻（按钮挂在输入区上方），由根壳持有一份。
 */
export function createJumpController() {
  let state = IDLE;
  let jump: () => void = () => {};
  const listeners = new Set<() => void>();
  return {
    get: () => state,
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => { listeners.delete(listener); };
    },
    set(patch: Partial<JumpState>) {
      const next = { ...state, ...patch };
      if (next.away === state.away && next.fresh === state.fresh) return;
      state = next;
      for (const listener of [...listeners]) listener();
    },
    /** 对话区登记自己的滚到底；返回注销函数。 */
    bind(handler: () => void) {
      jump = handler;
      return () => { if (jump === handler) { jump = () => {}; state = IDLE; for (const listener of [...listeners]) listener(); } };
    },
    jump: () => jump(),
  };
}
export type JumpController = ReturnType<typeof createJumpController>;

export type FollowBottomOptions = {
  scroller: RefObject<HTMLElement | null>;
  /** 消息列；它长高（新消息、流式正文）时贴底跟随。 */
  content: RefObject<HTMLElement | null>;
  /** 当前会话；换了就落到最底部并重新跟随。 */
  threadKey: string;
  /** 末尾的签名：变了表示来了新内容（只看末尾，向前加载更早的消息不算）。 */
  tail: string;
  /** 自己刚发出的一条（待发消息的序号，没有为 0）：出现时平滑滚到底。 */
  sent: number;
  /** 换会话时是否播淡入：从「新对话」接着进入刚建的线程不算换。 */
  fadeOnSwitch?: (previous: string, next: string) => boolean;
  controller?: JumpController;
};

type FollowSettings = Pick<FollowBottomOptions, "controller" | "fadeOnSwitch">;

/** 跟随状态与滚动操作放在一个只建一次的对象里，事件监听与各个 effect 共用。 */
function createFollow(scroller: RefObject<HTMLElement | null>, settings: { current: FollowSettings }) {
  let follow = true;
  let fresh = false;
  let glide: number | null = null;
  let lastTop = 0;

  const visible = () => {
    const el = scroller.current;
    return el && el.clientHeight > 0 ? el : null;
  };
  const distance = (el: HTMLElement) => el.scrollHeight - el.clientHeight - el.scrollTop;
  const publish = (el: HTMLElement) => {
    if (follow) fresh = false;
    settings.current.controller?.set({ away: distance(el) > AWAY_PX, fresh });
  };
  const pin = (el: HTMLElement) => {
    el.scrollTop = el.scrollHeight;
    lastTop = el.scrollTop;
  };
  const stop = () => {
    if (glide !== null) cancelAnimationFrame(glide);
    glide = null;
  };
  const toBottom = (smooth: boolean) => {
    const el = visible();
    stop();
    follow = true;
    fresh = false;
    if (!el) return;
    if (!smooth || reducedMotion() || typeof requestAnimationFrame !== "function") {
      pin(el);
      publish(el);
      return;
    }
    let start = el.scrollTop;
    const first = el.scrollHeight - el.clientHeight;
    if (first - start > el.clientHeight * 1.5) start = el.scrollTop = first - el.clientHeight;
    const begin = performance.now();
    settings.current.controller?.set({ fresh: false });
    const step = (now: number) => {
      // 目标随内容增长实时更新：回复还在长时也停在真正的底部。
      const target = el.scrollHeight - el.clientHeight;
      const t = (now - begin) / GLIDE_MS;
      el.scrollTop = t >= 1 ? target : start + (target - start) * easeOut(t);
      lastTop = el.scrollTop;
      if (t < 1) glide = requestAnimationFrame(step);
      else {
        glide = null;
        publish(el);
      }
    };
    glide = requestAnimationFrame(step);
  };

  return {
    toBottom,
    stop,
    onScroll(event: Event) {
      const el = scroller.current;
      // 自己在滑动、保活恢复补发的合成事件、隐藏中都不算用户滚动。
      if (!el || glide !== null || !event.isTrusted || el.clientHeight === 0) return;
      const up = el.scrollTop < lastTop;
      lastTop = el.scrollTop;
      if (distance(el) <= FOLLOW_PX) follow = true;
      else if (up) follow = false;
      publish(el);
    },
    onResize() {
      const el = visible();
      if (!el) return;
      if (follow && glide === null) pin(el);
      publish(el);
    },
    /** 换会话：落到最底部、重新跟随。 */
    reset() {
      stop();
      follow = true;
      fresh = false;
      const el = visible();
      if (el) {
        pin(el);
        publish(el);
      } else settings.current.controller?.set(IDLE);
    },
    /** 末尾来了新内容：贴底就跟上；不在底部就标成「有新回复」。 */
    grew() {
      const el = visible();
      if (!el) return;
      if (follow) {
        if (glide === null) pin(el);
      } else fresh = true;
      publish(el);
    },
  };
}

/**
 * 对话区贴底跟随（设计系统「对话页打磨」）：打开落在最底部；离底部 48px 内算贴底，内容增长时保持贴底；
 * 用户往上翻就停止跟随，直到回到底部。发送与「回到最新」平滑滚到底（缓出 360ms，距离超过一屏半先跳到只差一屏）。
 * 隐藏（保活）期间不改跟随状态；减少动态效果时直接到底。
 */
export function useFollowBottom({ scroller, content, threadKey, tail, sent, fadeOnSwitch, controller }: FollowBottomOptions) {
  const settings = useRef<FollowSettings>({ controller, fadeOnSwitch });
  settings.current = { controller, fadeOnSwitch };
  const [follow] = useState(() => createFollow(scroller, settings));

  useEffect(() => controller?.bind(() => follow.toBottom(true)), [controller, follow]);

  useEffect(() => {
    const el = scroller.current;
    const body = content.current;
    if (!el) return;
    // 用户一动就交还控制：滚轮、触摸、拖滚动条、按键都会打断平滑滚动。
    const interrupt = () => follow.stop();
    const inputs = ["wheel", "touchstart", "pointerdown", "keydown"] as const;
    el.addEventListener("scroll", follow.onScroll, { passive: true });
    for (const name of inputs) el.addEventListener(name, interrupt, { passive: true });
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(follow.onResize);
    observer?.observe(el);
    if (body) observer?.observe(body);
    return () => {
      follow.stop();
      el.removeEventListener("scroll", follow.onScroll);
      for (const name of inputs) el.removeEventListener(name, interrupt);
      observer?.disconnect();
    };
  }, [scroller, content, follow]);

  // 换会话：新内容淡入（不是从「新对话」接着进入刚建的线程时）。
  const shownThread = useRef<string | null>(null);
  useLayoutEffect(() => {
    const previous = shownThread.current;
    shownThread.current = threadKey;
    follow.reset();
    if (previous === null || previous === threadKey || !(settings.current.fadeOnSwitch?.(previous, threadKey) ?? true)) return;
    // 从别的主栏目点进来时整个栏目已在淡入（父组件的 layout effect 在这之后），不再叠一层。
    const column = content.current;
    queueMicrotask(() => { if (!ancestorEntering(column)) playEnter(column); });
  }, [threadKey, follow, content]);

  const lastTail = useRef(tail);
  useLayoutEffect(() => {
    if (lastTail.current === tail) return;
    lastTail.current = tail;
    follow.grew();
  }, [tail, follow]);

  // 自己刚发出：恢复跟随并平滑滚到底。
  const lastSent = useRef(sent);
  useLayoutEffect(() => {
    if (sent === lastSent.current) return;
    lastSent.current = sent;
    if (sent) follow.toBottom(true);
  }, [sent, follow]);
}
