import type { BackupEntryView, BackupMessage, RetentionMessage, StorageItemView, StorageMessage } from "../bindings";
import { i18n, locale } from "../i18n";

export const formatPanelCount = (value: number, useGrouping = true) => new Intl.NumberFormat(locale(), { useGrouping, maximumFractionDigits: 0 }).format(value);

/** Match the existing binary-unit precision and Rust's ties-to-even rounding. */
export function formatStorageBytes(bytes: number, tiesToEven = true): string {
  const value = Math.max(0, bytes);
  const [amount, digits, unit] = value >= 1024 ** 3 ? [value / 1024 ** 3, 2, "GB"] as const
    : value >= 100 * 1024 ** 2 ? [value / 1024 ** 2, 0, "MB"] as const
    : value >= 1024 ** 2 ? [value / 1024 ** 2, 1, "MB"] as const
    : value >= 1024 ? [value / 1024, 0, "KB"] as const : [value, 0, "B"] as const;
  if (!value) return "0";
  const scale = 10 ** digits;
  const scaled = amount * scale;
  const floor = Math.floor(scaled);
  const rounded = tiesToEven ? scaled - floor === 0.5 ? floor + floor % 2 : Math.round(scaled) : Number(amount.toFixed(digits)) * scale;
  return `${new Intl.NumberFormat(locale(), { useGrouping: false, minimumFractionDigits: digits, maximumFractionDigits: digits }).format(rounded / scale)} ${unit}`;
}

const system = (key: string, replace: Record<string, unknown> = {}) => String(i18n.t(key as never, { ns: "settingsSystem", replace }));
const content = (key: string, replace: Record<string, unknown> = {}) => String(i18n.t(key as never, { ns: "settingsContent", replace }));
const render = (ns: "settingsSystem" | "settingsContent", key: string, raw: Record<string, unknown>, replace: Record<string, unknown>) => String(i18n.t(key as never, { ns, count: typeof raw.count === "number" ? raw.count : undefined, replace }));

export function formatStorageMessage(message: StorageMessage | null, item?: Pick<StorageItemView, "model" | "tables">): string {
  if (!message) return "";
  const raw = message.params;
  const values: Record<string, unknown> = Object.fromEntries(Object.entries(raw).map(([key, value]) => [key, typeof value === "number" ? key === "bytes" || key.endsWith("_bytes") ? formatStorageBytes(value) : formatPanelCount(value) : value]));
  values.model = item?.model ?? "";
  values.tables = item?.tables.join(system("storage.format.tableSeparator")) ?? "";
  if (message.code === "storage_summary") {
    values.backup = message.params.backup_bytes > 0 ? system("storage.format.backup", { bytes: formatStorageBytes(message.params.backup_bytes) }) : "";
    values.logs = message.params.log_bytes > 0 ? system("storage.format.logs", { bytes: formatStorageBytes(message.params.log_bytes) }) : "";
  }
  if (message.code === "storage_free" || message.code === "storage_vacuum_space") {
    values.available = message.params.available_bytes === null ? "" : system(message.code === "storage_free" ? "storage.format.available" : "storage.format.vacuumAvailable", { bytes: formatStorageBytes(message.params.available_bytes) });
  }
  if (message.code === "storage_cleaned") values.result = render("settingsSystem", `storage.results.${message.params.action}`, raw, values);
  return render("settingsSystem", `storage.messages.${message.code}`, raw, values);
}

export function storageConfirmTitle(action: NonNullable<StorageItemView["action"]>): string {
  return system(`storage.confirmTitles.${action}`);
}

export function formatRetentionMessage(message: RetentionMessage | null): string {
  if (!message) return "";
  const raw = message.params;
  const values: Record<string, unknown> = { ...raw };
  if ("count" in raw) values.count = formatPanelCount(raw.count);
  if (message.code === "retention_preview_hint") {
    values.bytes = formatStorageBytes(message.params.bytes);
    values.first = message.params.first ? content("retention.format.first") : "";
  }
  if (message.code === "retention_confirm") {
    values.scope = message.params.days > 0 ? content("retention.format.scope", { days: formatPanelCount(message.params.days, false) }) : content("retention.format.customScope");
  }
  if (message.code === "retention_cleaned") values.backup = message.params.backed_up ? content("retention.format.backedUp") : "";
  return render("settingsContent", `retention.messages.${message.code}`, raw, values);
}

export function formatBackupWhen(ms: number, now = Date.now()): string {
  const date = new Date(ms);
  if (!Number.isFinite(date.getTime())) return "";
  const today = new Date(now);
  const yesterday = new Date(now);
  yesterday.setDate(yesterday.getDate() - 1);
  const sameDay = (other: Date) => date.getFullYear() === other.getFullYear() && date.getMonth() === other.getMonth() && date.getDate() === other.getDate();
  const clock = new Intl.DateTimeFormat(locale(), { hour: "2-digit", minute: "2-digit", hourCycle: "h23" }).format(date);
  if (sameDay(today)) return system("backup.format.whenToday", { clock });
  if (sameDay(yesterday)) return system("backup.format.whenYesterday", { clock });
  const parts = Object.fromEntries(new Intl.DateTimeFormat(locale(), { year: "numeric", month: "numeric", day: "numeric" }).formatToParts(date).map(part => [part.type, part.value]));
  return system(date.getFullYear() === today.getFullYear() ? "backup.format.whenMonth" : "backup.format.whenYear", { ...parts, clock });
}

export function formatBackupEntry(entry: BackupEntryView): string {
  const bytes = formatStorageBytes(entry.bytes);
  if (entry.kind === "unreadable") return system("backup.format.unreadableEntry", { bytes });
  return system("backup.format.entry", { kind: system(`backup.kinds.${entry.kind}`, { version: formatPanelCount(entry.schema_version, false) }), bytes, secrets: system(entry.secrets_included ? "backup.format.secrets" : "backup.format.noSecrets") });
}

export function formatBackupMessage(message: BackupMessage): string {
  const raw = message.params;
  const values: Record<string, unknown> = { ...raw };
  if (message.code === "backup_created" || message.code === "backup_restore_summary") {
    const counts = Object.fromEntries(Object.entries(raw).map(([key, value]) => [key, typeof value === "number" ? formatPanelCount(value) : value]));
    const secrets = message.params.secrets_included ? "backup.format.secrets" : message.code === "backup_created" ? "backup.format.createdNoSecrets" : "backup.format.restoreNoSecrets";
    values.summary = system("backup.format.summary", { counts: system("backup.format.counts", counts), secrets: system(secrets) });
  }
  if ("count" in raw) values.count = formatPanelCount(raw.count, !message.code.startsWith("prefs_"));
  if ("duplicates" in raw) {
    const kind = message.code.replace("prefs_", "");
    values.duplicates = raw.duplicates ? render("settingsSystem", `backup.duplicates.${kind}`, { count: raw.duplicates }, { count: formatPanelCount(raw.duplicates, false) }) : "";
  }
  return render("settingsSystem", `backup.messages.${message.code}`, raw, values);
}
