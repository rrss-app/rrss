import { useEffect, useState, type ReactNode } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import type { SettingSpec } from "../lib/api";
import { formatNotice } from "../lib/notice";
import { Stepper } from "./Stepper";
import { ChipBar, SaveBar, SettingRow, Toggle } from "./chips";
import { useEditReport, useSettingsWrite, type SettingsEditState } from "./editState";
import { formatMinutes, parseMinutes, registryEntry, type SettingId } from "./registry";
import { settingsKeys, settingsMutationOptions, settingsSchemaOptions } from "./settingsQuery";
import { tokensToWanText, wanTextToTokens } from "./autoSummary";
import { Pending } from "../components/Pending";

type Status = { tone: "ok" | "err"; text: string };
type Row = { spec: SettingSpec; value: string; label: string; hint?: string; control: string };

/**
 * 描述符决定控件与保存方式；草稿留在行组，写入和权威快照由查询层共享。
 * 有草稿时出现保存栏，离开时由设置壳的 dirty / busy 守卫拦截。
 */
export function SettingsRows({ ids, label, title, className, disabled = false, only, notes, onEditStateChange, onChanged }: {
  ids: SettingId[];
  /** 接在某行说明后的几行（估算、状态等随数据变化的内容）。 */
  notes?: Partial<Record<SettingId, ReactNode>>;
  /** 只显示枚举里的这些选项（例如对话引擎实际支持的思考档位）；缺省或为空时全部显示。 */
  only?: Partial<Record<SettingId, string[]>>;
  /** 离开守卫里显示的名称。 */
  label: string;
  /** 有标题时画成一张分区卡片。 */
  title?: string;
  className?: string;
  /** 页面别处正在写入时暂停这里的写入。 */
  disabled?: boolean;
  onEditStateChange?: (state: SettingsEditState | null) => void;
  onChanged?: () => void;
}) {
  const client = useQueryClient();
  const query = useQuery(settingsSchemaOptions());
  const [draft, setDraft] = useState<Record<string, string>>({});
  const [optimistic, setOptimistic] = useState<Record<string, string>>({});
  const { writing, run } = useSettingsWrite(settingsMutationOptions(client));
  const [status, setStatus] = useState<Status | null>(null);

  const rows: Row[] = ids.flatMap((id) => {
    const spec = query.data?.specs.find((s) => s.id === id);
    const entry = registryEntry(id);
    if (!spec || !entry) return [];
    const saved = query.data?.values.find((v) => v.id === id)?.value ?? spec.default;
    return [{ spec, value: draft[id] ?? optimistic[id] ?? saved, label: entry.label, hint: entry.hint, control: entry.control ?? entry.label }];
  });
  const saved = (id: string) => query.data?.values.find((v) => v.id === id)?.value;
  // 即时生效的数字在输入期间也暂存草稿，离开输入框才提交；它们不算「未保存的修改」。
  const dirty = Object.entries(draft).some(([id, value]) => value !== saved(id) && query.data?.specs.find((s) => s.id === id)?.apply === "draft");
  const blocked = disabled || writing;

  const submit = async (pending: Array<{ row: Row; value: string }>): Promise<boolean> => {
    if (disabled || !pending.length) return !pending.length;
    for (const { row, value } of pending) {
      const problem = checkDraft(row.spec, value);
      if (problem) {
        setStatus({ tone: "err", text: `「${row.label}」${problem}` });
        return false;
      }
    }
    setStatus(null);
    setOptimistic(Object.fromEntries(pending.map(({ row, value }) => [row.spec.id, value])));
    const outcome = await run(pending.map(({ row, value }) => ({ id: row.spec.id, value: value.trim() })));
    if (outcome.status !== "busy" && !outcome.mounted) return outcome.status === "done" && !outcome.data.failure;
    setOptimistic({});
    if (outcome.status === "busy") return false;
    if (outcome.status === "failed") {
      setStatus({ tone: "err", text: formatNotice("保存失败：", outcome.error) });
      return false;
    }
    const result = outcome.data;
    setDraft(old => Object.fromEntries(Object.entries(old).filter(([id]) => !result.saved.some(s => s.id === id))));
    if (result.failure) {
      const row = pending.find(p => p.row.spec.id === result.failure!.id)!.row;
      setStatus({ tone: "err", text: formatNotice(`没能保存「${row.label}」：`, result.failure.error) });
      return false;
    } else {
      onChanged?.();
      const affectsPi = pending.some(p => p.row.spec.affects_pi);
      if (affectsPi || pending.some(p => p.row.spec.apply === "draft")) {
        setStatus({ tone: "ok", text: affectsPi ? "已保存，将在下次发送时生效。" : "已保存。" });
      }
      return true;
    }
  };

  const change = (row: Row, value: string) => {
    if (blocked || client.isMutating({ mutationKey: settingsKeys.write })) return;
    if (row.spec.apply === "draft") {
      setStatus(null);
      setDraft(old => ({ ...old, [row.spec.id]: value }));
    } else if (value !== row.value) {
      void submit([{ row, value }]);
    }
  };
  const save = () => submit(rows
    .filter(r => draft[r.spec.id] !== undefined && draft[r.spec.id] !== saved(r.spec.id))
    .map(row => ({ row, value: draft[row.spec.id] })));
  useEditReport(onEditStateChange, label, dirty, writing, save);

  const control = (row: Row) => {
    const { spec, value } = row;
    const kind = spec.kind;
    if (kind.type === "bool") {
      const entry = registryEntry(spec.id);
      return <Toggle label={row.control} on={value === "1"} onLabel={entry?.on ?? "已开启"} offLabel={entry?.off ?? "已关闭"}
        disabled={blocked} onClick={() => void change(row, value === "1" ? "0" : "1")} />;
    }
    const display = registryEntry(spec.id)?.display;
    if (kind.type === "enum") {
      const allowed = only?.[spec.id as SettingId];
      const labels = registryEntry(spec.id)?.options;
      const listed = labels ? kind.options.map((o) => ({ ...o, label: labels[o.id] ?? o.label })) : kind.options;
      const options = allowed?.length ? listed.filter((o) => allowed.includes(o.id) || o.id === value) : listed;
      return <ChipBar className="segmented" value={value} options={options} disabled={blocked}
        onChange={(next) => void change(row, next)} />;
    }
    if (display === "multiline") {
      return <div className="settings-note-editor">
        <textarea value={draft[spec.id] ?? value} aria-label={row.control} disabled={blocked} rows={4}
          maxLength={kind.type === "text" ? kind.max : undefined} onChange={(e) => change(row, e.target.value)} />
        <div className="settings-note-actions"><span className="hint">{value.length}{kind.type === "text" ? ` / ${kind.max}` : ""}</span>
          {spec.level !== "l2" && <button type="button" className="link-btn" disabled={blocked || value === spec.default} onClick={() => change(row, spec.default)}>恢复默认</button>}</div>
      </div>;
    }
    if (display === "minutes") {
      // 库里是秒；草稿里存秒，输入框按分钟显示。填了非数字就原样留在草稿里，交给预检报错。
      const raw = draft[spec.id] ?? value;
      const shown = raw.trim() !== "" && Number.isFinite(Number(raw)) ? String(Number(raw) / 60) : raw;
      return <Stepper value={shown} label={row.control} disabled={blocked} min={kind.type === "int" ? kind.min / 60 : undefined} max={kind.type === "int" ? kind.max / 60 : undefined}
        onChange={(next) => {
          const text = next.trim();
          change(row, text !== "" && Number.isFinite(Number(text)) ? String(Math.round(Number(text) * 60)) : text);
        }} />;
    }
    if (display === "wan") {
      return <WanInput label={row.control} tokens={draft[spec.id] ?? value} disabled={blocked} onChange={(next) => change(row, next)} />;
    }
    if (display === "time") {
      return <input type="time" aria-label={row.control} value={formatMinutes(Number(value))} disabled={blocked}
        onChange={(e) => {
          const minutes = parseMinutes(e.target.value);
          if (minutes != null) void change(row, String(minutes));
        }} />;
    }
    if (kind.type === "int") return <Stepper value={draft[spec.id] ?? value} label={row.control} disabled={blocked} min={kind.min} max={kind.max}
      onChange={(next) => spec.apply === "draft" ? change(row, next) : setDraft(old => ({ ...old, [spec.id]: next }))}
      onCommit={spec.apply === "draft" ? undefined : (next) => { if (next !== saved(spec.id)) void submit([{ row, value: next }]); }} />;
    return <input value={draft[spec.id] ?? value} aria-label={row.control} disabled={blocked}
      placeholder={spec.default}
      onChange={(e) => change(row, e.target.value)} />;
  };

  const hint = (row: Row) => {
    const kind = row.spec.kind;
    const chosen = kind.type === "enum" ? kind.options.find((o) => o.id === row.value)?.hint ?? "" : "";
    return [chosen, row.hint].filter(Boolean).join("") || undefined;
  };

  const body = (
    <>
      {title ? <h4>{title}</h4> : null}
      {!query.data && !query.isError && <Pending>正在读取设置…</Pending>}
      {query.isError && (
        <p className="notice error" role="alert">
          {formatNotice("读取设置失败：", query.error)}
          <button type="button" className="btn sm" onClick={() => void query.refetch()}>重新读取</button>
        </p>
      )}
      {rows.map((row) => (
        <SettingRow key={row.spec.id} label={row.label} hint={hint(row)} note={notes?.[row.spec.id as SettingId]}>{control(row)}</SettingRow>
      ))}
      {status && <p className={status.tone === "err" ? "form-status error" : "form-status"} role={status.tone === "err" ? "alert" : "status"}>{status.text}</p>}
      {dirty && <SaveBar label={label} busy={blocked} saving={writing} onSave={() => void save()} onUndo={() => { setDraft({}); setStatus(null); }} />}
    </>
  );
  const classes = ["settings-rows", title ? "settings-card" : "", className ?? ""].filter(Boolean).join(" ");
  return title ? <section className={classes}>{body}</section> : <div className={classes}>{body}</div>;
}

