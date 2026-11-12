import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

export const TOOLING_DIR = path.dirname(fileURLToPath(import.meta.url));
export const DESKTOP_ROOT = path.resolve(TOOLING_DIR, "..");

export function isDirectRun(metaUrl) {
  const entry = process.argv[1];
  if (!entry) return false;
  return pathToFileURL(path.resolve(entry)).href === metaUrl;
}

export function toPosix(filePath) {
  return filePath.split(path.sep).join("/");
}

export function relPosix(root, filePath) {
  return toPosix(path.relative(root, filePath));
}

export function sourceFingerprint(source, fallback = "") {
  const normalized = String(source ?? "").replace(/\s+/g, " ").trim() || fallback;
  return createHash("sha256").update(normalized, "utf8").digest("hex").slice(0, 16);
}

export function readJson(file) {
  let raw;
  try {
    raw = fs.readFileSync(file, "utf8");
  } catch (err) {
    throw new Error(`cannot read ${file}: ${err instanceof Error ? err.message : err}`);
  }
  try {
    return JSON.parse(raw);
  } catch (err) {
    throw new Error(`invalid JSON: ${file}`);
  }
}

export function writeJson(file, value) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`, "utf8");
}

export function parseArgs(argv, spec) {
  const flags = new Set(spec.flags ?? []);
  const values = new Set(spec.values ?? []);
  const out = { positional: [], flags: {}, values: {} };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--") {
      out.positional.push(...argv.slice(i + 1));
      break;
    }
    if (!arg.startsWith("-")) {
      out.positional.push(arg);
      continue;
    }
    if (!arg.startsWith("--")) {
      throw new Error(`unknown argument: ${arg}`);
    }
    const name = arg.slice(2);
    if (flags.has(name)) {
      out.flags[name] = true;
      continue;
    }
    if (values.has(name)) {
      const next = argv[i + 1];
      if (!next || next.startsWith("-")) {
        throw new Error(`missing value for --${name}`);
      }
      out.values[name] = next;
      i += 1;
      continue;
    }
    throw new Error(`unknown argument: ${arg}`);
  }
  return out;
}

export function diagnosticKey(item) {
  return [item.file, item.to ?? "", item.rule, item.message, item.fingerprint].join("\0");
}

export function groupDiagnostics(items) {
  const map = new Map();
  for (const item of items) {
    const key = diagnosticKey(item);
    const current = map.get(key);
    const count = item.count ?? 1;
    if (current) current.count += count;
    else map.set(key, { ...item, count });
  }
  return map;
}

export function diffBaseline(actualItems, baselineItems) {
  const actual = groupDiagnostics(actualItems);
  const baseline = groupDiagnostics(baselineItems);
  const added = [];
  const stale = [];
  for (const [key, item] of actual) {
    const allowed = baseline.get(key)?.count ?? 0;
    if (item.count > allowed) added.push({ ...item, count: item.count - allowed });
  }
  for (const [key, item] of baseline) {
    const have = actual.get(key)?.count ?? 0;
    if (have < item.count) stale.push({ ...item, count: item.count - have });
  }
  return { added, stale };
}

export function loadBaseline(file) {
  if (!fs.existsSync(file)) {
    throw new Error(`missing baseline ${file}; refuse to auto-create`);
  }
  const data = readJson(file);
  if (!data || !Array.isArray(data.items)) {
    throw new Error(`baseline ${file} must be { items: [...] }`);
  }
  return data;
}

export function printDiagnostics(title, items, stream = process.stdout) {
  stream.write(`${title}: ${items.length}\n`);
  for (const item of items) {
    const edge = item.to ? ` → ${item.to}` : "";
    stream.write(
      `  ${item.file}${edge}  ${item.rule}  ×${item.count}  ${item.fingerprint}\n    ${item.message}\n`,
    );
  }
}

