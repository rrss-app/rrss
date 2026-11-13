import { settingsQueryPolicy } from "./queryPolicy";
import type { McpInput } from "../bindings";
import { mutationOptions, queryOptions, type QueryClient } from "@tanstack/react-query";
import { api, type SavePiConfigArgs, type SkillDraft } from "../lib/api";
import { settingsChanged } from "./editState";
import { formatNotice } from "../lib/notice";
import { forgetServer } from "./mcpRuntime";
import { i18n } from "../i18n";

export const piKeys = {
  page: ["settings", "pi", "page"] as const,
  probe: ["settings", "pi", "probe"] as const,
  config: ["settings", "pi", "config"] as const,
  generated: ["settings", "pi", "generated"] as const,
  write: ["settings", "pi", "write"] as const,
};
export const piQueryOptions = () => queryOptions({ ...settingsQueryPolicy, queryKey: piKeys.page, queryFn: () => api.pi() });
export const piProbeOptions = () => queryOptions({ ...settingsQueryPolicy, queryKey: piKeys.probe, queryFn: () => api.piProbe() });
export const piConfigOptions = () => queryOptions({ ...settingsQueryPolicy, queryKey: piKeys.config, queryFn: () => api.piConfig() });
// 生成配置只在打开查看时读，每次都读最新的。
export const generatedConfigOptions = () => queryOptions({ ...settingsQueryPolicy, staleTime: 0, gcTime: 0, queryKey: piKeys.generated, queryFn: () => api.piGeneratedConfig() });
export function prefetchPiSettings(client: QueryClient) {
  if (client.isMutating({ mutationKey: piKeys.write })) return Promise.resolve();
  // 启动预载绝不执行 pi --version。
  return client.prefetchQuery(piQueryOptions());
}

export type PiAction =
  | { type: "savePath"; path: string }
  | { type: "developerTools" | "fetchUrl"; enabled: boolean }
  | { type: "tool" | "skill"; name: string; enabled: boolean }
  | { type: "toolDescription"; name: string; description: string }
  | { type: "skillSave"; draft: SkillDraft }
  | { type: "skillDelete"; name: string }
  | { type: "mcpToggle"; id: number; enabled: boolean }
  | { type: "mcpDelete"; id: number }
  /** `saved` 收到写入后的服务 id（新服务保存后要接着登录时用）。 */
  | { type: "mcpSave"; input: McpInput; saved?: (id: number) => void }
  | { type: "mcpImport"; json: string }
  | { type: "config"; args: SavePiConfigArgs }
  | { type: "restart" };
const label = (type: PiAction["type"]) => i18n.t(`pi.label.${type}`, { ns: "settingsAssistant" });
export type PiStatus = { tone: "ok" | "err"; text: string };

async function perform(action: PiAction): Promise<string> {
  switch (action.type) {
    case "savePath": await api.savePiSettings({ pi_path: action.path }); return "";
    case "developerTools": await api.setSetting("developer_tools", action.enabled ? "1" : "0"); return "";
    case "fetchUrl": await api.setSetting("fetch_url_enabled", action.enabled ? "1" : "0"); return "";
    case "tool": await api.togglePiTool(action.name, action.enabled); return "";
    case "toolDescription": await api.setPiToolDescription(action.name, action.description); return "";
    case "skill": await api.toggleSkill(action.name, action.enabled); return "";
    case "skillSave": await api.saveSkill(action.draft); return "";
    case "skillDelete": await api.deleteSkill(action.name); return "";
    case "mcpToggle": await api.toggleMcp(action.id, action.enabled); return "";
    case "mcpDelete": await api.deleteMcp(action.id); forgetServer(action.id); return "";
    case "mcpSave": {
      // 先写入再回报 id：`saved?.(await …)` 在没有回调时连写入都会跳过。
      const id = await api.upsertMcp(action.input);
      action.saved?.(id);
      return "";
    }
    case "mcpImport": {
      const entries = await api.importMcp(action.json);
      return i18n.t("pi.imported", { ns: "settingsAssistant", imported: entries.filter((e) => !e.skipped).length, skipped: entries.filter((e) => e.skipped).length });
    }
    case "config": await api.savePiConfig(action.args); return "";
    case "restart": await api.restartPi(); return i18n.t("pi.restarted", { ns: "settingsAssistant" });
  }
}

/**
 * 对话助手页所有写入共用一条互斥通道。写入前后取消旧读取，完成后在锁内重读受影响的查询；
 * 重读失败只接在提示后面，不把已完成的写入报成失败。
 */
export function piMutationOptions(client: QueryClient) {
  return mutationOptions({
    mutationKey: piKeys.write, retry: false, networkMode: "always", gcTime: 0,
    mutationFn: async (action: PiAction): Promise<{ completed: boolean; status: PiStatus }> => {
      // IPC 不可取消；查询取消仅阻止旧结果重新发布到共享快照。
      await client.cancelQueries({ queryKey: ["settings", "pi"] });
      let status: PiStatus;
      let completed = false;
      try {
        const message = await perform(action);
        completed = true;
        status = { tone: "ok", text: message || i18n.t("pi.updated", { ns: "settingsAssistant" }) };
      } catch (error) {
        return { completed, status: { tone: "err", text: formatNotice(i18n.t("pi.failed", { ns: "settingsAssistant", label: label(action.type) }), error) } };
      }
      // 探测要跑 `pi --version`，不在锁内重读：随全部设置读取标记过期，页面解锁后自行重读。
      await settingsChanged(client);
      const reads: Promise<unknown>[] = [client.fetchQuery({ ...piQueryOptions(), staleTime: 0 })];
      if (action.type === "config") reads.push(client.fetchQuery({ ...piConfigOptions(), staleTime: 0 }));
      const failed = (await Promise.allSettled(reads)).find((r) => r.status === "rejected");
      if (failed) {
        status = { tone: "err", text: status.text + " " + formatNotice(i18n.t("pi.refreshFailed", { ns: "settingsAssistant" }), (failed as PromiseRejectedResult).reason) };
      }
      return { completed, status };
    },
  });
}
