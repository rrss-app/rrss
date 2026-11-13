import { useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api, type BackupDoneView, type PrefsImportView, type RestoreCheckView } from "../lib/api";
import { i18n, useTranslation } from "../i18n";
import { formatNotice } from "../lib/notice";
import { backupKeys, backupQueryOptions } from "./backupQuery";
import { readGate, settingsChanged, useEditReport, useSettingsWrite, type SettingsEditState } from "./editState";
import { downloadText } from "./io";
import type { Ask } from "./SettingsPanel";

type Status = { tone: "ok" | "err"; text: string };
export type BackupOp = "" | "backup" | "inspect" | "restore" | "schedule" | "export" | "import" | "notice";
export type PrefsParts = { rules: boolean; interests: boolean; memories: boolean };

function restoreMessage(check: RestoreCheckView) {
  const t = (key: "checked" | "replace" | "autoBackup" | "restart", options: Record<string, string> = {}) => i18n.t(`backup.restoreMessage.${key}`, { ns: "settingsSystem", ...options });
  return [
    t("checked", { name: check.file_name }),
    check.taken,
    check.summary,
    "",
    t("replace"),
    t("autoBackup"),
    t("restart"),
  ].join("\n");
}

function today() {
  const d = new Date();
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

export function useBackupSettings(
  onAsk: Ask,
  onEditStateChange?: (state: SettingsEditState | null) => void,
  onChanged?: () => void,
) {
  const { t } = useTranslation("settingsSystem");
  const client = useQueryClient();
  const { writing, run: write } = useSettingsWrite({ mutationKey: backupKeys.write, mutationFn: (fn: () => Promise<unknown>) => fn() });
  const [op, setOp] = useState<BackupOp>("");
  const [restarting, setRestarting] = useState(false);
  const busy = writing || !!op || restarting;
  const query = useQuery({ ...backupQueryOptions(), ...readGate(busy) });
  const [status, setStatus] = useState<Status | null>(null);
  const [done, setDone] = useState<BackupDoneView | null>(null);
  const [imported, setImported] = useState<PrefsImportView | null>(null);
  const changed = useRef(onChanged);
  changed.current = onChanged;
  useEditReport(onEditStateChange, t("backup.report"), false, busy);

  /** 同一时间只跑一个操作；页面卸载后迟到的结果不再写状态。 */
  const run = async <T,>(next: BackupOp, failure: string, fn: () => Promise<T>, onDone: (value: T) => void) => {
    if (op || client.isMutating({ mutationKey: backupKeys.write })) return;
    setOp(next);
    setStatus(null);
    const result = await write(fn);
    setOp("");
    if (result.status === "busy" || !result.mounted) return;
    if (result.status === "done") onDone(result.data as T);
    else setStatus({ tone: "err", text: formatNotice(failure, result.error) });
  };

  const reload = async () => {
    await settingsChanged(client);
    await client.fetchQuery(backupQueryOptions()).catch(() => undefined);
  };

  const backup = (includeSecrets: boolean) => void run("backup", t("backup.err.backup"), () => api.backupCreate(includeSecrets), (view) => {
    if (!view) return;
    setDone(view);
    setImported(null);
    void reload();
  });

  // 宿主成功时会重启，页面停在「正在重启」；只有失败才回到可操作状态。
  const confirmRestore = (check: RestoreCheckView) => {
    setRestarting(true);
    void run("restore", t("backup.err.restore"), async () => {
      try { await api.backupRestore(check.path); }
      catch (error) { setRestarting(false); throw error; }
    }, () => undefined);
  };

  const restore = (path?: string) => void run("inspect", t("backup.err.inspect"), () => api.backupInspect(path), (check) => {
    if (!check) return;
    onAsk(t("backup.restore.title"), restoreMessage(check), () => confirmRestore(check), {
      okLabel: t("backup.restore.okLabel"),
      onCancel: () => { void api.backupCancelRestore().catch(() => undefined); },
    });
  });

  const setSchedule = (value: string) => void run("schedule", t("backup.err.schedule"), () => api.backupSetSchedule(value), () => {
    void reload();
  });

  const dismissNotice = () => void run("notice", t("backup.err.notice"), () => api.backupDismissNotice(), () => {
    void reload();
  });

  const reveal = (path: string) => {
    void api.backupReveal(path).catch((error) => setStatus({ tone: "err", text: formatNotice(t("backup.err.reveal"), error) }));
  };

  const exportPrefs = (parts: PrefsParts) => void run("export", t("backup.err.export"), () => api.prefsExport(parts), (text) => {
    downloadText(`rrss-prefs-${today()}.json`, text, "application/json");
    setStatus({ tone: "ok", text: t("backup.prefs.exported") });
  });

  const importPrefs = (file: File) => void run("import", t("backup.err.import"), async () => api.prefsImport(await file.text()), (report) => {
    setImported(report);
    setDone(null);
    void reload();
    changed.current?.();
  });

  return {
    query, busy, op, restarting, status, done, imported,
    backup, restore, setSchedule, dismissNotice, reveal, exportPrefs, importPrefs,
    closeDone: () => setDone(null), closeImported: () => setImported(null),
  };
}
