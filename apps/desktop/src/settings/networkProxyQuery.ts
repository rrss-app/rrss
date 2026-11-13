import { settingsQueryPolicy } from "./queryPolicy";
import { mutationOptions, queryOptions, type QueryClient } from "@tanstack/react-query";
import { api, type NetworkProxyRuleView, type NetworkProxyTestArgs, type NetworkProxyView, type SaveNetworkProxyArgs, type SaveNetworkProxyRuleArgs } from "../lib/api";
import { settingsChanged } from "./editState";

export const networkProxyKeys = {
  page: ["settings", "networkProxy", "page"] as const,
  write: ["settings", "networkProxy", "write"] as const,
};


export const networkProxyQueryOptions = () => queryOptions({
  ...settingsQueryPolicy,
  queryKey: networkProxyKeys.page,
  queryFn: () => api.networkProxy(),
});

/** 已保存的认证只以 `has_auth` 表示；用户名 / 密码栏只用于替换，留空保留旧值。 */
export type NetworkProxyDraftRule = NetworkProxyRuleView & {
  username: string;
  password: string;
  clear_auth: boolean;
};

export type ProxyScope = "global" | "content" | "api" | "pi";
export type NetworkProxyDraft = Record<ProxyScope, NetworkProxyDraftRule>;
/** 可单独覆盖的三个用途，顺序即界面顺序。 */
export const PROXY_USES = ["content", "api", "pi"] as const satisfies readonly ProxyScope[];
export type ProxyUse = (typeof PROXY_USES)[number];

/** 草稿的键顺序固定为视图顺序，`sameDraft` 依赖它做比较。 */
export function draftFromView(view: NetworkProxyView): NetworkProxyDraft {
  const rule = (saved: NetworkProxyRuleView): NetworkProxyDraftRule => ({ ...saved, username: "", password: "", clear_auth: false });
  return { global: rule(view.global), content: rule(view.content), api: rule(view.api), pi: rule(view.pi) };
}

export function effectiveRule(draft: NetworkProxyDraft, scope: ProxyScope): NetworkProxyDraftRule {
  return draft[scope].mode === "inherit" ? draft.global : draft[scope];
}

export function isSocks(rule: NetworkProxyDraftRule): boolean {
  return rule.mode === "custom" && /^socks5h?:\/\//i.test(rule.url.trim());
}

/**
 * 粘贴含认证的地址时把用户名 / 密码移出地址栏，避免明文留在输入框里；
 * 认证仍随保存提交，后端照常只回 `has_auth`。
 */
export function withPastedAuth(rule: NetworkProxyDraftRule, url: string): NetworkProxyDraftRule {
  const match = /^([a-z][a-z0-9+.-]*:\/\/)([^@/?#]*)@(.*)$/i.exec(url.trim());
  if (!match) return { ...rule, url };
  const [, scheme, userinfo, rest] = match;
  const split = userinfo.indexOf(":");
  const decode = (value: string) => { try { return decodeURIComponent(value); } catch { return value; } };
  return {
    ...rule,
    url: scheme + rest,
    username: decode(split < 0 ? userinfo : userinfo.slice(0, split)),
    password: split < 0 ? "" : decode(userinfo.slice(split + 1)),
    clear_auth: false,
  };
}

/** 只拦明显缺 scheme 或误填 URL 的情况；主机、端口和网段的完整校验以后端为准。 */
export function validateProxyDraft(draft: NetworkProxyDraft): string | null {
  for (const rule of Object.values(draft)) {
    if (rule.mode === "custom" && !/^(https?|socks5h?):\/\/\S/i.test(rule.url.trim())) return "请填写完整代理地址，例如 http://127.0.0.1:7890。";
    if (rule.no_proxy.split(/[,\s]+/).some((value) => value.includes("://") || value.includes("@"))) return "不代理地址只填写域名、IP、网段或 *。";
  }
  return null;
}

export function ruleForSave(rule: NetworkProxyDraftRule): SaveNetworkProxyRuleArgs {
  return {
    mode: rule.mode,
    url: rule.url,
    no_proxy: rule.no_proxy,
    username: rule.username || undefined,
    password: rule.password || undefined,
    clear_auth: rule.clear_auth,
  };
}

export function argsFromDraft(draft: NetworkProxyDraft): SaveNetworkProxyArgs {
  return {
    global: ruleForSave(draft.global),
    content: ruleForSave(draft.content),
    api: ruleForSave(draft.api),
    pi: ruleForSave(draft.pi),
  };
}

/** 测活带整份草稿：跟随全局与认证留空的合并交给后端，与保存后的实际线路一致。 */
export function testArgsFromDraft(scope: ProxyUse, draft: NetworkProxyDraft): NetworkProxyTestArgs {
  return { scope, draft: argsFromDraft(draft) };
}

export function sameDraft(a: NetworkProxyDraft, b: NetworkProxyDraft): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

export function networkProxyMutationOptions(client: QueryClient) {
  return mutationOptions({
    mutationKey: networkProxyKeys.write,
    retry: false,
    networkMode: "always" as const,
    gcTime: 0,
    mutationFn: async (args: SaveNetworkProxyArgs) => {
      await client.cancelQueries({ queryKey: ["settings", "networkProxy"] });
      const view = await api.saveNetworkProxy(args);
      await settingsChanged(client);
      client.setQueryData(networkProxyKeys.page, view);
      return view;
    },
  });
}

/** 与网络设置连接检查共用的线路文案；代理认证不会出现在摘要中。 */
export function routeSummary(rule: NetworkProxyRuleView): string {
  if (rule.mode !== "custom") return { inherit: "跟随全局", direct: "无代理", system: "系统代理" }[rule.mode];
  const match = /^([a-z0-9]+):\/\/(?:[^@/?#]*@)?([^/?#]+)/i.exec(rule.url.trim());
  return match ? `${match[1].toUpperCase()} · ${match[2]}` : "自定义代理 · 待填写地址";
}
