import { settingsQueryPolicy } from "./queryPolicy";
import { mutationOptions, queryOptions, type QueryClient } from "@tanstack/react-query";
import type { TranslatePage } from "../bindings";
import { api } from "../lib/api";
import { formatNotice } from "../lib/notice";
import { redactAiText, secretsFromUrl } from "./aiError";
import { settingsChanged } from "./editState";

export const translateKeys = {
  page: ["settings", "translate"] as const,
  write: ["settings", "translate", "write"] as const,
};

// 纯读取，无参数；预载与挂载必须复用同一 key / policy。仅驻留内存。
export const translateQueryOptions = () => queryOptions({
  queryKey: translateKeys.page,
  queryFn: () => api.translatePage(),
  ...settingsQueryPolicy,
});

export const TRANSLATE_BACKENDS = [
  { id: "complete", label: "大模型" },
  { id: "deepl", label: "DeepL 官方" },
  { id: "deeplx", label: "DeepLX" },
  { id: "http", label: "HTTP 服务" },
];

export function prefetchTranslateSettings(client: QueryClient) {
  if (client.isMutating({ mutationKey: translateKeys.write })) return Promise.resolve();
  return client.prefetchQuery(translateQueryOptions());
}

export type TranslateDraft = Pick<TranslatePage, "enabled" | "backend" | "target_lang" | "deepl_url" | "deeplx_url" | "http_url"> & {
  deepl_key?: string; deeplx_token?: string; http_key?: string;
};

export function translateDraft(page: TranslatePage): TranslateDraft {
  return {
    enabled: page.enabled, backend: page.backend, target_lang: page.target_lang,
    deepl_key: undefined, deepl_url: page.deepl_url,
    deeplx_url: page.deeplx_url, deeplx_token: undefined,
    http_url: page.http_url, http_key: undefined,
  };
}

export function sameTranslateDraft(a: TranslateDraft, b: TranslateDraft) {
  return (Object.keys(a) as (keyof TranslateDraft)[]).every((key) => a[key] === b[key]);
}

export function translateError(prefix: string, error: unknown, draft?: TranslateDraft) {
  const secrets = draft ? [draft.deepl_key ?? "", draft.deeplx_token ?? "", draft.http_key ?? "",
    ...[draft.deepl_url, draft.deeplx_url, draft.http_url].flatMap(secretsFromUrl)] : [];
  return formatNotice(prefix, redactAiText(String(error), secrets));
}

export type TranslateAction = "save" | "test";
export type TranslateStatus = { tone: "ok" | "err"; text: string };
type WriteArgs = { draft: TranslateDraft; action: TranslateAction };

export function translateMutationOptions(client: QueryClient) {
  return mutationOptions({
    mutationKey: translateKeys.write,
    retry: false,
    networkMode: "always",
    // mutation variables 含密钥；完成且无观察者后立即回收，不持久化。
    gcTime: 0,
    mutationFn: async (args: WriteArgs) => {
      await client.cancelQueries({ queryKey: translateKeys.page, exact: true });
      const submitted = { ...args.draft, target_lang: args.draft.target_lang || "zh" };
      let saved = false;
      let page: TranslatePage | undefined;
      let status: TranslateStatus;
      try {
        await api.saveTranslateSettings(submitted);
        saved = true;
        if (args.action === "test") {
          const sample = await api.testTranslate();
          status = { tone: "ok", text: "设置已保存，测试翻译：" + redactAiText(sample, [submitted.deepl_key ?? "", submitted.deeplx_token ?? "", submitted.http_key ?? ""]) };
        } else {
          status = { tone: "ok", text: "翻译设置已保存。" };
        }
      } catch (error) {
        const prefix = args.action === "test"
          ? saved ? "设置已保存，但测试失败：" : "保存失败，未开始测试："
          : "保存失败：";
        status = { tone: "err", text: translateError(prefix, error, submitted) };
      }
      // 没有权威回读时不拼造页面快照。
      if (saved) {
        await settingsChanged(client);
        client.removeQueries({ queryKey: translateKeys.page, exact: true });
        try {
          page = await client.fetchQuery(translateQueryOptions());
        } catch (error) {
          status = { tone: "err", text: status.text + " 设置已保存，但" +
            translateError("读取翻译设置失败，当前输入已保留：", error, submitted) };
        }
      }
      return { saved, submitted: saved ? { ...submitted, deepl_key: undefined, deeplx_token: undefined, http_key: undefined } : submitted, page, status };
    },
  });
}
