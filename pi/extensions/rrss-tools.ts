import { installMcpGuard } from "./mcp-guard.ts";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { readFileSync } from "node:fs";

type Param = { name: string; kind: string; description: string; optional?: boolean };
type ToolSpec = {
  name: string;
  label: string;
  description: string;
  confirm?: boolean;
  parameters?: Param[];
};

function bin(): string {
  return process.env.RRSS_TOOL || "rrss";
}

function loadManifest(): ToolSpec[] {
  const path = process.env.RRSS_TOOL_MANIFEST;
  if (!path) {
    return [];
  }
  try {
    const raw = JSON.parse(readFileSync(path, "utf8"));
    return Array.isArray(raw.tools) ? raw.tools : [];
  } catch (err) {
    console.error("rrss-tools: failed to read manifest", err);
    return [];
  }
}

function buildSchema(params: Param[] = []) {
  const props: Record<string, ReturnType<typeof Type.String> | ReturnType<typeof Type.Number>> = {};
  for (const p of params) {
    const inner = p.kind === "number"
      ? Type.Number({ description: p.description })
      : Type.String({ description: p.description });
    props[p.name] = p.optional ? Type.Optional(inner) : inner;
  }
  return Type.Object(props);
}

function flags(spec: ToolSpec, params: Record<string, unknown>): string[] {
  const out: string[] = ["tool", spec.name];
  for (const p of spec.parameters || []) {
    const v = params[p.name];
    if (v === undefined || v === null) {
      continue;
    }
    out.push(`--${p.name.replaceAll("_", "-")}`, String(v));
  }
  return out;
}

async function runTool(pi: ExtensionAPI, args: string[], signal?: AbortSignal) {
  const result = await pi.exec(bin(), args, { signal });
  const text = (result.stdout || result.stderr || "").trim();
  if (result.code !== 0) {
    throw new Error(text || `exit ${result.code}`);
  }
  return {
    content: [{ type: "text" as const, text: text || "{}" }],
    details: {},
  };
}

/** 确认请求的期限；与阶段 18「ChatTurn」一致。 */
const CONFIRM_TIMEOUT_MS = 120_000;

/** 系统提示里长期记忆小节的标签；内容每轮更新，不写进用户消息。 */
const MEMORY_SECTION = "rrss_memory";

async function memoryPreview(pi: ExtensionAPI): Promise<string> {
  try {
    const result = await pi.exec(bin(), ["tool", "memory_preview"]);
    return result.code === 0 ? (result.stdout || "").trim() : "";
  } catch (err) {
    console.error("rrss-tools: memory preview failed", err);
    return "";
  }
}

export default function (pi: ExtensionAPI) {
  installMcpGuard(pi);
  // 每轮把最新的长期记忆写进系统提示的独立小节（阶段 18 D10）。改小节而不是整段替换，
  // Pi 只补发变化的小节，提示缓存前缀得以保留。没有 sections 的旧版 Pi 退回追加到提示末尾。
  pi.on("before_agent_start", async (event) => {
    const memory = await memoryPreview(pi);
    const body = memory ? `以下是用户的长期记忆，回答时参考：\n${memory}` : "";
    const sections = (event as { systemPromptOptions?: { sections?: Record<string, string> } })
      .systemPromptOptions?.sections;
    if (sections) {
      if (body) {
        sections[MEMORY_SECTION] = body;
      } else {
        delete sections[MEMORY_SECTION];
      }
      return;
    }
    if (body) {
      return { systemPrompt: `${event.systemPrompt}\n\n<${MEMORY_SECTION}>\n${body}\n</${MEMORY_SECTION}>` };
    }
  });

  const manifest = loadManifest();
  for (const t of manifest) {
    pi.registerTool({
      name: t.name,
      label: t.label,
      description: t.description,
      parameters: buildSchema(t.parameters),
      async execute(_id, params, signal, _onUpdate, ctx) {
        if (t.confirm) {
          // 超时由 Pi 按拒绝处理，宿主不用计时，也不会因为没人回应卡住整轮。
          const ok = await ctx.ui.confirm(t.label, `执行 ${t.name}？`, { timeout: CONFIRM_TIMEOUT_MS });
          if (!ok) {
            return {
              content: [{ type: "text" as const, text: `用户取消了 ${t.name}。` }],
              details: {},
            };
          }
        }
        const args = flags(t, params as Record<string, unknown>);
        if (t.name === "get_assistant_config" && ctx.model?.name) {
          // Pass only the alias and thinking level, never the model/provider object.
          args.push("--model-alias", ctx.model.name, "--thinking", pi.getThinkingLevel());
        }
        return runTool(pi, args, signal);
      },
    });
  }
}
