/** 全局单键快捷键：输入框 / composer 内不触发。 */
export function isTypingTarget(el: Element | null): boolean {
  if (!el) return false;
  const tag = el.tagName;
  if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return true;
  if (el instanceof HTMLElement && el.isContentEditable) return true;
  if (el.closest(".drawer.settings, .feed-edit, .model-dropdown-menu")) return true;
  return false;
}

export function isMetaK(e: KeyboardEvent): boolean {
  return (e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k";
}
