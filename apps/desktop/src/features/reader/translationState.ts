import type { ReaderView, TranslationEvent, TranslationFailure, TranslationIdentity, TranslationSnapshot, TranslationUnitState } from "../../bindings";
import { i18n } from "../../i18n";
import { mergeDocumentPatches, parseDocument, type Block, type Document } from "../../lib/blockIr";

export type TranslationData = { snapshot: TranslationSnapshot; document: Document; title: string; ready: boolean };
export const unitFailed = (unit: TranslationUnitState): unit is { failed: TranslationFailure } =>
  typeof unit === "object" && unit !== null && "failed" in unit;
export const translationActive = (data: TranslationData | null) => !!data && (
  ["queued", "running"].includes(data.snapshot.phase) || data.snapshot.save === "saving"
  || (["completed", "partial"].includes(data.snapshot.phase) && data.snapshot.save === "unsaved")
);
export function sameTranslation(a: TranslationIdentity, b: TranslationIdentity) {
  return a.article_id === b.article_id && a.generation === b.generation && a.source_hash === b.source_hash && a.config_hash === b.config_hash;
}
function requireValid(ok: boolean) { if (!ok) throw new Error(i18n.t("translation.inconsistent", { ns: "reader" })); }
function validUnit(unit: TranslationUnitState) {
  return ["pending", "inflight", "done"].includes(String(unit)) || unitFailed(unit);
}
function validateMapping(snapshot: TranslationSnapshot, reader: ReaderView) {
  requireValid(snapshot.identity.article_id === reader.id && snapshot.identity.source_hash === reader.source_hash);
  requireValid(Number.isSafeInteger(snapshot.seq) && snapshot.seq >= 0);
  requireValid(snapshot.identity.generation === null || (Number.isSafeInteger(snapshot.identity.generation) && snapshot.identity.generation > 0));
  requireValid(snapshot.units.length === snapshot.unit_map.length && snapshot.units.every(validUnit));
  const size = parseDocument(reader.document).blocks.length;
  const seen = new Set<number | null>();
  for (const block of snapshot.unit_map) {
    requireValid(!seen.has(block) && (block === null || (Number.isInteger(block) && block >= 0 && block < size)));
    seen.add(block);
  }
}
function validateUpdates(snapshot: TranslationSnapshot, blocks: TranslationSnapshot["blocks"], title: string | null) {
  const done = new Set(snapshot.unit_map.filter((_, i) => snapshot.units[i] === "done"));
  requireValid(blocks.every(block => done.has(block.index)) && (title === null || done.has(null)));
}
function data(snapshot: TranslationSnapshot, document: Document, title: string, retainedReady = false): TranslationData {
  const ready = snapshot.units.some((unit, i) => unit === "done" && snapshot.unit_map[i] !== null)
    || (snapshot.units.length > 0 && snapshot.unit_map.every(block => block === null) && snapshot.phase === "completed")
    || retainedReady;
  return { snapshot, document, title, ready };
}
export function restoreTranslation(reader: ReaderView, snapshot: TranslationSnapshot, previous: TranslationData | null = null): TranslationData {
  validateMapping(snapshot, reader);
  validateUpdates(snapshot, snapshot.blocks, snapshot.title);
  if (snapshot.phase !== "invalidated") {
    const completed = snapshot.unit_map.filter((_, i) => snapshot.units[i] === "done");
    requireValid(snapshot.blocks.length === completed.filter(index => index !== null).length);
    requireValid(!completed.includes(null) || snapshot.title !== null);
  }
  const retains = !!previous && previous.snapshot.identity.source_hash === snapshot.identity.source_hash
    && previous.snapshot.identity.config_hash === snapshot.identity.config_hash && snapshot.phase !== "invalidated";
  // 同一输入重译 / 失败重试保留仍有效的内容，直到对应的新块到达；失效绝不复用。
  const cached = !retains && reader.translation_ready && !!reader.translated_document && snapshot.phase !== "invalidated";
  const base = retains ? previous.document : cached ? reader.translated_document : reader.document;
  const document = mergeDocumentPatches(base, snapshot.phase === "invalidated" ? [] : snapshot.blocks);
  return data(snapshot, document, snapshot.title ?? (retains ? previous.title : cached ? reader.translated_title : ""), (retains && previous.ready) || cached);
}
export function applyTranslationEvent(current: TranslationData, event: TranslationEvent): TranslationData {
  const before = current.snapshot;
  if (!sameTranslation(before.identity, event.identity) || event.seq <= before.seq) return current;
  requireValid(event.seq === before.seq + 1 && event.total === before.units.length);
  const units = before.units.slice(), seen = new Set<number>();
  for (const [i, unit] of event.units) {
    requireValid(Number.isInteger(i) && i >= 0 && i < units.length && !seen.has(i) && validUnit(unit));
    seen.add(i); units[i] = unit;
  }
  // 作业级失效 / 超时可能只发 error，快照恢复才有完整的逐单元终态。
  if (event.kind === "error") {
    for (let i = 0; i < units.length; i++) if (units[i] === "pending" || units[i] === "inflight") units[i] = { failed: event.error ?? "invalid_output" };
  }
  requireValid(units.filter(unit => unit === "done").length === event.done && units.filter(unitFailed).length === event.failed);
  const snapshot: TranslationSnapshot = { ...before, seq: event.seq, phase: event.phase, save: event.save, units,
    title: event.title ?? before.title, blocks: [], error: event.error };
  validateUpdates(snapshot, event.blocks, event.title);
  return data(snapshot, mergeDocumentPatches(current.document, event.blocks), event.title ?? current.title, current.ready);
}

const sameBlock = (a: Block | undefined, b: Block | undefined) => a === b || JSON.stringify(a) === JSON.stringify(b);
/** 逐块说明。缓存重开的块是另一份对象，按内容判断失败处显示的是旧译文还是原文。 */
export function translationNotes(data: TranslationData | null, original: Document, mode: "original" | "bilingual" | "translated", debug: boolean) {
  const notes = new Map<number, string>();
  if (!data || mode === "original") return notes;
  data.snapshot.units.forEach((unit, i) => {
    const block = data.snapshot.unit_map[i];
    if (block === null) return;
    if (unitFailed(unit)) {
      const note = sameBlock(data.document.blocks[block], original.blocks[block]) ? i18n.t("translation.note.keptOriginal", { ns: "reader" }) : i18n.t("translation.note.keptTranslation", { ns: "reader" });
      notes.set(block, debug ? i18n.t("translation.note.withReason", { ns: "reader", note, reason: unit.failed }) : note);
    }
    else if (unit !== "done" && mode === "translated") notes.set(block, i18n.t("translation.note.showOriginal", { ns: "reader" }));
    else if (debug && unit === "inflight") notes.set(block, i18n.t("translation.note.inflight", { ns: "reader" }));
  });
  return notes;
}
