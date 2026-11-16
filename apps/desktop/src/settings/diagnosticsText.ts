import type { DiagnosticCount, DiagnosticsPage } from "../lib/api";
import { i18n, locale } from "../i18n";

export type DiagRow = "db" | "vec" | "stats" | "ai";

type ValueKey = "db" | "vecUnavailable" | "vectors" | "interests" | "models" | "dims" | "chunkError" | "scheduler" | "modelQueue" | "ai";
const t = (key: ValueKey, options: Record<string, string | number>) => i18n.t(`diag.value.${key}`, { ns: "settingsSystem", ...options });
const num = (value: number) => value.toLocaleString(locale());
const megabytes = (bytes: number) =>
  new Intl.NumberFormat(locale(), { style: "unit", unit: "megabyte", maximumFractionDigits: 1 }).format(bytes / 1_048_576);
const counts = (items: DiagnosticCount[], label: (value: string) => string = (value) => value) =>
  items.length ? items.map((item) => `${label(item.label)} × ${num(item.count)}`).join(", ") : "—";

/** 诊断页每一行的文字（可能多行）。后端只给数与名，这里按界面语言拼句。 */
export function diagnosticsLines(page: DiagnosticsPage): Record<DiagRow, string[]> {
  const { db, vec, vectors, ai, scheduler } = page;
  const stats: string[] = [];
  if (vectors) {
    stats.push(t("vectors", {
      embedded: num(vectors.chunks_embedded), total: num(vectors.chunks_total),
      pendingChunks: num(vectors.chunks_pending), pendingArticles: num(vectors.articles_pending),
    }));
    stats.push(t("interests", {
      embedded: num(vectors.interests_embedded), total: num(vectors.interests_total), memories: num(vectors.memories_embedded),
    }));
    stats.push(t("models", { dims: counts(vectors.dims, (dims) => t("dims", { dims })), models: counts(vectors.models) }));
    for (const error of vectors.recent_errors) stats.push(t("chunkError", { chunk: error.chunk_id, attempts: error.attempts, error: error.error }));
  }
  if (scheduler) {
    const { feeds, models } = scheduler;
    stats.push(t("scheduler", {
      queued: num(feeds.queued), parked: num(feeds.parked), http: num(feeds.http_active), httpLimit: num(feeds.http_limit),
      parsing: num(feeds.parsing), parsingLimit: num(feeds.parsing_limit),
      buffered: megabytes(feeds.buffered_bytes), bufferLimit: megabytes(feeds.buffer_high_water),
    }));
    stats.push(t("modelQueue", { active: num(models.active), interactive: num(models.waiting_interactive), background: num(models.waiting_background) }));
  }
  return {
    db: [t("db", { path: db.path, size: megabytes(db.size_bytes), version: db.user_version, mode: db.journal_mode, fts: num(db.fts_rows) })],
    vec: [vec.available ? `sqlite-vec ${vec.version ?? ""}`.trim() : t("vecUnavailable", { reason: vec.reason ?? "—" })],
    stats: stats.length ? stats : ["—"],
    ai: [t("ai", {
      jobs: num(ai.job_tokens), budget: num(ai.budget_tokens), cache: num(ai.cache_rows),
      input: num(ai.chat_input_tokens), output: num(ai.chat_output_tokens),
      cost: ai.chat_cost.toLocaleString(locale(), { minimumFractionDigits: 4, maximumFractionDigits: 4 }),
    })],
  };
}
