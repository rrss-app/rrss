import { sameChoice } from "../../settings/modelChoices";
import type {
  FeatureView, McpView, AiPage, CapsView, GeneratedConfig, ModelBindingsView, ModelPoolView, NetworkProxyView, PiConfigView, PiPage, PiProbe, PoolEntryView, ProviderView, RuleView, RulesPage, SaveModelPoolArgs, TranslatePage,
} from "../../bindings";

// 设置页视图的合成数据：组件测试、e2e 模拟宿主与性能场景都从这里取默认值，场景只覆盖需要的字段。
// 视图字段增删只改这里；不承担服务商预设或后端校验。

export function trainingFeaturesFixture(): FeatureView[] {
  return [
    { dimension: "title_kw", pattern: "融资" },
    { dimension: "author", pattern: "某某" },
    { dimension: "tag", pattern: "产品" },
    { dimension: "url_phrase", pattern: "example.test" },
  ];
}

export function ruleFixture(overrides: Partial<RuleView> = {}): RuleView {
  return {
    id: 1, polarity: "like", dimension_id: "title_kw", pattern: "Rust",
    summary: { code: "rule_summary", params: { effect: "boost" } }, scope_name: "", scope_kind: "global", scope_id: 0,
    weight: 10, source: "manual", enabled: true, pending: false, pack_id: "",
    ...overrides,
  };
}

export function rulesPageFixture(overrides: Partial<RulesPage> = {}): RulesPage {
  return { rules: [ruleFixture()], presets: [], folders: [], feeds: [], hide_threshold: -10, ...overrides };
}

export function providerFixture(overrides: Partial<ProviderView> = {}): ProviderView {
  return {
    id: 1,
    kind: "openai_compat",
    name: "测试连接",
    base_url: "http://127.0.0.1:9/v1",
    protocol: "chat",
    key: "none",
    key_hint: "",
    ...overrides,
  };
}

export function aiPageFixture(overrides: Partial<AiPage> = {}): AiPage {
  return {
    providers: [], selected_id: 0,
    usage: {
      generation: { today: 0, month: 0, paused: false },
      embedding: { today: 0, month: 0, paused: false },
      chat: { today: 0, month: 0, paused: false },
    },
    auto_summary: {
      today: 0, daily_limit: 100, window_days: 3, batch_size: 8, batch_minutes: 15,
      model_ready: false, per_article_tokens: 4000, digest_daily_tokens: 35000, notice: null,
      cluster_today: 0, cluster_daily_limit: 20, per_cluster_tokens: 2000, cluster_model_ready: false,
    },
    ...overrides,
  };
}

export function bindingsFixture(overrides: Partial<ModelBindingsView> = {}): ModelBindingsView {
  return { tasks: [], chat_uses_own_config: false, ...overrides };
}

const unknownCaps = (): CapsView => ({ tool_calling: "unknown", structured_output: "unknown", image_input: "unknown", reasoning: "unknown" });

/** 加进来的一个模型；能力默认未测。`effective` 按手动设置 > 测试结果推出，可覆盖。 */
export function poolEntryFixture(overrides: Partial<PoolEntryView> = {}): PoolEntryView {
  const detected = overrides.detected ?? unknownCaps();
  const overridesCaps = overrides.overrides ?? {};
  const pick = (k: keyof CapsView) => overridesCaps[k] === true ? "supported" as const : overridesCaps[k] === false ? "unsupported" as const : detected[k];
  return {
    provider_id: 1, model: "fixture-model", context_window: undefined, engine_context_window: undefined,
    chat: false, display_name: "", thinking: "off", uses: [],
    detected, overrides: overridesCaps,
    effective: { tool_calling: pick("tool_calling"), structured_output: pick("structured_output"), image_input: pick("image_input"), reasoning: pick("reasoning") },
    ...overrides,
  };
}

export function modelPoolFixture(overrides: Partial<ModelPoolView> = {}): ModelPoolView {
  return { pool: [], catalogs: [], replaced: [], unset: [], ...overrides };
}

/**
 * 模拟宿主的加入 / 移出：加入去重，对话列表为空时第一个自动用于对话；移出时按「用于哪些用途」改用替换模型或变为未选择。
 * `bindings` 给出时一并改写（模拟宿主传入可变的用途快照）。
 */
