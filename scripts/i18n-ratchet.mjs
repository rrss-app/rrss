/** A reduction never blocks parallel migrations; only increases fail. */
export function diffCounts(actual, baseline) {
  const added = [];
  const stale = [];
  for (const file of new Set([...Object.keys(actual), ...Object.keys(baseline)])) {
    const now = actual[file] ?? 0;
    const allowed = baseline[file] ?? 0;
    if (now > allowed) added.push({ file, now, allowed });
    else if (now < allowed) stale.push({ file, now, allowed });
  }
  return { added, stale };
}

export const totalLiterals = (counts) => Object.values(counts).reduce((sum, n) => sum + n, 0);
