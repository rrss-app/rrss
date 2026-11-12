/** 模型选择列表的模糊匹配与缩写（阶段 19-6）。 */

export type FuzzyHit = { score: number; indices: number[] };

const BOUNDARY = /[/\-_.:\s]/;

/**
 * 查询里的字符按顺序出现即命中；连续命中与词首命中加分。三个字符以上的查询，零散命中过半不算，
 * 免得 `son` 这类查询把几乎所有长 id 都筛进来。
 */
export function fuzzyMatch(text: string, query: string): FuzzyHit | null {
  const chars = [...query.toLowerCase().replace(/\s+/g, "")];
  if (!chars.length) return { score: 0, indices: [] };
  const lower = text.toLowerCase();
  let from = 0;
  let prev = -2;
  let score = 0;
  let weak = 0;
  const indices: number[] = [];
  for (const ch of chars) {
    const at = lower.indexOf(ch, from);
    if (at < 0) return null;
    const next = at === prev + 1;
    const start = at === 0 || BOUNDARY.test(lower[at - 1]);
    score += (next ? 3 : 1) + (start ? 2 : 0);
    if (!next && !start) weak += 1;
    indices.push(at);
    prev = at;
    from = at + 1;
  }
  if (chars.length >= 3 && weak > chars.length / 2) return null;
  return { score: score - text.length / 100, indices };
}

/** 按钮上的缩写：去掉 `厂商/` 前缀，过长截掉中间。完整 id 放在悬停提示和展开的列表里。 */
export function shortModel(model: string, max = 22) {
  const bare = model.slice(model.indexOf("/") + 1);
  return bare.length > max ? `${bare.slice(0, max - 9)}…${bare.slice(-8)}` : bare;
}