/**
 * 按「万 token / 天」填写的 token 数（两份日预算）：输入框自己留着正在敲的文字（「1.」这类半截小数），
 * 只把换算成 token 的值交出去；外部值变了（保存、撤销）且和输入框对不上时再同步回来。
 */
function WanInput({ label, tokens, disabled, onChange }: { label: string; tokens: string; disabled: boolean; onChange: (tokens: string) => void }) {
  const [text, setText] = useState(() => tokensToWanText(tokens));
  useEffect(() => {
    setText((old) => (wanTextToTokens(old) === tokens ? old : tokensToWanText(tokens)));
  }, [tokens]);
  return <span className="settings-amount">
    <input type="text" inputMode="decimal" aria-label={label} value={text} disabled={disabled}
      onChange={(e) => { setText(e.target.value); onChange(wanTextToTokens(e.target.value)); }} />
    <span className="settings-amount-unit">万 token / 天</span>
  </span>;
}

/** 草稿的前端预检；后端仍按描述符再校验一次。 */
export function checkDraft(spec: SettingSpec, raw: string): string | null {
  const text = raw.trim();
  if (spec.kind.type === "int") {
    const n = Number(text);
    if (!text || !Number.isSafeInteger(n)) return "需要填写整数。";
    if (n < spec.kind.min || n > spec.kind.max) return `需要在 ${spec.kind.min} 到 ${spec.kind.max} 之间。`;
  }
  if (spec.kind.type === "text" && text.length > spec.kind.max) return `不能超过 ${spec.kind.max} 字。`;
  return null;
}
