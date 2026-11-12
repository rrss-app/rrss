import { mutationOptions, queryOptions, type QueryClient } from "@tanstack/react-query";
import {
  api,
  type ConnectionTestArgs,
  type ConnectionTestView,
  type ModelPoolView,
  type SaveAiSettingsArgs,
  type SaveModelBindingsArgs,
  type SaveModelPoolArgs,
  type UpdatePoolModelArgs,
} from "../lib/api";
import { aiKeys } from "./aiKeys";
import { settingsChanged } from "./editState";

export { aiKeys };

const policy = {
  staleTime: 30_000, gcTime: 5 * 60_000, retry: false, retryOnMount: false,
  refetchOnMount: true, refetchOnWindowFocus: false, refetchOnReconnect: false,
  networkMode: "always" as const,
};

// 连接列表不含完整密钥（阶段 19-9），只有状态与首尾各 4 位。
// IPC 不可取消；消费 signal 让 query 取消隔离迟到快照。
export const aiQueryOptions = () => queryOptions({
  ...policy, queryKey: aiKeys.page, queryFn: ({ signal }) => { void signal; return api.ai(0); },
});

export const bindingsQueryOptions = () => queryOptions({
  ...policy, queryKey: aiKeys.bindings, queryFn: ({ signal }) => { void signal; return api.modelBindings(); },
});

export const poolQueryOptions = () => queryOptions({
  ...policy, queryKey: aiKeys.pool, queryFn: ({ signal }) => { void signal; return api.modelPool(); },
});

/** 预设是后端常量，一次读取即可。 */
export const presetsQueryOptions = () => queryOptions({
  ...policy, staleTime: Infinity, queryKey: aiKeys.presets, queryFn: () => api.providerPresets(),
});

export function prefetchAiSettings(client: QueryClient) {
  if (client.isMutating({ mutationKey: aiKeys.write })) return Promise.resolve();
  return Promise.all([client.prefetchQuery(aiQueryOptions()), client.prefetchQuery(bindingsQueryOptions()),
    client.prefetchQuery(poolQueryOptions())]);
}

export type AiWrite =
  | { type: "save"; args: SaveAiSettingsArgs }
  | { type: "delete"; id: number }
  | { type: "bindings"; args: SaveModelBindingsArgs }
  /** 加入与移出模型；移出正被用途使用的模型时这些用途改用 `replace`，缺省则暂停。 */
  | { type: "pool"; args: SaveModelPoolArgs }
  /** 改一个模型的设置。 */
  | { type: "model"; args: UpdatePoolModelArgs }
  /** 「测试能力」：向模型发请求，结果存进模型记录。 */
  | { type: "probe"; providerId: number; model: string }
  /** 获取并记住连接的模型目录；网络请求，失败时目录不变。 */
  | { type: "catalog"; providerId: number };

export type AiWriteResult = { savedId: number; reread: unknown; pool?: ModelPoolView };

/**
 * 模型页所有写入共用一条互斥通道。宿主返回的快照直接回写；受影响的其他查询在写入锁内重读，
 * 重读失败只留在该查询的错误状态里，返回给调用方区分「写入失败」与「已写入但刷新失败」。
 */
export function aiMutationOptions(client: QueryClient) {
  return mutationOptions({
    mutationKey: aiKeys.write,
    retry: false,
    networkMode: "always",
    // 变量可能含新填的密钥；完成且无观察者后立即回收。
    gcTime: 0,
    mutationFn: async (write: AiWrite): Promise<AiWriteResult> => {
      await Promise.all([
        client.cancelQueries({ queryKey: aiKeys.page, exact: true }),
        client.cancelQueries({ queryKey: aiKeys.bindings, exact: true }),
        client.cancelQueries({ queryKey: aiKeys.pool, exact: true }),
      ]);
      let savedId = 0;
      let pool: ModelPoolView | undefined;
      const reread: Array<Promise<unknown>> = [];
      if (write.type === "bindings") {
        const view = await api.saveModelBindings(write.args);
        await settingsChanged(client);
        client.setQueryData(aiKeys.bindings, view);
        // 「用于 N 项」跟着用途变。
        reread.push(client.fetchQuery({ ...poolQueryOptions(), staleTime: 0 }));
      } else if (write.type === "pool" || write.type === "catalog" || write.type === "model" || write.type === "probe") {
        pool = write.type === "pool" ? await api.saveModelPool(write.args)
          : write.type === "model" ? await api.updatePoolModel(write.args)
          : write.type === "probe" ? await api.probePoolModel(write.providerId, write.model)
          : await api.fetchModelCatalog(write.providerId);
        await settingsChanged(client);
        client.setQueryData(aiKeys.pool, pool);
        // 用途行显示所选模型的能力；移出模型还可能改了用途。
        if (write.type !== "catalog") reread.push(client.fetchQuery({ ...bindingsQueryOptions(), staleTime: 0 }));
        // 连接行显示「已加入 N 个模型」。
        if (write.type === "pool") reread.push(client.fetchQuery({ ...aiQueryOptions(), staleTime: 0 }));
      } else {
        if (write.type === "save") {
          const page = await api.saveAiSettings(write.args);
          await settingsChanged(client);
          client.setQueryData(aiKeys.page, page);
          savedId = page.selected_id;
        } else {
          await api.deleteProvider(write.id);
          await settingsChanged(client);
          reread.push(client.fetchQuery({ ...aiQueryOptions(), staleTime: 0 }));
          // 连接的目录与模型随之删除，用到它的用途变为未选择。
          reread.push(client.fetchQuery({ ...poolQueryOptions(), staleTime: 0 }));
          reread.push(client.fetchQuery({ ...bindingsQueryOptions(), staleTime: 0 }));
        }
      }
      const failed = (await Promise.allSettled(reread)).find((r) => r.status === "rejected");
      return { savedId, reread: failed ? (failed as PromiseRejectedResult).reason : null, pool };
    },
  });
}

/**
 * 「测试」连接：只请求一次模型列表。测的是已保存的地址与密钥时宿主把结果记到连接上，
 * 这里让连接列表重新读取；草稿的结果只给这次编辑。
 */
export async function testConnection(client: QueryClient, args: ConnectionTestArgs): Promise<ConnectionTestView> {
  const result = await api.testConnection(args);
  if (args.provider_id > 0 && !client.isMutating({ mutationKey: aiKeys.write })) {
    void client.invalidateQueries({ queryKey: aiKeys.page });
  }
  return result;
}
