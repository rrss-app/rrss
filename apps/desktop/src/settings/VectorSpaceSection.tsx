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
import { i18n, locale, useTranslation } from "../i18n";

/** 阶段 23 V1c：向量库绑定、重建进度与探针核对。 */
export const vectorSpaceKey = ["vector-space"] as const;
export const vectorSpaceQueryOptions = () => queryOptions({
  queryKey: vectorSpaceKey,
  queryFn: api.vectorSpaceStatus,
  // 重建时轮询进度，其余时候只在打开页面和操作后读取。
  refetchInterval: (query) => (query.state.data?.building ? 3_000 : false),
});

const count = (n: number) => n.toLocaleString(locale());
// 粗估：本机每秒约 30 段；远程只报 token 量，价格看服务商。
const effort = (s: VectorSpaceStatusView) => s.local
  ? i18n.t("vector.effortLocal", { ns: "settingsJobs", minutes: Math.max(1, Math.round(s.estimate_chunks / 30 / 60)) })
  : i18n.t("vector.effortRemote", { ns: "settingsJobs", tokens: count(s.estimate_tokens) });

/** 改选嵌入模型前的确认文案；返回 null 表示不能改（已绑定且未允许更换）。 */
export function embedChangeConfirm(status: VectorSpaceStatusView | undefined, label: string) {
  if (!status?.effective) {
    return { title: i18n.t("vector.bind.title", { ns: "settingsJobs" }), okLabel: i18n.t("vector.bind.ok", { ns: "settingsJobs" }),
      message: i18n.t("vector.bind.message", { ns: "settingsJobs", label, chunks: count(status?.estimate_chunks ?? 0),
        effort: status ? effort(status) : i18n.t("vector.bind.effortUnknown", { ns: "settingsJobs" }) }) };
  }
  if (!status.allow_model_change) return null;
  return { title: i18n.t("vector.change.title", { ns: "settingsJobs" }), okLabel: i18n.t("vector.change.ok", { ns: "settingsJobs" }),
    message: i18n.t("vector.change.message", { ns: "settingsJobs", from: status.effective.model, label,
      chunks: count(status.estimate_chunks), effort: effort(status) }) };
}

