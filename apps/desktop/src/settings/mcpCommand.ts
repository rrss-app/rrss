/** 单行命令只做分词，不执行 shell 展开；反斜杠原样保留，兼容 Windows 路径。 */
export function parseCommand(line: string): { parts: string[]; complete: boolean } {
  const parts: string[] = [];
  let current = "";
  let quote: string | null = null;
  let started = false;
  for (const ch of line) {
    if (quote) {
      if (ch === quote) quote = null;
      else current += ch;
    } else if (ch === '"' || ch === "'") {
      quote = ch;
      started = true;
    } else if (/\s/.test(ch)) {
      if (started) parts.push(current);
      current = "";
      started = false;
    } else {
      current += ch;
      started = true;
    }
  }
  if (started) parts.push(current);
  return { parts, complete: quote === null };
}

/** 相邻引号片段表示同一个参数，单双引号同时出现也不需要转义反斜杠。 */
export function joinCommand(parts: string[]): string {
  return parts.map((part) => {
    if (part === "") return '""';
    if (!/[\s"']/.test(part)) return part;
    if (!part.includes('"')) return `"${part}"`;
    return `'${part.replace(/'/g, `'"'"'`)}'`;
  }).join(" ");
}

/** `npx -y @modelcontextprotocol/server-filesystem ~/notes` → `filesystem`。 */
export function guessName(parts: string[]): string {
  const target = parts.slice(1).find((part) => !part.startsWith("-")) ?? parts[0] ?? "";
  return target.split(/[\\/]/).pop()!
    .replace(/@[^@]*$/, "")
    .replace(/\.(m?js|py|exe)$/i, "")
    .replace(/^(mcp-)?(server-)?/, "")
    .replace(/(-mcp)?(-server)?$/, "");
}
