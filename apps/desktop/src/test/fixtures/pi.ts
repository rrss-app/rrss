import type { AiPage, NetworkProxyView, PiConfigView, PiPage, PiProbe, SettingsSchema } from "../../bindings";
import { createSettingsSchema } from "../../settings/registry";
import { createApiStub, type ApiStub } from "../api";
import { aiPageFixture, networkProxyFixture, piConfigFixture, piPageFixture, piProbeFixture } from "./settings";

function clone<T>(value: T): T {
  return structuredClone(value);
}

// 默认快照来自共享工厂；这里只负责把它们接成可读写的 API 桩。
export const FIXTURE_PI_PAGE: PiPage = piPageFixture();
export const FIXTURE_PI_PROBE: PiProbe = piProbeFixture();
export const FIXTURE_AI_PAGE: AiPage = aiPageFixture();

export type PiFixture = {
  stub: ApiStub;
  snapshot: () => { pi: PiPage; probe: PiProbe; ai: AiPage; schema: SettingsSchema; config: PiConfigView; networkProxy: NetworkProxyView };
};

/** Pi 页读写：pi / piProbe / ai / savePiSettings / piConfig / savePiConfig，以及注册表选项 settingsSchema / setSetting（共享描述符，校验失败由场景注入）。 */
export function createPiFixture(): PiFixture {
  const state = {
    pi: clone(FIXTURE_PI_PAGE),
    probe: clone(FIXTURE_PI_PROBE),
    ai: clone(FIXTURE_AI_PAGE),
    schema: createSettingsSchema(),
    config: piConfigFixture(),
    networkProxy: networkProxyFixture(),
  };
  const stub = createApiStub();
  stub.handle("pi", () => clone(state.pi));
  stub.handle("piProbe", () => clone(state.probe));
  stub.handle("piInstallStatus", () => ({ state: "idle", received: 0, total: null, message: "", dir: "" }));
  stub.handle("ai", () => clone(state.ai));
  // 复合保存只剩可执行文件路径；空闲超时与思考强度走 `setSetting`（阶段 19-1）。
  stub.handle("savePiSettings", (args) => {
    const piPath = args.pi_path.trim();
    state.pi = { ...state.pi, pi_path: piPath };
    return clone(state.pi);
  });
  // 覆盖只收 JSON 对象，空白还原为 `{}`；后端的完整校验不在这里复制。
  stub.handle("piConfig", () => clone(state.config));
  stub.handle("networkProxy", () => clone(state.networkProxy));
  stub.handle("testNetworkProxy", () => ({ ok: true, latency_ms: 42, status: 200, message: "GitHub 可访问 · HTTP 200 · 42 ms" }));
  stub.handle("saveNetworkProxy", (args) => {
    const next = clone(state.networkProxy);
    for (const key of ["global", "content", "api", "pi"] as const) {
      const input = args[key];
      const saved = next[key];
      next[key] = { mode: input.mode, url: input.url, no_proxy: input.no_proxy, has_auth: !!input.url && (input.clear_auth ? false : input.username !== undefined || input.password !== undefined || saved.has_auth) };
    }
    state.networkProxy = next;
    return clone(next);
  });
  stub.handle("savePiConfig", (args) => {
    const next = { ...state.config };
    for (const key of ["settings_override", "models_override"] as const) {
      const raw = args[key];
      if (raw === undefined) continue;
      const value: unknown = raw.trim() ? JSON.parse(raw) : {};
      if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("覆盖必须是 JSON 对象");
      next[key] = JSON.stringify(value);
    }
    if (args.use_own_config !== undefined) next.use_own_config = args.use_own_config;
    if (args.own_model !== undefined) next.own_model = args.own_model;
    state.config = next;
    return clone(next);
  });
  stub.handle("settingsSchema", () => clone(state.schema));
  stub.handle("setSetting", (id, value) => {
    const spec = state.schema.specs.find((s) => s.id === id);
    if (!spec) throw new Error(`没有名为 ${id} 的设置项`);
    if (id === "developer_tools") state.pi = { ...state.pi, developer_tools: value === "1" };
    if (id === "fetch_url_enabled") state.pi = { ...state.pi, fetch_url: value === "1" };
    state.schema.values = state.schema.values.map((v) => (v.id === id ? { id, value } : v));
    return { id, value };
  });
  return {
    stub,
    snapshot: () => clone(state),
  };
}
