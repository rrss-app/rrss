import type { FeedTree } from "../bindings";
import { deferred, type Deferred } from "./deferred";
import { applyFeedTreeOrder } from "./treeOrder";

export type ProductionApi = typeof import("../lib/api").api;
export type ApiName = keyof ProductionApi;

export class UnexpectedApiCallError extends Error {
  readonly method: string;
  constructor(method: string) {
    super(`unexpected api.${method} call`);
    this.name = "UnexpectedApiCallError";
    this.method = method;
  }
}

export type ApiCallLog = { method: ApiName };

export type ApiHandler<K extends ApiName> = (
  ...args: Parameters<ProductionApi[K]>
) => ReturnType<ProductionApi[K]> | Awaited<ReturnType<ProductionApi[K]>>;

export type ApiStub = {
  readonly api: ProductionApi;
  readonly calls: readonly ApiCallLog[];
  handle: <K extends ApiName>(name: K, handler: ApiHandler<K>) => void;
  /** 覆盖下次响应并跳过 handler。延迟提交或延迟确认请用 handle + deferred。 */
  deferNext: <K extends ApiName>(name: K) => Deferred<Awaited<ReturnType<ProductionApi[K]>>>;
  callCount: (name: ApiName) => number;
  assertNoUnexpectedCalls: () => void;
  /** 给 moveTreeItems / restoreTreeLayout 一份会按订阅树规则改写的树。 */
  seedFeeds: (tree: FeedTree) => void;
  readFeeds: () => FeedTree;
};

type Queued<K extends ApiName = ApiName> = Deferred<Awaited<ReturnType<ProductionApi[K]>>>;

export function createApiStub(): ApiStub {
  const handlers = new Map<ApiName, ApiHandler<ApiName>>();
  handlers.set("schedulerStatus", async () => ({ run_id: "test", version: 0, operations: [], background_requested: 0, background_completed: 0, background: [], stopped: false, feed_pause: null }));
  let feeds: FeedTree = { folders: [], uncategorized: [], folder_combo: ["未分类"] };
  handlers.set("focusClusterMembers", async () => []);
  // 组件测试按浏览器预览处理：没有宿主标题栏，不画窗口三键。
  handlers.set("windowChrome", async () => null);
  // 同样没有托盘；根壳启动时交托盘菜单文字。
  handlers.set("trayInstall", async () => false);
  handlers.set("moveTreeItems", ((...args: Parameters<ProductionApi["moveTreeItems"]>) => {
    const [items, parentFolderId, before] = args;
    const result = applyFeedTreeOrder(feeds, "move", { items, parentFolderId, before });
    feeds = result.tree;
    return result.layout;
  }) as ApiHandler<ApiName>);
  handlers.set("restoreTreeLayout", ((...args: Parameters<ProductionApi["restoreTreeLayout"]>) => {
    feeds = applyFeedTreeOrder(feeds, "restore", { layout: args[0] }).tree;
  }) as ApiHandler<ApiName>);
  const queues = new Map<ApiName, Queued[]>();
  const calls: ApiCallLog[] = [];
  const unexpected: ApiName[] = [];
  const methods = {} as ProductionApi;

  function dispatch(name: ApiName, args: unknown[]): Promise<unknown> {
    calls.push({ method: name });
    const queued = queues.get(name)?.shift();
    // deferNext 只替换本次 Promise，跳过 handler，因此写操作不会执行副作用。
    if (queued) return queued.promise;
    const handler = handlers.get(name);
    if (!handler) {
      unexpected.push(name);
      return Promise.reject(new UnexpectedApiCallError(String(name)));
    }
    return Promise.resolve().then(() =>
      (handler as (...handlerArgs: unknown[]) => unknown)(...args),
    );
  }

  const api = new Proxy(methods, {
    get(target, method) {
      if (typeof method !== "string" || method === "then") return undefined;
      const name = method as ApiName;
      if (!Object.prototype.hasOwnProperty.call(target, name)) {
        (target as Record<string, unknown>)[name] = (...args: never[]) => dispatch(name, args);
      }
      return (target as Record<string, unknown>)[name];
    },
  });

  return {
    api,
    calls,
    handle(name, handler) {
      handlers.set(name, handler as ApiHandler<ApiName>);
    },
    /**
     * 覆盖该命令下一次调用的响应，并跳过已注册 handler，因此不会执行写入。
     * 延迟提交：`handle` 的 async handler 先 `await gate.promise`，成功后再改 fixture。
     * 立即写入、只延迟确认：先改 fixture，再返回 `gate.promise`。
     */
    deferNext(name) {
      const pending = deferred<Awaited<ReturnType<ProductionApi[typeof name]>>>();
      const queue = queues.get(name) ?? [];
      queue.push(pending as Queued);
      queues.set(name, queue);
      return pending;
    },
    callCount(name) {
      return calls.filter((call) => call.method === name).length;
    },
    assertNoUnexpectedCalls() {
      if (unexpected.length === 0) return;
      throw new Error(`unexpected api calls: ${unexpected.join(", ")}`);
    },
    seedFeeds(tree) { feeds = structuredClone(tree); },
    readFeeds() { return structuredClone(feeds); },
  };
}

export function createApiProxy(getStub: () => ApiStub | null): ProductionApi {
  return new Proxy({} as ProductionApi, {
    get(_target, method) {
      if (typeof method !== "string" || method === "then") return undefined;
      const stub = getStub();
      if (!stub) {
        return () => Promise.reject(new UnexpectedApiCallError(method));
      }
      return stub.api[method as ApiName];
    },
  });
}
