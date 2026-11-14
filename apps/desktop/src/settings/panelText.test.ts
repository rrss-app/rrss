import { describe, expect, it } from "vitest";
import { i18n } from "../i18n";
import type { StorageMessage, RetentionMessage, BackupMessage, StorageGroupKind, StorageItemKind, StorageAction, StoragePassive } from "../bindings";
import { formatStorageMessage, formatRetentionMessage, formatBackupMessage, formatStorageBytes, formatPanelCount } from "./panelText";

const StorageMessageSamples = {
  "legacy_text": {"text": "preserved"},
  "storage_summary": {"database_bytes": 1234, "media_bytes": 1234, "engine_bytes": 1234, "backup_bytes": 1234, "log_bytes": 1234},
  "storage_articles": {"count": 1234},
  "storage_search_index": {},
  "storage_vectors_unset": {"count": 1234},
  "storage_vectors_other": {"count": 1234, "old_count": 1234},
  "storage_vectors_current": {"count": 1234},
  "storage_old_vectors_confirm": {"count": 1234},
  "storage_old_vectors_warning": {},
  "storage_ai_cache": {"count": 1234},
  "storage_ai_cache_confirm": {"count": 1234},
  "storage_ai_cache_warning": {},
  "storage_translations": {"count": 1234},
  "storage_translations_confirm": {"count": 1234},
  "storage_translations_warning": {},
  "storage_recommendation_logs": {"count": 1234},
  "storage_recommendation_logs_confirm": {"count": 1234},
  "storage_recommendation_logs_warning": {},
  "storage_other": {"chat_bytes": 1234, "threads": 1234, "other_bytes": 1234, "digest_bytes": 1234, "periods": 1234},
  "storage_legacy": {},
  "storage_legacy_confirm": {},
  "storage_legacy_warning": {},
  "storage_free": {"required_bytes": 1234, "available_bytes": null},
  "storage_vacuum_confirm": {"bytes": 1234},
  "storage_vacuum_space": {"required_bytes": 1234, "available_bytes": null},
  "storage_vacuum_wait": {},
  "storage_wal": {"bytes": 1234},
  "storage_images": {},
  "storage_images_confirm": {"bytes": 1234},
  "storage_images_warning": {},
  "storage_videos": {},
  "storage_videos_confirm": {"bytes": 1234},
  "storage_videos_warning": {},
  "storage_media_other": {},
  "storage_untracked": {"count": 1234},
  "storage_untracked_confirm": {"count": 1234, "bytes": 1234},
  "storage_untracked_warning": {},
  "storage_pi_program": {},
  "storage_pi_sessions": {},
  "storage_auto_backups": {"count": 1234, "keep": 1234},
  "storage_logs": {},
  "storage_logs_confirm": {},
  "storage_logs_warning": {},
  "storage_logs_location": {},
  "storage_cleaned": {"action": "vacuum", "count": 1234}
} satisfies { [K in StorageMessage["code"]]: Extract<StorageMessage, { code: K }>["params"] };

const RetentionMessageSamples = {
  "retention_preview": {"count": 1234},
  "retention_preview_empty": {},
  "retention_preview_hint": {"bytes": 1234, "first": false},
  "retention_daily": {},
  "retention_confirm": {"count": 1234, "days": 1234},
  "retention_preserve": {},
  "retention_backup": {},
  "retention_cleaned": {"count": 1234, "backed_up": false}
} satisfies { [K in RetentionMessage["code"]]: Extract<RetentionMessage, { code: K }>["params"] };

const BackupMessageSamples = {
  "backup_created": {"count": 1234, "feeds": 1234, "rules": 1234, "threads": 1234, "secrets_included": false},
  "backup_restore_summary": {"count": 1234, "feeds": 1234, "rules": 1234, "threads": 1234, "secrets_included": false},
  "prefs_rules": {"count": 1234, "duplicates": 1234},
  "prefs_packs": {"count": 1234},
  "prefs_interests": {"count": 1234, "duplicates": 1234},
  "prefs_memories": {"count": 1234, "duplicates": 1234},
  "prefs_empty": {}
} satisfies { [K in BackupMessage["code"]]: Extract<BackupMessage, { code: K }>["params"] };

describe("localized panel contracts", () => {
  it("has copy for every display message id and fills its parameters", () => {
    for (const [name, samples, format, ns, path] of [
      ["storage", StorageMessageSamples, (value: unknown) => formatStorageMessage(value as StorageMessage), "settingsSystem", "storage.messages"],
      ["retention", RetentionMessageSamples, (value: unknown) => formatRetentionMessage(value as RetentionMessage), "settingsContent", "retention.messages"],
      ["backup", BackupMessageSamples, (value: unknown) => formatBackupMessage(value as BackupMessage), "settingsSystem", "backup.messages"],
    ] as const) {
      for (const [code, params] of Object.entries(samples)) {
        const key = `${path}.${code}${"count" in params ? "_other" : ""}`;
        expect(i18n.exists(key, { ns }), `${name}:${code}`).toBe(true);
        const text = format({ code, params });
        expect(text).not.toContain(`${path}.`);
        expect(text).not.toContain("{{");
      }
    }
  });

  it("uses the active locale for binary sizes and grouped counts", async () => {
    const before = i18n.language;
    try {
      await i18n.changeLanguage("de");
      expect(formatStorageBytes(31 * 1024 ** 2 + 200_000)).toBe("31,2 MB");
      expect(formatPanelCount(1234)).toBe("1.234");
      expect(formatStorageBytes(1024 * 1024 + 1024 * 1024 / 4)).toBe("1,2 MB");
    } finally { await i18n.changeLanguage(before); }
  });
});

const displayIds = {
  groups: { database: true, media: true, engine: true, backups: true, logs: true } satisfies Record<StorageGroupKind, true>,
  items: { articles: true, search_index: true, vectors: true, ai_cache: true, translations: true, recommendation_logs: true, other: true, legacy: true, free: true, images: true, videos: true, media_other: true, untracked_media: true, pi_program: true, pi_sessions: true, auto_backups: true, logs: true } satisfies Record<StorageItemKind, true>,
  actions: { old_vectors: true, ai_cache: true, translations: true, recommendation_logs: true, legacy_tables: true, vacuum: true, images: true, videos: true, untracked_media: true, logs: true } satisfies Record<StorageAction, true>,
  passive: { follows_articles: true, manage_elsewhere: true, manage_backups: true, none: true, other_log_instance: true, log_unavailable: true } satisfies Record<StoragePassive, true>,
};

it("covers every panel label id and every cleanup result", () => {
  for (const [group, ids] of Object.entries(displayIds)) for (const id of Object.keys(ids)) {
    expect(i18n.exists(`storage.${group}.${id}`, { ns: "settingsSystem" }), `${group}:${id}`).toBe(true);
  }
  for (const action of Object.keys(displayIds.actions) as StorageAction[]) {
    expect(i18n.exists(`storage.confirmTitles.${action}`, { ns: "settingsSystem" }), action).toBe(true);
    for (const count of [0, 1234]) {
      const text = formatStorageMessage({ code: "storage_cleaned", params: { action, count } });
      expect(text).not.toContain("storage.results.");
      expect(text).not.toContain("{{");
    }
  }
});
