import { useSyncExternalStore } from "react";
import type { ChatStepView } from "../../lib/api";
import { i18n } from "../../i18n";

/** 等待过程里的一步：启动对话引擎、一次工具调用，或两者之间的「思考」。 */
export type TraceStep = {
  id: string;
  kind: "engine" | "tool" | "think";
  label: string;
  detail: string;
  articleId: number | null;
  /** 外部工具（MCP）的服务显示名与服务里的工具名；其他步骤为空。 */
  server: string;
  serverTool: string;
  /** 经过确认的外部工具调用：允许或拒绝。 */
  approval: "allowed" | "denied" | null;
  startedAt: number;
  /** 还在进行为 null。 */
  endedAt: number | null;
  error: boolean;
};

/** 正在进行的一轮：发送时按会话位置登记，宿主报出线程 id 后两者都能对上。 */
export type LiveTurn = {
  slot: string;
  threadId: number;
  startedAt: number;
  /** 引擎与工具步骤（按开始先后）；「思考」由间隙推出，见 `traceTimeline`。 */
  steps: TraceStep[];
  /** 开始出字的时刻；之前回复位置显示等待过程，之后收成一行摘要。 */
  answeringAt: number | null;
  /**
   * 开始出字后又去调用工具的几段：从这次调用开始，到回复接着出字（或这一轮结束）。
   * 这几段和出字前一样计时，进行中时回复下方再显示等待过程。
   */
  pauses: { from: number; to: number | null }[];
  /** 最近一次变化的时刻：出字后的摘要算到这里。 */
  at: number;
  /** 正在等用户确认的外部工具；等待过程停下脉冲，写「等你确认」。 */
  asking: { server: string; tool: string } | null;
  /** 确认先于步骤开始到达时，暂存到对应步骤出现。 */
  approvals: { server: string; tool: string; approval: "allowed" | "denied" }[];
};

/** 收起后的摘要，只在本次运行内保留（不存库）。`tools` 是本地工具次数，`external` 是外部工具次数。 */
export type Trace = { steps: TraceStep[]; elapsed: number; tools: number; external: number };

type Listener = () => void;

/** 两步之间不足这么久的空隙不算一次「思考」。 */
const THINK_MIN_MS = 300;

/**
 * 一轮对话的等待过程。同一时间只有一轮（发送与回复互斥），所以只持有一份进行中的；
 * 回复落库后摘要按回复消息 id 留到本次运行结束。
 */
export function createTurnStore(now: () => number = Date.now) {
  let live: LiveTurn | null = null;
  let traces: ReadonlyMap<number, Trace> = new Map();
  const listeners = new Set<Listener>();
  const notify = () => { for (const listener of [...listeners]) listener(); };
  const mine = (threadId: number) => !!live && (live.threadId === threadId || (live.threadId === 0 && threadId > 0));
  /** 已收到的回复字数：变了才算接着出字（一条消息结束时会把同样的文本再发一次）。不进快照，免得每段字都通知。 */
  let textLength = 0;
  const update = (patch: Partial<LiveTurn>) => {
    if (!live) return;
    live = { ...live, at: now(), ...patch };
    notify();
  };

  return {
    getLive: () => live,
    getTraces: () => traces,
    subscribe(listener: Listener) {
      listeners.add(listener);
      return () => { listeners.delete(listener); };
    },
    /** 发出一条：等待过程从这里开始计时。 */
    begin(slot: string, threadId: number) {
      const at = now();
      live = { slot, threadId: Math.max(0, threadId), startedAt: at, steps: [], answeringAt: null, pauses: [], at, asking: null, approvals: [] };
      textLength = 0;
      notify();
    },
    /** 宿主报出线程（chat:start 或发送被接受）。 */
    attach(threadId: number) {
      if (live && live.threadId === 0 && threadId > 0) update({ threadId });
    },
    /** 发送失败：这一轮作废。 */
    abandon() {
      if (!live) return;
      live = null;
      notify();
    },
    step(event: ChatStepView, threadId: number) {
      // 冷启动的引擎步骤可能早于线程建立（线程 id 为 0）；只有一轮在进行，直接记到它上面。
      if (!live || (threadId > 0 && !mine(threadId))) return;
      if (threadId > 0 && live.threadId === 0) live = { ...live, threadId };
      const at = now();
      const kind = event.kind === "engine" ? "engine" : "tool";
      const server = event.server ?? "";
      const serverTool = event.server_tool ?? "";
      if (event.start) {
        const early = server ? live.approvals.findIndex((a) => a.server === server && a.tool === serverTool) : -1;
        // 已经出字后又调用工具：从这里开始一段等待，接着出字时结束。
        const opens = live.answeringAt !== null && live.pauses.at(-1)?.to !== null;
        update({
          at,
          pauses: opens ? [...live.pauses, { from: at, to: null }] : live.pauses,
          steps: [...live.steps, {
            id: event.id || `${kind}-${live.steps.length}`, kind, label: event.label, detail: event.detail,
            articleId: event.article_id, server, serverTool, approval: early >= 0 ? live.approvals[early].approval : null,
            startedAt: at, endedAt: null, error: false,
          }],
          approvals: early >= 0 ? live.approvals.filter((_, i) => i !== early) : live.approvals,
          asking: live.asking && early < 0 && live.asking.server === server && live.asking.tool === serverTool ? live.asking : null,
        });
        return;
      }
      // 结束按 id 对上；没有 id 的（个别引擎版本）结束最近一个同名、还没结束的。
      let index = -1;
      live.steps.forEach((step, i) => {
        if (step.endedAt === null && (event.id ? step.id === event.id : step.label === event.label)) index = i;
      });
      if (index < 0) return;
      const asked = live.asking && live.asking.server === server && live.asking.tool === serverTool;
      update({ at, steps: live.steps.map((step, i) => (i === index ? { ...step, endedAt: at, error: event.error } : step)), asking: asked ? null : live.asking });
    },
    /** 宿主请求确认一个外部工具调用。 */
    ask(threadId: number, server: string, tool: string) {
      if (live && mine(threadId)) update({ asking: { server, tool } });
    },
    /** 用户回应了确认：记到这次调用上（步骤还没出现就先暂存）。 */
    answer(threadId: number, server: string, tool: string, ok: boolean) {
      if (!live || !mine(threadId)) return;
      const approval = ok ? "allowed" as const : "denied" as const;
      let index = -1;
      live.steps.forEach((step, i) => { if (step.server === server && step.serverTool === tool && step.approval === null) index = i; });
      // 拒绝后这次调用不会执行，引擎不一定再报结束：这一步就此结束，等待过程回到「思考」。
      const at = now();
      update(index >= 0
        ? { steps: live.steps.map((step, i) => (i === index ? { ...step, approval, endedAt: ok ? step.endedAt : step.endedAt ?? at } : step)), asking: null }
        : { approvals: [...live.approvals, { server, tool, approval }], asking: null });
    },
    /** 收到正文（`length` 是目前的总字数）：第一段开始出字；调用工具后再出字结束那段等待。 */
    answering(threadId: number, length: number) {
      if (!live || !mine(threadId) || length === textLength) return;
      textLength = length;
      const last = live.pauses.at(-1);
      if (live.answeringAt !== null && last?.to !== null) return;
      const at = now();
      update({
        at,
        threadId: live.threadId || threadId,
        answeringAt: live.answeringAt ?? at,
        pauses: last ? [...live.pauses.slice(0, -1), { ...last, to: at }] : live.pauses,
      });
    },
    /**
     * 一轮结束：摘要挂到这一轮的回复上。没有回复（出错、中止）时不留在这里，
     * 返回截到现在的摘要，由调用方决定是否挂到失败提示上。
     */
    finish(threadId: number, replyId: number | null): Trace | null {
      if (!live || !mine(threadId)) return null;
      const turn = live;
      live = null;
      const end = now();
      if (replyId !== null && turn.answeringAt !== null) {
        const next = new Map(traces);
        next.set(replyId, summarize(turn, end));
        traces = next;
      }
      notify();
      return replyId === null ? summarize(turn, end) : null;
    },
  };
}
export type TurnStore = ReturnType<typeof createTurnStore>;

