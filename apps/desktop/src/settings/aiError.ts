/** AI 设置失败信息：中文归类、折叠详情、去敏 Key / URL 凭证。 */

export type AiErrorKind =
  | "auth"
  | "forbidden"
  | "notFound"
  | "rateLimit"
  | "timeout"
  | "network"
  | "server"
  | "other";

export type AiErrorView = {
  kind: AiErrorKind;
  title: string;
  detail: string;
};

const TITLE: Record<AiErrorKind, string> = {
  auth: "认证失败，请检查 API Key 是否正确。",
  forbidden: "没有访问权限，请检查账号或 Key 的权限。",
  notFound: "找不到地址或模型，请核对接口地址和模型名。",
  rateLimit: "请求过于频繁，请稍后再试。",
  timeout: "连接超时。服务可能未启动，或网络不通。",
  network: "无法连上接口。请确认地址、本机服务和网络。",
  server: "接口服务器出错，请稍后再试。",
  other: "操作失败。可展开查看详情。",
};

export function rawErrorText(err: unknown): string {
  return String(err ?? "").replace(/^Error:\s*/, "").trim();
}

export function secretsFromUrl(url: string): string[] {
  const raw = url.trim();
  if (!raw) return [];
  const found: string[] = [];
  try {
    const parsed = new URL(raw.includes("://") ? raw : `http://${raw}`);
    if (parsed.username) found.push(decodeURIComponent(parsed.username));
    if (parsed.password) found.push(decodeURIComponent(parsed.password));
    parsed.searchParams.forEach((value, key) => {
      if (/key|token|secret|password|auth/i.test(key) && value) found.push(value);
    });
  } catch {
    const userinfo = raw.match(/^https?:\/\/([^/@]+)@/i);
    if (userinfo?.[1]?.includes(":")) {
      const [user, pass] = userinfo[1].split(":");
      if (user) found.push(user);
      if (pass) found.push(pass);
    }
  }
  return found.filter((s) => s.length >= 4);
}

export function redactAiText(text: string, extraSecrets: string[] = []): string {
  let out = text;
  const secrets = extraSecrets
    .map((s) => s.trim())
    .filter((s) => s.length > 0)
    .sort((a, b) => b.length - a.length);
  for (const secret of secrets) {
    out = out.split(secret).join("***");
  }
  out = out.replace(/:\/\/([^/\s]*):([^/\s]*)@/g, "://***:***@");
  out = out.replace(
    /([?&](?:api[_-]?key|access[_-]?token|token|secret|password|auth)=)[^&\s"'<>]+/gi,
    "$1***",
  );
  out = out.replace(/(Bearer\s+)[A-Za-z0-9._\-+=/]+/gi, "$1***");
  out = out.replace(/\bsk-[A-Za-z0-9]{8,}\b/g, "***");
  return out;
}

export function classifyAiError(raw: string): AiErrorKind {
  const t = raw;
  if (/\b401\b|unauthorized|invalid api key|incorrect api key|invalid.?key/i.test(t)) return "auth";
  if (/\b403\b|forbidden|access denied|not allowed/i.test(t)) return "forbidden";
  if (/\b404\b|not found|no such model/i.test(t)) return "notFound";
  if (/\b429\b|rate limit|too many requests/i.test(t)) return "rateLimit";
  if (/timed?\s*out|timeout|deadline exceeded|etimedout|time limit/i.test(t)) return "timeout";
  if (
    /econnrefused|enotfound|eai_again|econnreset|network|connection refused|dns|failed to fetch|error sending request|tcp connect|connect error|connection reset/i.test(
      t,
    )
  ) {
    return "network";
  }
  if (/\b5\d\d\b|internal server error|bad gateway|service unavailable/i.test(t)) return "server";
  return "other";
}

export function explainAiError(err: unknown, extraSecrets: string[] = []): AiErrorView {
  const detail = redactAiText(rawErrorText(err), extraSecrets);
  const kind = classifyAiError(rawErrorText(err));
  return { kind, title: TITLE[kind], detail };
}

export function secretsFromConnection(apiKey: string, baseUrl: string): string[] {
  return [apiKey, ...secretsFromUrl(baseUrl)];
}
