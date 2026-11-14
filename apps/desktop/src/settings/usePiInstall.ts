import { useCallback, useEffect, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { api, onPiInstall, type PiInstallView } from "../lib/api";
import { formatError } from "../lib/errors";
import { formatNotice } from "../lib/notice";
import { piKeys } from "./piQuery";
import { i18n } from "../i18n";

export const piInstalling = (view: PiInstallView | null | undefined) =>
  view?.state === "downloading" || view?.state === "installing";

/**
 * 对话引擎的后台下载：不走设置写入互斥，下载中可以离开设置页、改别的设置。
 * 进入页面先取一次当前进度（可能是启动时的自动下载），之后跟 `pi:install` 事件。
 */
export function usePiInstall() {
  const [view, setView] = useState<PiInstallView | null>(null);
  useEffect(() => {
    let alive = true;
    api.piInstallStatus().then((current) => { if (alive) setView((seen) => seen ?? current); }, () => {});
    // 订阅失败（宿主不支持事件）时只是看不到进度，不打断页面。
    const unlisten = onPiInstall(setView).catch(() => () => {});
    return () => { alive = false; void unlisten.then((stop) => stop()); };
  }, []);
  const start = useCallback(async () => {
    try { setView(await api.installPi()); }
    catch (err) { setView({ state: "failed", received: 0, total: null, message: formatNotice("", err), dir: "" }); }
  }, []);
  return { view, start };
}

/** 根壳：下载在后台结束时提示一句并让引擎探测过期，用户可能已经离开设置页。 */
export function usePiInstallNotice(notify: (notice: { message: string }) => void) {
  const client = useQueryClient();
  useEffect(() => {
    const unlisten = onPiInstall((view) => {
      if (view.state !== "done" && view.state !== "failed") return;
      void client.invalidateQueries({ queryKey: piKeys.probe });
      notify({ message: i18n.t(view.state === "done" ? "install.done" : "install.failed", { ns: "settingsAssistant", message: view.notice ? formatError(view.notice) : view.error ? formatError(view.error) : view.message }) });
    }).catch(() => () => {});
    return () => { void unlisten.then((stop) => stop()); };
  }, [client, notify]);
}

const mb = (bytes: number) => `${(bytes / 1024 / 1024).toFixed(1)} MB`;

/** 进度一句话：有总大小时给百分比，没有时只报已下载。 */
export function installProgressText(view: PiInstallView) {
  if (view.state === "installing") return i18n.t("install.installing", { ns: "settingsAssistant" });
  if (!view.received) return i18n.t("install.connecting", { ns: "settingsAssistant" });
  return view.total
    ? i18n.t("install.downloading", { ns: "settingsAssistant", received: mb(view.received), total: mb(view.total) })
    : i18n.t("install.downloadingNoTotal", { ns: "settingsAssistant", received: mb(view.received) });
}
