import { settingsQueryPolicy } from "./queryPolicy";
import { mutationOptions, queryOptions, type QueryClient } from "@tanstack/react-query";
import type { TranslatePage } from "../bindings";
import { api } from "../lib/api";
import { formatNotice } from "../lib/notice";
import { i18n } from "../i18n";
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

const backendLabel = (id: "complete" | "deepl" | "deeplx" | "http"): string => i18n.t(`translate.backend.${id}`, { ns: "settingsJobs" });
const backendOption = (id: "complete" | "deepl" | "deeplx" | "http") => ({ id, get label() { return backendLabel(id); } });

/** 标签用 getter 在读取时取文案，不在模块加载时求值。 */
export const TRANSLATE_BACKENDS = (["complete", "deepl", "deeplx", "http"] as const).map(backendOption);

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
          status = { tone: "ok", text: i18n.t("translate.notice.testOk", { ns: "settingsJobs" }) + redactAiText(sample, [submitted.deepl_key ?? "", submitted.deeplx_token ?? "", submitted.http_key ?? ""]) };
        } else {
          status = { tone: "ok", text: i18n.t("translate.notice.saved", { ns: "settingsJobs" }) };
        }
      } catch (error) {
        const prefix = args.action === "test"
          ? saved ? i18n.t("translate.notice.testFailed", { ns: "settingsJobs" }) : i18n.t("translate.notice.saveFailedNoTest", { ns: "settingsJobs" })
          : i18n.t("translate.notice.saveFailed", { ns: "settingsJobs" });
        status = { tone: "err", text: translateError(prefix, error, submitted) };
      }
      // 没有权威回读时不拼造页面快照。
      if (saved) {
        await settingsChanged(client);
        client.removeQueries({ queryKey: translateKeys.page, exact: true });
        try {
          page = await client.fetchQuery(translateQueryOptions());
        } catch (error) {
          status = { tone: "err", text: status.text + " " + i18n.t("translate.notice.savedButReadFailed", { ns: "settingsJobs",
              error: translateError(i18n.t("translate.notice.readFailedKept", { ns: "settingsJobs" }), error, submitted) }) };
        }
      }
      return { saved, submitted: saved ? { ...submitted, deepl_key: undefined, deeplx_token: undefined, http_key: undefined } : submitted, page, status };
    },
  });
}
