/**
 * 列表标题自动翻译的前端队列：卡片渲染时登记还没有译文的外文标题，按批交给后端，结果放在内存里。
 * 同一时间只有一批在途；失败的这次运行内不再请求；后端说暂停（预算用完、没配置、没开）时
 * 停一段时间再试。后端会把译文存库，下次列表查询直接带回来，这里只补刚翻出来、列表还没重查的。
 */
import { useEffect, useSyncExternalStore } from "react";
import { TITLE_TRANSLATE_BATCH, type ArticleView } from "../bindings";
import { api } from "./api";

const PAUSE_MS = 10 * 60_000;
const DEBOUNCE_MS = 120;

type Transport = (ids: number[]) => ReturnType<typeof api.translateTitles>;

export function createTitleTranslations(send: Transport = (ids) => api.translateTitles(ids), now = () => Date.now()) {
  const titles = new Map<number, string>();
  const settled = new Set<number>();
  const queue = new Set<number>();
  const listeners = new Set<() => void>();
  let inflight = false;
  let pausedUntil = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let epoch = 0;

  const emit = () => { for (const listener of listeners) listener(); };
  const schedule = () => {
    if (timer !== undefined || inflight || queue.size === 0) return;
    timer = setTimeout(() => { timer = undefined; void flush(); }, DEBOUNCE_MS);
  };
  async function flush() {
    if (inflight || queue.size === 0) return;
    if (now() < pausedUntil) { queue.clear(); return; }
    const ids = [...queue].slice(0, TITLE_TRANSLATE_BATCH);
    for (const id of ids) queue.delete(id);
    const started = epoch;
    inflight = true;
    try {
      const reply = await send(ids);
      if (started !== epoch) return;
      for (const item of reply.titles) titles.set(item.id, item.title);
      if (reply.paused) {
        // 暂停时没翻的不算失败：恢复后卡片重新出现会再登记。
        pausedUntil = now() + PAUSE_MS;
        queue.clear();
      } else {
        // 没返回的是已经不生效的，这次运行内不再问。
        for (const id of ids) settled.add(id);
      }
      if (reply.titles.length) emit();
    } catch {
      if (started === epoch) for (const id of ids) settled.add(id);
    } finally {
      if (started === epoch) { inflight = false; schedule(); }
    }
  }

  return {
    /** 登记一条需要译文的标题；已翻过、失败过或正在排队的忽略。 */
    request(id: number) {
      if (titles.has(id) || settled.has(id) || queue.has(id) || now() < pausedUntil) return;
      queue.add(id);
      schedule();
    },
    get: (id: number) => titles.get(id),
    /** 每次 `reset` 加一；卡片据此重新登记。 */
    epoch: () => epoch,
    subscribe(listener: () => void) { listeners.add(listener); return () => { listeners.delete(listener); }; },
    /** 设置变了（开关、目标语言、服务）：清掉失败与暂停记录，已到的译文作废。 */
    reset() {
      epoch++;
      clearTimeout(timer); timer = undefined;
      titles.clear(); settled.clear(); queue.clear();
      inflight = false; pausedUntil = 0;
      emit();
    },
  };
}

export type TitleTranslationStore = ReturnType<typeof createTitleTranslations>;
export const titleTranslations = createTitleTranslations();

/** 卡片要显示的标题：标题自动翻译生效时先用列表带回的译文，没有就登记并等队列补上。 */
export function useDisplayTitle(row: Pick<ArticleView, "id" | "title" | "title_lang" | "title_translation">, store: TitleTranslationStore = titleTranslations) {
  const needs = !!row.title_lang && !row.title_translation;
  const epoch = useSyncExternalStore(store.subscribe, store.epoch);
  useEffect(() => { if (needs) store.request(row.id); }, [needs, row.id, store, epoch]);
  const fetched = useSyncExternalStore(store.subscribe, () => (needs ? store.get(row.id) : undefined));
  const translated = row.title_lang ? row.title_translation || fetched : undefined;
  return { title: translated || row.title, translated: !!translated, lang: row.title_lang ?? undefined };
}
