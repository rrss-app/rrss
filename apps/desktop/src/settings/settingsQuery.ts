import { settingsQueryPolicy } from "./queryPolicy";
import { mutationOptions, queryOptions, type QueryClient } from "@tanstack/react-query";
import { api, type SettingsSchema, type SettingValue } from "../lib/api";
import { isTauri } from "../lib/preview";
import { settingsChanged } from "./editState";
import { createSettingsSchema } from "./registry";

export const settingsKeys = {
  schema: ["settings", "schema"] as const,
  write: ["settings", "schema", "write"] as const,
};

export function settingsSchemaOptions() {
  return queryOptions({
    queryKey: settingsKeys.schema,
    queryFn: ({ signal, client }) => {
      void signal; // IPC 不可取消；消费 signal 让 query 取消隔离迟到快照。
      return isTauri() ? api.settingsSchema() : Promise.resolve(client.getQueryData<SettingsSchema>(settingsKeys.schema) ?? createSettingsSchema());
    },
    ...settingsQueryPolicy,
  });
}

export function prefetchSettingsSchema(client: QueryClient) {
  if (client.isMutating({ mutationKey: settingsKeys.write })) return Promise.resolve();
  return client.prefetchQuery(settingsSchemaOptions());
}

/** 整批登记写入；已成功的项立即回写，失败项及其后面的项不再提交。 */
export function settingsMutationOptions(client: QueryClient) {
  return mutationOptions({
    mutationKey: settingsKeys.write, retry: false, networkMode: "always", gcTime: 0,
    mutationFn: async (values: SettingValue[]) => {
      const saved: SettingValue[] = [];
      const cancelReads = () => client.cancelQueries({ queryKey: settingsKeys.schema, exact: true });
      await cancelReads();
      for (const input of values) {
        try {
          const value = isTauri() ? await api.setSetting(input.id, input.value) : input;
          // 登记项会改动别的页面（对话助手、翻译等），全部设置读取标记过期后再回写本项。
          await settingsChanged(client);
          client.setQueryData<SettingsSchema>(settingsKeys.schema, old => old && ({
            ...old,
            values: old.values.map(v => v.id === value.id ? value : v),
          }));
          saved.push(value);
        } catch (error) {
          return { saved, failure: { id: input.id, error } };
        }
      }
      return { saved, failure: null };
    },
  });
}
