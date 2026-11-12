import { useSyncExternalStore } from "react";
import type { McpInput, McpProbeView, McpToolView, McpView } from "../bindings";
import { api } from "../lib/api";
import { formatNotice } from "../lib/notice";

/** 按网址猜一个名字：`https://mcp.linear.app/mcp` → `linear`。 */
export function guessHostName(url: string): string {
  let host = "";
  try { host = new URL(url.trim()).hostname; } catch { return ""; }
  if (/^[\d.]+$/.test(host) || host.includes(":")) return host;
  const parts = host.split(".").filter((part) => !["mcp", "www", "api"].includes(part));
  return parts.length > 1 ? parts[parts.length - 2] : parts[0] ?? "";
}

/** 已保存的服务按原样转成写入 / 检测用的输入：密钥值留空表示沿用已保存的。 */
export function inputFromView(server: McpView, patch: Partial<McpInput> = {}): McpInput {
  return {
    id: server.id, name: server.name, transport: server.transport, command: server.command, args: server.args, url: server.url,
    env: server.env.map((e) => ({ name: e.name, value: "" })), headers: server.headers.map((h) => ({ name: h.name, value: "" })),
    auth: server.auth, include_tools: server.include_tools, ...patch,
  };
}

/** 只在草稿内比较，含密钥，不能记录或持久化。名字与工具开放范围不影响连接。 */
export function connectionKey(input: McpInput): string {
  return JSON.stringify([input.id, input.transport, input.command, input.args, input.url, input.env, input.headers, input.auth]);
}

/** 开放范围：`include_tools` 为空表示全部开放。 */
export function openTools(tools: McpToolView[], include: string[]): Set<string> {
  return new Set(include.length === 0 ? tools.map((t) => t.name) : tools.filter((t) => include.includes(t.name)).map((t) => t.name));
}

/** 勾选结果写回 `include_tools`：全部勾选存为空。 */
export function includeFrom(tools: McpToolView[], chosen: Set<string>): string[] {
  const names = tools.filter((t) => chosen.has(t.name)).map((t) => t.name);
  return names.length === tools.length ? [] : names;
}

/** 宿主的原因句末尾可能带句号；状态行里接在「·」后面时去掉，详情里补上。 */
export const clause = (text: string) => text.trim().replace(/[。.]$/, "");
export const sentence = (text: string) => { const t = text.trim(); return !t || /[。.！？!?]$/.test(t) ? t : `${t}。`; };

function failure(error: unknown): McpProbeView {
  return { ok: false, elapsed_ms: 0, tools: [], error: formatNotice("", error) || "检测没有完成", hint: "", output: "" };
}

/** 一份按服务 id 记的外部状态；检测与登录的结果在本次运行内保留，离开设置页再回来仍在。 */
function keyed<T>() {
  let map: ReadonlyMap<number, T> = new Map();
  const listeners = new Set<() => void>();
  return {
    get: () => map,
    subscribe(listener: () => void) { listeners.add(listener); return () => { listeners.delete(listener); }; },
    set(id: number, value: T | undefined) {
      const next = new Map(map);
      if (value === undefined) next.delete(id); else next.set(id, value);
      map = next;
      for (const listener of [...listeners]) listener();
    },
  };
}

export type ProbeEntry = { busy: boolean; result: McpProbeView | null };
export type LoginEntry = { state: "waiting" } | { state: "done"; ok: boolean; message: string };
type Bridge = Pick<typeof api, "probeMcp" | "loginMcp" | "cancelMcpLogin">;

/** 生产持有一份；请求身份由其发布的快照对象表示，作废时不另留一张票据表。 */
export function createMcpRuntime(bridge: Bridge) {
  const probes = keyed<ProbeEntry>();
  const logins = keyed<LoginEntry>();
  const loginWork = new Map<number, Promise<void>>();
  const trackLogin = (id: number, work: Promise<void>) => {
    const tracked = work.finally(() => { if (loginWork.get(id) === tracked) loginWork.delete(id); });
    loginWork.set(id, tracked);
  };
  const forgetProbe = (id: number) => probes.set(id, undefined);

  return {
    probes: { get: probes.get, subscribe: probes.subscribe },
    logins: { get: logins.get, subscribe: logins.subscribe },
    forgetProbe,
    forgetServer(id: number) { forgetProbe(id); logins.set(id, undefined); },
    /** 已保存服务的检测仅接受当前请求；重测期间可显示上次结果。 */
    probeServer(input: McpInput) {
      const waiting: ProbeEntry = { busy: true, result: probes.get().get(input.id)?.result ?? null };
      probes.set(input.id, waiting);
      const settle = (result: McpProbeView) => {
        if (probes.get().get(input.id) === waiting) probes.set(input.id, { busy: false, result });
      };
      bridge.probeMcp(input).then(settle, (error) => settle(failure(error)));
    },
    probeDraft(input: McpInput): Promise<McpProbeView> { return bridge.probeMcp(input).catch(failure); },
    startLogin(id: number) {
      if (logins.get().get(id)?.state === "waiting") return;
      const waiting: LoginEntry = { state: "waiting" };
      logins.set(id, waiting);
      const settle = (ok: boolean, message: string) => {
        if (logins.get().get(id) === waiting) logins.set(id, { state: "done", ok, message });
      };
      const work = (loginWork.get(id) ?? Promise.resolve()).then(async () => {
        // 旧登录和取消命令都已收尾；等待中再次取消 / 删除就不再启动。
        if (logins.get().get(id) !== waiting) return;
        try { const result = await bridge.loginMcp(id); settle(result.ok, result.message); }
        catch (error) { settle(false, formatNotice("", error)); }
      });
      trackLogin(id, work);
    },
    cancelLogin(id: number) {
      logins.set(id, undefined);
      const cancelled = bridge.cancelMcpLogin(id).catch(() => {});
      trackLogin(id, Promise.all([loginWork.get(id), cancelled]).then(() => {}));
    },
    /** 处理完成后按快照身份取走；旧写入收尾不能取走新登录。 */
    takeLogin(id: number, expected?: LoginEntry) {
      const entry = logins.get().get(id);
      if (entry?.state !== "done" || (expected && entry !== expected)) return undefined;
      logins.set(id, undefined);
      return entry;
    },
  };
}

const runtime = createMcpRuntime(api);
export const { probeServer, probeDraft, forgetProbe, forgetServer, startLogin, cancelLogin, takeLogin } = runtime;
export function useProbes(): ReadonlyMap<number, ProbeEntry> {
  return useSyncExternalStore(runtime.probes.subscribe, runtime.probes.get, runtime.probes.get);
}
export function useLogins(): ReadonlyMap<number, LoginEntry> {
  return useSyncExternalStore(runtime.logins.subscribe, runtime.logins.get, runtime.logins.get);
}
