/** 模型页查询键；对话助手页也读其中的绑定。 */
export const aiKeys = {
  page: ["settings", "ai"] as const,
  /** 主模型与用途绑定；模型页与对话助手页共用。能力缓存挂在其下，随之失效。 */
  bindings: ["settings", "model-bindings"] as const,
  presets: ["settings", "provider-presets"] as const,
  /** 模型池与各连接的模型目录（阶段 19-6）。 */
  pool: ["settings", "model-pool"] as const,
  write: ["settings", "ai", "write"] as const,
};