export function VectorSpaceSection({ onAsk }: { onAsk: Ask }) {
  const { t } = useTranslation("settingsJobs");
  const client = useQueryClient();
  const status = useQuery(vectorSpaceQueryOptions());
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<{ tone: "ok" | "err"; text: string } | null>(null);
  const s = status.data;
  const space = s?.effective;
  const stale = space?.state === "stale";
  const sourceDims = space ? space.source_dims || space.dims : null;
  const dimsOptions = [
    { value: "auto", label: t("vector.dims.auto") },
    ...[256, 512, 768, 1024, 1536, 2048]
      .filter(dims => sourceDims === null || dims <= sourceDims)
      .map(dims => ({ value: String(dims), label: t("vector.dims.n", { dims }) })),
    { value: "native", label: sourceDims === null ? t("vector.dims.native") : t("vector.dims.nativeN", { dims: sourceDims }) },
  ];
  const changingDims = !!(space && s?.building
    && s.building.space.model === space.model && s.building.space.dims !== space.dims);
  const dimsHint = t("vector.dims.hint")
    + (stale ? t("vector.dims.hintStale") : s?.building ? t("vector.dims.hintBuilding") : "");

  const act = async (run: () => Promise<VectorSpaceStatusView>, done: string) => {
    setBusy(true);
    setNotice(null);
    try {
      client.setQueryData(vectorSpaceKey, await run());
      setNotice({ tone: "ok", text: done });
    } catch (err) {
      setNotice({ tone: "err", text: formatNotice(t("vector.notice.failed"), err) });
    } finally {
      setBusy(false);
    }
  };

  const changeDims = (value: string) => {
    if (!s || busy || stale || s.building || value === s.dims_setting) return;
    const save = () => void act(() => api.setVectorDims(value), t("vector.notice.dimsSaved"));
    if (!space || sourceDims === null) { save(); return; }
    const target = value === "native" ? sourceDims : Math.min(value === "auto" ? 1024 : Number(value), sourceDims);
    if (target === space.dims) { save(); return; }
    const smaller = target < space.dims;
    onAsk(t("vector.resize.title"), smaller
      ? t("vector.resize.truncate", { from: space.dims, to: target, chunks: count(space.chunks) })
      : t("vector.resize.rebuild", { from: space.dims, to: target, chunks: count(s.estimate_chunks), effort: effort(s) }),
    save, { okLabel: smaller ? t("vector.resize.okAdjust") : t("vector.resize.okRebuild") });
  };

  return (
    <section className="settings-group settings-section" id="vector-space" aria-labelledby="vector-space-title">
      <div className="settings-section-meta">
        <div className="settings-group-head"><h4 id="vector-space-title">{t("vector.title")}</h4></div>
        <p className="hint">{t("vector.intro")}</p>
      </div>
      <div className="settings-section-body">
        {status.isError && <p className="hint warn">{formatNotice(t("vector.notice.readFailed"), status.error)}</p>}
        {s && (space ? (
          <p className="vector-space-bound"><span className="meta-tag accent">{t("vector.bound.tag")}</span>
            {space.source_dims > space.dims
              ? t("vector.bound.summaryNative", { model: space.model, provider: space.provider_name, dims: space.dims, sourceDims: space.source_dims, chunks: count(space.chunks) })
              : t("vector.bound.summary", { model: space.model, provider: space.provider_name, dims: space.dims, chunks: count(space.chunks) })}</p>
        ) : <p className="hint">{t("vector.unbound")}</p>)}
        {s?.building && (
          <div className="vector-rebuild" role="status">
            <strong>{changingDims
              ? t("vector.building.resizing", { dims: s.building.space.dims, written: count(s.building.written), total: count(s.building.total) })
              : t("vector.building.rebuilding", { model: s.building.space.model, written: count(s.building.written), total: count(s.building.total) })}</strong>
            <button type="button" className="btn sm ghost" disabled={busy}
              onClick={() => onAsk(t("vector.cancel.title"), changingDims ? t("vector.cancel.messageDims", { dims: space?.dims }) : t("vector.cancel.messageModel"), () => void act(api.cancelVectorRebuild, t("vector.notice.cancelled")), { okLabel: t("vector.cancel.ok"), cancelLabel: t("vector.cancel.keep") })}>{t("vector.cancel.button")}</button>
            <progress className="engine-progress" max={Math.max(1, s.building.total)} value={s.building.written} aria-label={t("vector.building.progressAria")} />
            <p className="hint">{changingDims
              ? t("vector.building.keepDims", { dims: space?.dims })
              : t("vector.building.keepModel", { model: space?.model ?? t("vector.building.previousVectors") })}</p>
          </div>
        )}
        {stale && (
          <div className="vector-rebuild" data-tone="danger" role="alert">
            <strong>{t("vector.stale.title")}</strong>
            <button type="button" className="btn sm" disabled={busy}
              onClick={() => void act(api.restoreVectorSpace, t("vector.notice.restored"))}>{t("vector.stale.restore")}</button>
            <p className="hint">{space?.stale_reason || t("vector.stale.defaultReason")}{t("vector.stale.existingUsable")}</p>
          </div>
        )}
        {s && <SettingRow label={t("vector.dims.label")} hint={dimsHint}>
          <MenuSelect label={t("vector.dims.label")} className="menu-select vector-dims-select" value={s.dims_setting} options={dimsOptions}
            prefix={dimsOptions.some(option => option.value === s.dims_setting) ? undefined : t("vector.dims.n", { dims: s.dims_setting })}
            disabled={busy || stale || !!s.building} onChange={changeDims} />
        </SettingRow>}
        <SettingsRows ids={["allow_embedding_model_change"]} label={t("vector.title")}
          onChanged={() => void client.invalidateQueries({ queryKey: vectorSpaceKey })} />
        {space && (
          <div className="settings-option">
            <div><strong>{t("vector.check.title")}</strong>
              <p className="hint">{t("vector.check.hint")}{space.checked_at ? t("vector.check.last", { time: new Date(space.checked_at).toLocaleString(locale(), { hour12: false }) }) : ""}</p></div>
            <button type="button" className="btn sm ghost" disabled={busy}
              onClick={() => void act(api.checkVectorSpace, t("vector.notice.checked"))}>{t("vector.check.button")}</button>
          </div>
        )}
        {notice && <Feedback result={notice} />}
      </div>
    </section>
  );
}
