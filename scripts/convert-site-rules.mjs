// FiveFilters configurations are public domain (upstream LICENSE.txt).
// README.md “File naming”: .example.com.txt means subdomains, excluding www/bare.
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

const fields = ["body", "title", "author", "date", "strip"];
const quoted = String.raw`(?:"[^"]*"|'[^']*')`;
const attr = String.raw`[a-zA-Z_][\w-]*`;
const cssString = (value) => {
  if (value.includes("\0")) throw new Error("null_character");
  return `"${value.replace(/["\\\x00-\x1f\x7f]/g, (ch) =>
    ch === '"' || ch === "\\" ? `\\${ch}` : `\\${ch.codePointAt(0).toString(16)} `)}"`;
};
const unquote = (value) => value.slice(1, -1);

// Split only outside XPath strings, predicates and function calls. Never guess at syntax.
function splitTop(value, delimiter) {
  let quote = "", depth = 0, start = 0;
  const parts = [];
  for (let i = 0; i < value.length; i++) {
    const ch = value[i];
    if (quote) { if (ch === quote) quote = ""; continue; }
    if (ch === '"' || ch === "'") { quote = ch; continue; }
    if (ch === "[" || ch === "(") depth++;
    if (ch === "]" || ch === ")") depth--;
    if (depth < 0) throw new Error("malformed_xpath");
    if (!depth && value.startsWith(delimiter, i)) {
      parts.push(value.slice(start, i));
      i += delimiter.length - 1;
      start = i + 1;
    }
  }
  if (quote || depth) throw new Error("malformed_xpath");
  return [...parts, value.slice(start)].map((p) => p.trim());
}

// XPath string comparisons are case-sensitive, including HTML enumerated attributes.
function predicate(value) {
  const equality = value.match(new RegExp(`^@(${attr})\\s*=\\s*(${quoted})$`));
  if (equality) return `[${equality[1]}=${cssString(unquote(equality[2]))} s]`;
  const exists = value.match(new RegExp(`^@(${attr})$`));
  if (exists) return `[${exists[1]}]`;
  const contains = value.match(new RegExp(`^(contains|starts-with)\\(\\s*@(${attr})\\s*,\\s*(${quoted})\\s*\\)$`));
  if (contains) {
    const text = unquote(contains[3]);
    if (!text) throw new Error("empty_substring");
    return `[${contains[2]}${contains[1] === "contains" ? "*" : "^"}=${cssString(text)} s]`;
  }
  throw new Error("unsupported_predicate");
}

function step(value) {
  const tag = value.match(/^(\*|[a-zA-Z][\w-]*)/);
  if (!tag) throw new Error("unsupported_node_or_axis");
  let rest = value.slice(tag[0].length), result = tag[0];
  while (rest) {
    // Find the predicate end without treating quoted ']' as syntax.
    const match = rest.match(/^\[((?:"[^"]*"|'[^']*'|[^\[\]"'])*)\]/);
    if (!match) throw new Error("unsupported_node_or_axis");
    const condition = match[1].trim();
    const alternatives = splitTop(condition, " or ");
    result += alternatives.length > 1
      ? `:is(${alternatives.map((v) => `*${predicate(v)}`).join(", ")})`
      : splitTop(condition, " and ").map(predicate).join("");
    rest = rest.slice(match[0].length);
  }
  return result;
}

/** Exact subset only: attributes, substring predicates, child/descendant paths, union. */
export function xpathToCss(xpath) {
  return splitTop(xpath.trim(), "|").map((branch) => {
    if (!branch.startsWith("//")) throw new Error("unsupported_root");
    // First split descendants, then direct children; slashes in predicates stay intact.
    return splitTop(branch.slice(2), "//")
      .map((part) => splitTop(part, "/").map(step).join(" > "))
      .join(" ");
  }).join(", ");
}

const normalizeDomain = (domain) => {
  const lower = domain.toLowerCase();
  return lower.startsWith("*.")
    ? `*.${lower.slice(2).replace(/^www\./, "")}`
    : lower.replace(/^www\./, "");
};

export function convertRules(files, commit) {
  const skipped = {}, ignored = {}, rules = [], domains = new Set();
  let converted = 0, emptyRules = 0;
  const count = (bucket, reason) => { bucket[reason] = (bucket[reason] ?? 0) + 1; };
  for (const [name, contents] of [...files].sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0)) {
    if (!/^\.?[a-z\d-]+(?:\.[a-z\d-]+)+\.txt$/i.test(name)) {
      count(skipped, "non_domain_filename"); continue;
    }
    const domain = normalizeDomain(name.slice(0, -4).replace(/^\./, "*."));
    const selectors = Object.fromEntries(fields.map((key) => [key, []]));
    for (const line of contents.split(/\r?\n/)) {
      const directive = line.trim().match(/^([a-z_]+)(\(.*\))?:\s*(.*?)\s*$/);
      if (!directive) continue;
      const [, key, parameters, value] = directive;
      if (parameters || (!fields.includes(key) && key !== "strip_id_or_class")) {
        count(ignored, key); continue;
      }
      try {
        if (!value) throw new Error("empty_value");
        const css = key === "strip_id_or_class"
          ? `[id*=${cssString(value)}], [class*=${cssString(value)}]`
          : xpathToCss(value);
        selectors[key === "strip_id_or_class" ? "strip" : key].push(css);
        converted++;
      } catch (error) { count(skipped, error.message); }
    }
    if (!fields.some((key) => selectors[key].length)) { emptyRules++; continue; }
    if (domains.has(domain)) { count(skipped, "duplicate_domain"); continue; }
    domains.add(domain);
    rules.push({ domain, ...Object.fromEntries(fields.map((key) => [key,
      key === "strip" ? [...new Set(selectors[key])] : selectors[key].join(", ") || null,
    ])) });
  }
  rules.sort((a, b) => a.domain < b.domain ? -1 : a.domain > b.domain ? 1 : 0);
  return {
    data: { source: "fivefilters/ftr-site-config", commit, rules },
    stats: { rules: rules.length, converted, emptyRules, skipped, ignored },
  };
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const [input, output] = process.argv.slice(2);
  if (!input || !output) throw new Error("Usage: node scripts/convert-site-rules.mjs <checkout> <output.json>");
  const commit = execFileSync("git", ["-C", input, "rev-parse", "HEAD"], { encoding: "utf8" }).trim();
  const files = readdirSync(input).filter((name) => name.endsWith(".txt") && name !== "LICENSE.txt")
    .map((name) => [name, readFileSync(resolve(input, name), "utf8")]);
  const { data, stats } = convertRules(files, commit);
  const json = `${JSON.stringify(data, null, 2)}\n`;
  const bytes = Buffer.byteLength(json);
  if (bytes > 1_500_000) throw new Error(`Data exceeds 1.5 MB: ${bytes} bytes; no file written`);
  writeFileSync(output, json);
  process.stdout.write(`${JSON.stringify({ ...stats, bytes, commit }, null, 2)}\n`);
}