export function applyPoolChange(view: ModelPoolView, args: SaveModelPoolArgs, bindings?: ModelBindingsView): ModelPoolView {
  const removed = view.pool.filter((e) => args.remove.some((r) => sameChoice(r, e)));
  const kept = view.pool.filter((e) => !removed.includes(e));
  let chatEmpty = !kept.some((e) => e.chat);
  const added = args.add.filter((a) => !kept.some((e) => sameChoice(a, e))).map((a) => {
    const chat = chatEmpty && !/embed/i.test(a.model);
    if (chat) chatEmpty = false;
    return poolEntryFixture({ ...a, chat });
  });
  const replace = args.replace && [...kept, ...added].find((e) => sameChoice(e, args.replace!));
  const replaced: string[] = [];
  const unset: string[] = [];
  for (const task of bindings?.tasks ?? []) {
    if (!removed.some((e) => sameChoice(e, task))) continue;
    if (replace && (task.task !== "chat" || replace.chat)) {
      Object.assign(task, { provider_id: replace.provider_id, model: replace.model });
      replaced.push(task.task);
    } else {
      Object.assign(task, { provider_id: 0, model: "" });
      unset.push(task.task);
    }
  }
  const pool = [...kept, ...added].map((e) => ({ ...e, uses: (bindings?.tasks ?? []).filter((t) => sameChoice(t, e)).map((t) => t.task) }));
  return { ...view, pool, replaced, unset };
}

export function translatePageFixture(overrides: Partial<TranslatePage> = {}): TranslatePage {
  return {
    enabled: true, backend: "http", target_lang: "zh",
    selection_config: { identity: "selection-config-1", service: "HTTP 服务" },
    deepl_key: { state: "none", hint: "" }, deepl_url: "", deeplx_url: "http://127.0.0.1:1188", deeplx_token: { state: "none", hint: "" },
    http_url: "http://127.0.0.1:5000", http_key: { state: "none", hint: "" },
    ...overrides,
  };
}

export function networkProxyFixture(overrides: Partial<NetworkProxyView> = {}): NetworkProxyView {
  const rule = (mode: "direct" | "system" | "custom" | "inherit" = "direct") => ({ mode, url: "", no_proxy: "", has_auth: false });
  return { global: rule(), content: rule("inherit"), api: rule("inherit"), pi: rule("inherit"), system: { http: null, https: null, all: null, no_proxy: null, source: "环境变量", pac: null }, ...overrides };
}

export function piPageFixture(overrides: Partial<PiPage> = {}): PiPage {
  return {
    developer_tools: false, fetch_url: false,
    pi_path: "", binary_source: "fixture-binary", session_dir: "/tmp/rrss-fixture/sessions",
    manage_notice: { code: "legacy_text" as const, params: {}, detail: "托管 fixture" },
    system_prompt: "<!-- rrss-system v1 -->\n你是 RRSS 阅读助手。", thinking_levels: [],
    tools: [], skills: [], mcp_servers: [],
    ...overrides,
  };
}

export function piProbeFixture(overrides: Partial<PiProbe> = {}): PiProbe {
  return {
    binary_source: "fixture-binary", version: "0.87.1", version_state: "ok", config_error: null,
    ...overrides,
  };
}

export function piConfigFixture(overrides: Partial<PiConfigView> = {}): PiConfigView {
  return { settings_override: "{}", models_override: "{}", use_own_config: false, own_model: "", ...overrides };
}

export function generatedConfigFixture(overrides: Partial<GeneratedConfig> = {}): GeneratedConfig {
  return {
    own_config: false, program: "/tmp/rrss-fixture/bin/pi", args: ["--mode", "rpc", "--model", "rrss-1/test-model"],
    env: [["PI_OFFLINE", "1"]], files: [["agent/settings.json", '{\n  "cacheWarming": "off"\n}']], ignored: [],
    ...overrides,
  };
}

export function mcpFixture(overrides: Partial<McpView> = {}): McpView {
  return { id: 1, name: "filesystem", transport: "stdio", command: "npx", args: ["-y", "@modelcontextprotocol/server-filesystem"],
    url: "", env: [], headers: [], auth: "none", logged_in: false, include_tools: [], enabled: true, ...overrides };
}
