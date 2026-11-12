import { useState } from "react";
import { queryOptions, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "../lib/api";
import type { VectorSpaceStatusView } from "../bindings";
import { formatNotice } from "../lib/notice";
import { MenuSelect } from "../components/MenuSelect";
import { Feedback } from "./ConnectionDialog";
import { SettingRow } from "./chips";
import { SettingsRows } from "./SettingsRows";
import type { Ask } from "./SettingsPanel";

/** 阶段 23 V1c：向量库绑定、重建进度与探针核对。 */
export const vectorSpaceKey = ["vector-space"] as const;
export const vectorSpaceQueryOptions = () => queryOptions({
  queryKey: vectorSpaceKey,
  queryFn: api.vectorSpaceStatus,
  // 重建时轮询进度，其余时候只在打开页面和操作后读取。
  refetchInterval: (query) => (query.state.data?.building ? 3_000 : false),
});

const count = (n: number) => n.toLocaleString("zh-CN");
// 粗估：本机每秒约 30 段；远程只报 token 量，价格看服务商。
const effort = (s: VectorSpaceStatusView) => s.local
  ? `约 ${Math.max(1, Math.round(s.estimate_chunks / 30 / 60))} 分钟，不产生费用`
  : `约 ${count(s.estimate_tokens)} token，按服务商价格计费`;

/** 改选嵌入模型前的确认文案；返回 null 表示不能改（已绑定且未允许更换）。 */
export function embedChangeConfirm(status: VectorSpaceStatusView | undefined, label: string) {
  if (!status?.effective) {
    return { title: "绑定本库的向量", okLabel: "绑定并开始生成",
      message: `本库的向量将由 ${label} 生成，约 ${count(status?.estimate_chunks ?? 0)} 段，${status ? effort(status) : "耗时取决于文章数量"}。以后更换嵌入模型需要全量重建向量库。生成在后台进行，期间照常使用。` };
  }
  if (!status.allow_model_change) return null;
  return { title: "更换嵌入模型并重建", okLabel: "开始重建",
    message: `从 ${status.effective.model} 换到 ${label}，需要重新生成 ${count(status.estimate_chunks)} 段，${effort(status)}。重建完成前，相似文章和对话检索继续用 ${status.effective.model}；完成后自动切换，旧向量被替换。` };
}

export function VectorSpaceSection({ onAsk }: { onAsk: Ask }) {
  const client = useQueryClient();
  const status = useQuery(vectorSpaceQueryOptions());
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<{ tone: "ok" | "err"; text: string } | null>(null);
  const s = status.data;
  const space = s?.effective;
  const stale = space?.state === "stale";
  const sourceDims = space ? space.source_dims || space.dims : null;
  const dimsOptions = [
    { value: "auto", label: "自动（不超过 1024 维）" },
    ...[256, 512, 768, 1024, 1536, 2048]
      .filter(dims => sourceDims === null || dims <= sourceDims)
      .map(dims => ({ value: String(dims), label: `${dims} 维` })),
    { value: "native", label: sourceDims === null ? "原始" : `原始（${sourceDims} 维）` },
  ];
  const changingDims = !!(space && s?.building
    && s.building.space.model === space.model && s.building.space.dims !== space.dims);
  const dimsHint = "只有部分模型适合降维（如 OpenAI text-embedding-3、Gemini embedding、Qwen3-Embedding、EmbeddingGemma），其他模型降维后效果会明显变差。默认不超过 1024 维，节省空间。"
    + (stale ? "向量库已暂停生成，请先恢复原配置再改维度。" : s?.building ? "正在重建向量库，完成或取消后才能改维度。" : "");

  const act = async (run: () => Promise<VectorSpaceStatusView>, done: string) => {
    setBusy(true);
    setNotice(null);
    try {
      client.setQueryData(vectorSpaceKey, await run());
      setNotice({ tone: "ok", text: done });
    } catch (err) {
      setNotice({ tone: "err", text: formatNotice("没有完成：", err) });
    } finally {
      setBusy(false);
    }
  };

  const changeDims = (value: string) => {
    if (!s || busy || stale || s.building || value === s.dims_setting) return;
    const save = () => void act(() => api.setVectorDims(value), "已保存向量维度。");
    if (!space || sourceDims === null) { save(); return; }
    const target = value === "native" ? sourceDims : Math.min(value === "auto" ? 1024 : Number(value), sourceDims);
    if (target === space.dims) { save(); return; }
    const smaller = target < space.dims;
    onAsk("调整向量维度", smaller
      ? `将向量库从 ${space.dims} 维调整到 ${target} 维，直接在本地截断已有的 ${count(space.chunks)} 段向量，不调用接口、不产生费用；完成前检索继续用当前 ${space.dims} 维。`
      : `将向量库从 ${space.dims} 维调整到 ${target} 维，需要重新生成向量，约 ${count(s.estimate_chunks)} 段，${effort(s)}。重建完成前检索继续用当前 ${space.dims} 维。`,
    save, { okLabel: smaller ? "开始调整" : "开始重建" });
  };

  return (
    <section className="settings-group settings-section" id="vector-space" aria-labelledby="vector-space-title">
      <div className="settings-section-meta">
        <div className="settings-group-head"><h4 id="vector-space-title">向量库</h4></div>
        <p className="hint">向量只能和同一个模型生成的向量比较，所以本库绑定一个嵌入模型。</p>
      </div>
      <div className="settings-section-body">
        {status.isError && <p className="hint warn">{formatNotice("读取向量库状态失败：", status.error)}</p>}
        {s && (space ? (
          <p className="vector-space-bound"><span className="meta-tag accent">已绑定本库向量</span>
            {space.model}（{space.provider_name}）· {space.dims} 维{space.source_dims > space.dims ? `（原始 ${space.source_dims} 维）` : ""} · {count(space.chunks)} 段</p>
        ) : <p className="hint">还没有嵌入模型。没有它时，对话检索只用全文检索，排序不用向量。</p>)}
        {s?.building && (
          <div className="vector-rebuild" role="status">
            <strong>{changingDims ? `正在把向量库调整到 ${s.building.space.dims} 维` : `正在用 ${s.building.space.model} 重建向量库`}：{count(s.building.written)} / {count(s.building.total)} 段</strong>
            <button type="button" className="btn sm ghost" disabled={busy}
              onClick={() => onAsk("取消重建", changingDims ? `已生成的新向量会被删除，维度改回 ${space?.dims} 维。` : "已生成的新向量会被删除，嵌入改回原来的模型。", () => void act(api.cancelVectorRebuild, "已取消重建。"), { okLabel: "取消重建", cancelLabel: "继续重建" })}>取消重建</button>
            <progress className="engine-progress" max={Math.max(1, s.building.total)} value={s.building.written} aria-label="向量库重建进度" />
            <p className="hint">重建完成前，检索继续用 {changingDims ? `当前 ${space?.dims} 维` : space?.model ?? "原来的向量"}；完成后自动切换。</p>
          </div>
        )}
        {stale && (
          <div className="vector-rebuild" data-tone="danger" role="alert">
            <strong>已暂停生成新向量</strong>
            <button type="button" className="btn sm" disabled={busy}
              onClick={() => void act(api.restoreVectorSpace, "已恢复原配置并重新核对。")}>恢复原配置</button>
            <p className="hint">{space?.stale_reason || "服务返回的向量和建库时不同，可能是模型版本或量化变了。"}已有向量照常用于检索。</p>
          </div>
        )}
        {s && <SettingRow label="向量维度" hint={dimsHint}>
          <MenuSelect label="向量维度" className="menu-select vector-dims-select" value={s.dims_setting} options={dimsOptions}
            prefix={dimsOptions.some(option => option.value === s.dims_setting) ? undefined : `${s.dims_setting} 维`}
            disabled={busy || stale || !!s.building} onChange={changeDims} />
        </SettingRow>}
        <SettingsRows ids={["allow_embedding_model_change"]} label="向量库"
          onChanged={() => void client.invalidateQueries({ queryKey: vectorSpaceKey })} />
        {space && (
          <div className="settings-option">
            <div><strong>核对生成方式</strong>
              <p className="hint">定期检查服务生成的向量有没有变化；变了会暂停生成并提醒你。{space.checked_at ? `上次核对：${new Date(space.checked_at).toLocaleString("zh-CN", { hour12: false })}。` : ""}</p></div>
            <button type="button" className="btn sm ghost" disabled={busy}
              onClick={() => void act(api.checkVectorSpace, "核对完成。")}>立即核对</button>
          </div>
        )}
        {notice && <Feedback result={notice} />}
      </div>
    </section>
  );
}
