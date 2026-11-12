import { useQuery, useQueryClient } from "@tanstack/react-query";
import type { ModelChoice } from "../../lib/api";
import { formatNotice } from "../../lib/notice";
import { aiMutationOptions } from "../../settings/aiQuery";
import { ChipBar } from "../../settings/chips";
import { useSettingsWrite } from "../../settings/editState";
import {
  ChatModelMenu, chatDefault, describeChoice, findEntry, modelName, OwnModelMenu, sameChoice, thinkingLabel, thinkingOptions, usePoolChoices,
} from "../../settings/modelChoices";
import { piConfigOptions, piQueryOptions } from "../../settings/piQuery";

/**
 * 输入区的模型按钮：模型与思考强度合在一处，只改这条对话之后的轮次（阶段 19-6）。`value` / `thinking` 为 null
 * 跟随设置里的默认；选回默认即清除。自有 Pi 配置下只列 Pi 的模型，思考默认由 Pi 配置决定。
 */
export function ChatModelChip({ value, thinking, onChange, onThinking, onManagePool, disabled }: {
  value: ModelChoice | null;
  thinking: string | null;
  onChange: (choice: ModelChoice | null, label: string) => void;
  onThinking: (level: string | null, label: string) => void;
  onManagePool: () => void;
  disabled?: boolean;
}) {
  const { pool, providers, bindings } = usePoolChoices();
  const own = !!bindings?.chat_uses_own_config;
  const config = useQuery({ ...piConfigOptions(), enabled: own });
  // 改名与「管理模型」的显示名称是同一写入；写回的池快照让按钮、列表与模型页一起刷新。
  const { run } = useSettingsWrite(aiMutationOptions(useQueryClient()));
  const rename = async (choice: ModelChoice, name: string) => {
    const outcome = await run({ type: "model", args: { ...choice, display_name: name } });
    return outcome.status === "busy" ? "正在保存别的模型设置，稍后再试。"
      : outcome.status === "failed" ? formatNotice("没有保存：", outcome.error) : null;
  };

  if (own) {
    const fallback = config.data?.own_model ?? "";
    const current = value?.provider_id === 0 ? value.model : fallback;
    return <OwnModelMenu value={current} ownModel={fallback} disabled={disabled || !config.data}
      chat={{ badge: thinking ? <Level level={thinking} /> : null,
        extra: <Thinking value={thinking} fallback={null} own model={current || "Pi 默认"} onPick={onThinking} /> }}
      onPick={(model) => onChange(model === fallback ? null : { provider_id: 0, model }, model || "Pi 默认")} />;
  }

  const fallback = chatDefault(bindings);
  const current = value && value.provider_id > 0 ? value : fallback;
  const entry = findEntry(pool, current);
  const reasoning = entry?.effective.reasoning !== "unsupported";
  const level = reasoning ? thinking ?? entry?.thinking ?? null : null;
  const pick = (choice: ModelChoice) => {
    const label = describeChoice(choice, pool, providers).title;
    onChange(sameChoice(choice, fallback) ? null : choice, label);
  };
  return <ChatModelMenu value={current} onPick={pick} providers={providers} pool={pool} bindings={bindings}
    disabled={disabled || !bindings} onManage={onManagePool} onRename={rename}
    chat={{ badge: level ? <Level level={level} /> : null,
      // 还没有可用模型时只给「管理模型」，不摆一个调了也用不上的思考档位。
      extra: !entry ? null : !reasoning
        ? <div className="chat-thinking"><p className="hint">{modelName(entry)} 不支持推理，对话时不思考。</p></div>
        : <Thinking value={thinking} fallback={entry.thinking ?? null} model={modelName(entry)} onPick={onThinking} /> }} />;
}

const Level = ({ level }: { level: string }) => <span className="chat-model-level">{thinkingLabel(level)}</span>;

/** 菜单里的思考档位。引擎握手报告过的档位只读启动预载的缓存，不为它发请求。选回模型默认档即跟随模型。 */
function Thinking({ value, fallback, own = false, model, onPick }: {
  value: string | null;
  /** 模型设置里的档位；自有 Pi 配置下未知。 */
  fallback: string | null;
  /** 自有 Pi 配置：默认档位由 Pi 配置决定。 */
  own?: boolean;
  model: string;
  onPick: (level: string | null, label: string) => void;
}) {
  const reported = useQuery({ ...piQueryOptions(), enabled: false }).data?.thinking_levels ?? [];
  const options = thinkingOptions(reported);
  const current = value ?? fallback ?? "";
  return (
    <div className="chat-thinking" role="group" aria-label="思考强度">
      <h5>思考</h5>
      <ChipBar className="segmented" value={current} options={options}
        onChange={(level) => onPick(level === fallback ? null : level, thinkingLabel(level))} />
      <p className="hint">
        {fallback ? `${model} 默认 ${thinkingLabel(fallback)}，这里改只影响这条对话。`
          : own ? "默认由你的 Pi 配置决定，这里改只影响这条对话。" : `${model} 没设默认档位时按模型自己的默认；这里改只影响这条对话。`}
        模型不支持的档位会自动换成最接近的一档。
      </p>
    </div>
  );
}
