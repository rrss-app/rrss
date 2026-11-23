/**
 * 伪本地化（用于布局与译文长度检查）：从英文包生成 `en-XA`，字母换成带附加符号的同形字、按长度加长约 40%、首尾加括号。
 * 截图里没变形的文字就是漏进语言包的硬编码，括号不全就是被截断。只在界面调试模式下可选，不进正式选项。
 */
export const PSEUDO_LANGUAGE = "en-XA";

const ACCENTED: Record<string, string> = {
  a: "á", b: "ƀ", c: "ç", d: "ð", e: "é", f: "ƒ", g: "ĝ", h: "ĥ", i: "î", j: "ĵ", k: "ķ", l: "ļ", m: "ɱ",
  n: "ñ", o: "ö", p: "þ", q: "ǫ", r: "ŕ", s: "š", t: "ţ", u: "û", v: "ṽ", w: "ŵ", x: "ẋ", y: "ý", z: "ž",
  A: "Á", B: "Ɓ", C: "Ç", D: "Ð", E: "É", F: "Ƒ", G: "Ĝ", H: "Ĥ", I: "Î", J: "Ĵ", K: "Ķ", L: "Ļ", M: "Ṁ",
  N: "Ñ", O: "Ö", P: "Þ", Q: "Ǫ", R: "Ŕ", S: "Š", T: "Ţ", U: "Û", V: "Ṽ", W: "Ŵ", X: "Ẋ", Y: "Ý", Z: "Ž",
};

/** 一条文案的伪译文：插值 `{{…}}`、标签 `<…>` 与 HTML 实体原样保留。 */
export function pseudoText(text: string): string {
  if (!text.trim()) return text;
  const parts = text.split(/(\{\{[^}]*\}\}|<[^>]*>|&\w+;)/);
  const body = parts.map((part, i) => i % 2 ? part : part.replace(/[A-Za-z]/g, (ch) => ACCENTED[ch] ?? ch)).join("");
  const visible = parts.filter((_, i) => i % 2 === 0).join("").length;
  const pad = visible > 3 ? " " + "·".repeat(Math.ceil(visible * 0.4)) : "";
  // 前后的空格（拼接用）留在括号外，不改变拼接结果。
  const lead = body.match(/^\s*/)![0];
  const trail = body.match(/\s*$/)![0];
  return `${lead}[${body.trim()}${pad}]${trail}`;
}

/** 整个命名空间的伪译文，结构不变。 */
export function pseudoTree<T>(tree: T): T {
  if (typeof tree === "string") return pseudoText(tree) as T;
  if (Array.isArray(tree)) return tree.map(pseudoTree) as T;
  if (tree && typeof tree === "object") {
    return Object.fromEntries(Object.entries(tree).map(([key, value]) => [key, pseudoTree(value)])) as T;
  }
  return tree;
}