type Timed = Pick<LiveTurn, "steps" | "startedAt" | "answeringAt" | "pauses">;

/** 等待的几段：出字前，以及出字后每次调用工具到接着出字。都截到 `until`。 */
function waits(turn: Timed, until: number) {
  const all = [{ from: turn.startedAt, to: turn.answeringAt ?? until }, ...turn.pauses.map((pause) => ({ from: pause.from, to: pause.to ?? until }))];
  return all.map((wait) => ({ from: Math.min(wait.from, until), to: Math.min(wait.to, until) }));
}

/**
 * 每段等待里，引擎与工具步骤之间的空隙补成「思考」。出字的时间不算：
 * 出字后的步骤记到它所在的那段，按自己的起止计时，不截到开始出字。
 */
export function traceTimeline(turn: Timed, until: number): TraceStep[] {
  const out: TraceStep[] = [];
  const think = (from: number, to: number) => {
    if (to - from >= THINK_MIN_MS) {
      out.push({ id: `think-${from}`, kind: "think", label: i18n.t("progress.think", { ns: "chat" }), detail: "", articleId: null, server: "", serverTool: "", approval: null, startedAt: from, endedAt: to, error: false });
    }
  };
  const segments = waits(turn, until);
  // 步骤按开始先后排列；每步归到开始时刻之前最近的一段。
  let next = 0;
  segments.forEach((segment, i) => {
    const later = segments[i + 1]?.from ?? Infinity;
    let cursor = segment.from;
    for (; next < turn.steps.length && (turn.steps[next].startedAt < later || i === segments.length - 1); next++) {
      const step = turn.steps[next];
      if (step.kind !== "engine") think(cursor, step.startedAt);
      const end = Math.min(step.endedAt ?? until, until);
      out.push({ ...step, endedAt: Math.max(step.startedAt, end) });
      cursor = Math.max(cursor, end);
    }
    think(cursor, segment.to);
  });
  return out;
}

export function summarize(turn: Timed, until: number): Trace {
  const steps = traceTimeline(turn, until);
  const tools = turn.steps.filter((step) => step.kind === "tool");
  const elapsed = waits(turn, until).reduce((sum, wait) => sum + Math.max(0, wait.to - wait.from), 0);
  return { steps, elapsed, tools: tools.filter((step) => !step.server).length, external: tools.filter((step) => step.server).length };
}

/** 出字后正在等工具（或工具结束后还没接着出字）：返回这段等待的开始，否则为 null。 */
export function pausedSince(turn: LiveTurn): number | null {
  const last = turn.pauses.at(-1);
  return turn.answeringAt !== null && last && last.to === null ? last.from : null;
}

/** 这一处会话的进行中一轮：发送时的位置对得上，或宿主已报出的线程对得上。 */
export function useLiveTurn(store: TurnStore, slot: string, threadId: number): LiveTurn | null {
  const live = useSyncExternalStore(store.subscribe, store.getLive, store.getLive);
  if (!live) return null;
  return live.slot === slot || (threadId > 0 && live.threadId === threadId) ? live : null;
}

export function useTraces(store: TurnStore): ReadonlyMap<number, Trace> {
  return useSyncExternalStore(store.subscribe, store.getTraces, store.getTraces);
}
