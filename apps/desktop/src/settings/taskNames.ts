import { i18n } from "../i18n";

/** 模型用途的名字：后端只给用途 id，文字取语言包；不认识的 id 原样显示。 */
export function taskTitle(task: string): string {
  const key = `model_task.${task}` as "model_task.chat";
  return i18n.exists(key, { ns: "errors" }) ? String(i18n.t(key, { ns: "errors" })) : task;
}
