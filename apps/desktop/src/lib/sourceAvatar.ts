/** 缓存图标不可用时，用来源首字与稳定色相回退。 */
export function localIconHref(href: string): string {
  return /^(?:rrss:\/\/localhost|http:\/\/rrss\.localhost)\/media\/\d+(?:\?.*)?$/.test(href) ? href : "";
}

export function sourceInitial(title: string): string {
  const trimmed = title.trim();
  if (!trimmed) return "·";
  const first = [...trimmed][0] ?? "";
  return /[a-z]/i.test(first) ? first.toUpperCase() : first;
}

export function sourceHue(title: string): number {
  let hash = 0;
  for (const ch of title.trim()) hash = (hash * 31 + ch.codePointAt(0)!) >>> 0;
  return hash % 360;
}
