import type { ExtensionAPI, ToolLoadout, ToolAnnotations } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { readFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

type Policy = { annotations: { key: keyof ToolAnnotations; value: boolean; writes: boolean }[]; name_words: string[]; default: boolean };
const policy: Policy = JSON.parse(readFileSync(join(dirname(fileURLToPath(import.meta.url)), "mcp-policy.json"), "utf8"));
export function writes(name: string, annotations?: ToolAnnotations): boolean {
  for (const rule of policy.annotations) if (annotations?.[rule.key] === rule.value) return rule.writes;
  const words = name.replace(/[A-Z]/g, c => `_${c.toLowerCase()}`).split(/[^a-z0-9]+/);
  return words.some(word => policy.name_words.includes(word)) || policy.default;
}
function summary(input: Record<string, unknown>): string {
  const raw = JSON.stringify(input);
  const count = Array.from(raw).length;
  const entries = Object.entries(input);
  const lines = entries.slice(0, 4).map(([key, value]) => {
    const text = `${key}: ${typeof value === "string" ? value : JSON.stringify(value)}`.replace(/[\r\n]+/g, " ↵ ");
    const chars = Array.from(text);
    return chars.length > 100 ? chars.slice(0, 100).join("") + "…" : text;
  });
  if (entries.length > 4 || lines.some(line => line.endsWith("…"))) {
    if (lines.length === 4) lines[3] = lines[3].slice(0, 70) + `…（共 ${count} 字）`;
    else lines.push(`（共 ${count} 字）`);
  }
  return lines.join("\n");
}

export function installMcpGuard(pi: ExtensionAPI) {
  const names = new Map<string, { server: string; tool: string }>();
  const remembered = new Set<string>();
  const capture = (loadout: ToolLoadout) => {
    for (const tool of loadout.registered) {
      if (!tool.name.startsWith("mcp__")) continue;
      // MCP's label preserves the original tool name, including punctuation and long names.
      const slash = tool.label.indexOf("/");
      const namespace = loadout.getNamespace(tool.name);
      if (slash > 0) names.set(tool.name, { server: namespace?.description || tool.label.slice(0, slash), tool: tool.label.slice(slash + 1) });
    }
    return { hiddenDeclarations: ["rrss_mcp_guard"] };
  };
  // Public loadout API exposes raw MCP labels; getAllTools intentionally omits them.
  // This observer is active but never declared to the model or executed as a proxy.
  pi.registerTool({ name: "rrss_mcp_guard", label: "MCP permission observer", description: "Internal permission observer",
    parameters: Type.Object({}), exposure: "model-only", prepareLoadout: capture,
    async execute() { throw new Error("Internal permission observer"); },
  });
  pi.on("session_start", () => { remembered.clear(); names.clear(); });
  pi.on("tool_call", async (event, ctx) => {
    if (!event.toolName.startsWith("mcp__")) return;
    const info = pi.getAllTools().find(tool => tool.name === event.toolName);
    const match = /^mcp__(.+?)__(.+)$/.exec(event.toolName);
    const name = names.get(event.toolName) ?? { server: info?.namespace?.description || match?.[1] || "外部工具", tool: match?.[2] || event.toolName };
    ctx.ui.notify("RRSS_MCP_STEP:" + JSON.stringify({ id: event.toolCallId, name: event.toolName, ...name }));
    if (!writes(name.tool, info?.annotations) || remembered.has(event.toolName)) return;
    const request = JSON.stringify({ server: name.server, tool: name.tool, args: summary(event.input), can_remember: true });
    const answer = await ctx.ui.select("RRSS_MCP_CONFIRM:" + request, ["allow", "remember", "deny"], { timeout: 300_000, signal: ctx.signal });
    if (answer === "remember") remembered.add(event.toolName);
    if (answer !== "allow" && answer !== "remember") return { block: true, reason: "用户拒绝", terminate: false };
  });
}
