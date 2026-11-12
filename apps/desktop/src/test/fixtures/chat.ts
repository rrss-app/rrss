import type { ContextUsageView, DigestThreadView, ThreadView } from "../../bindings";

export function contextUsageFixture(overrides: Partial<ContextUsageView> = {}): ContextUsageView {
  return { used: 0, window: null, at: 0, ...overrides };
}

export function threadFixture(overrides: Partial<ThreadView> = {}): ThreadView {
  return { id: 1, title: "测试对话", chat_model: null, chat_thinking: null, context: null, folder_id: null, pinned: false, updated_at: 0, archived_at: null, ...overrides };
}

export function digestThreadFixture(overrides: Partial<DigestThreadView> = {}): DigestThreadView {
  return { thread_id: 0, chat_model: null, chat_thinking: null, context: null, messages: [], has_more: false, ...overrides };
}
