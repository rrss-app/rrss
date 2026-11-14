import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { diffCounts, totalLiterals } from "./i18n-ratchet.mjs";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const BASELINE = path.join(ROOT, "scripts/rust-i18n-baseline.json");
const EXEMPTIONS = path.join(ROOT, "scripts/rust-i18n-exemptions.json");
const HAN = /\p{Script=Han}/u;
const SKIP = /(^|\/)(tests?|benches|examples|fixtures|testdata)(\/|$)|(^|\/)([^/]*_tests?|tests?_[^/]*|tests?)\.rs$/;

// Lex Rust rather than stripping comments with regexes: comments may nest, and
// raw strings may contain comment markers, quotes, braces and test attributes.
function tokens(source) {
  const out = [];
  let i = 0;
  while (i < source.length) {
    if (/\s/.test(source[i])) { i++; continue; }
    if (source.startsWith("//", i)) { i = source.indexOf("\n", i); if (i < 0) break; continue; }
    if (source.startsWith("/*", i)) {
      i += 2;
      let depth = 1;
      while (i < source.length && depth) {
        if (source.startsWith("/*", i)) { depth++; i += 2; }
        else if (source.startsWith("*/", i)) { depth--; i += 2; }
        else i++;
      }
      continue;
    }
    const raw = /^(?:br|cr|r)(#*)"/.exec(source.slice(i));
    if (raw) {
      const start = i + raw[0].length;
      const end = source.indexOf(`"${raw[1]}`, start);
      out.push({ literal: true, text: source.slice(start, end < 0 ? source.length : end) });
      i = end < 0 ? source.length : end + raw[1].length + 1;
      continue;
    }
    const quoted = /^(?:b|c)?"/.exec(source.slice(i));
    if (quoted) {
      const start = i + quoted[0].length;
      i = start;
      while (i < source.length && source[i] !== '"') i += source[i] === "\\" ? 2 : 1;
      const text = source.slice(start, i).replace(/\\(?:u\{([\da-f_]+)\}|[\s\S])/gi, (escape, hex) => {
        if (!hex) return escape;
        const value = parseInt(hex.replaceAll("_", ""), 16);
        return value <= 0x10ffff ? String.fromCodePoint(value) : escape;
      });
      out.push({ literal: true, text });
      i++;
      continue;
    }
    // Character literals are not strings; lifetimes must remain ordinary tokens.
    const char = /^'(?:\\(?:u\{[\da-f_]+\}|x[\da-f]{2}|.)|[^'\\])'/iu.exec(source.slice(i));
    if (char) { i += char[0].length; continue; }
    const word = /^[a-z_][a-z_\d]*/i.exec(source.slice(i));
    if (word) { out.push({ text: word[0] }); i += word[0].length; }
    else { out.push({ text: source[i] }); i++; }
  }
  return out;
}

function afterGroup(list, start, open, close) {
  let depth = 0;
  for (let i = start; i < list.length; i++) {
    if (list[i].literal) continue;
    if (list[i].text === open) depth++;
    if (list[i].text === close && --depth === 0) return i + 1;
  }
  return list.length;
}

// Evaluate cfg with test=false, other predicates unknown. Only an item which
// cannot exist outside tests is skipped; cfg(not(test)) is production code.
function testOnlyCfg(list) {
  let i = 0;
  function predicate() {
    const name = list[i++]?.text;
    if (["all", "any", "not"].includes(name) && list[i]?.text === "(") {
      i++;
      const values = [];
      while (i < list.length && list[i].text !== ")") {
        values.push(predicate());
        if (list[i]?.text === ",") i++;
      }
      i++;
      if (name === "not") return values[0] == null ? null : !values[0];
      if (name === "all") return values.includes(false) ? false : values.includes(null) ? null : true;
      return values.includes(true) ? true : values.includes(null) ? null : false;
    }
    if (list[i]?.text === "=") i += 2;
    return name === "test" ? false : null;
  }
  return predicate() === false;
}

function afterItem(list, start) {
  let i = start;
  while (i < list.length) {
    if (list[i].literal) { i++; continue; }
    const token = list[i].text;
    if (token === ";") return i + 1;
    if (token === "{") return afterGroup(list, i, "{", "}");
    if (token === "(") i = afterGroup(list, i, "(", ")");
    else if (token === "[") i = afterGroup(list, i, "[", "]");
    else i++;
  }
  return i;
}

export function countRustLiterals(source) {
  const list = tokens(source);
  let count = 0;
  for (let i = 0; i < list.length;) {
    if (list[i].text === "#" && list[i + 1]?.text === "[") {
      let skip = false;
      let attributes = 0;
      while (list[i]?.text === "#" && list[i + 1]?.text === "[") {
        const end = afterGroup(list, i + 1, "[", "]");
        const attr = list.slice(i + 2, end - 1);
        skip ||= attr[0]?.text === "cfg" && testOnlyCfg(attr.slice(2, -1));
        skip ||= attr.some((token, at) => token.text === "test" && (at === 0 || attr[at - 1].text === ":"));
        if (attr[0]?.text !== "doc") attributes += attr.filter(token => token.literal && HAN.test(token.text)).length;
        i = end;
      }
      if (skip) i = afterItem(list, i);
      else count += attributes;
      continue;
    }
    if (list[i].literal && HAN.test(list[i].text)) count++;
    i++;
  }
  return count;
}

export function scan(root = path.join(ROOT, "crates"), exemptions = {}) {
  const counts = {};
  function walk(dir) {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const abs = path.join(dir, entry.name);
      const file = path.relative(root, abs).split(path.sep).join("/");
      if (SKIP.test(file) || entry.name === "target") continue;
      if (entry.isDirectory()) walk(abs);
      else if (entry.name.endsWith(".rs") && !Object.hasOwn(exemptions, file)) {
        const count = countRustLiterals(fs.readFileSync(abs, "utf8"));
        if (count) counts[file] = count;
      }
    }
  }
  walk(root);
  return Object.fromEntries(Object.entries(counts).sort(([a], [b]) => a.localeCompare(b)));
}

export function runCheck(argv = [], io = process, options = {}) {
  if (argv.some(arg => arg !== "--update")) throw new Error("expected --update or no arguments");
  const root = options.root ?? path.join(ROOT, "crates");
  const baselinePath = options.baseline ?? BASELINE;
  const exemptions = options.exemptions ?? JSON.parse(fs.readFileSync(EXEMPTIONS, "utf8"));
  for (const [file, reason] of Object.entries(exemptions)) {
    if (typeof reason !== "string" || !reason.trim()) throw new Error(`missing exemption reason: ${file}`);
    if (!fs.existsSync(path.join(root, file))) throw new Error(`stale exemption: ${file}`);
  }
  const actual = scan(root, exemptions);
  if (argv.includes("--update")) {
    fs.writeFileSync(baselinePath, `${JSON.stringify({ files: actual }, null, 2)}\n`);
    io.stdout.write(`Rust i18n baseline updated: ${totalLiterals(actual)} literals\n`);
    return 0;
  }
  const { added, stale } = diffCounts(actual, JSON.parse(fs.readFileSync(baselinePath, "utf8")).files);
  if (added.length) {
    io.stderr.write("Rust i18n check failed: return a UI code and parameters instead of new Chinese strings.\n");
    for (const { file, allowed, now } of added) io.stderr.write(`  ${file}: ${allowed} -> ${now}\n`);
    return 1;
  }
  if (stale.length) io.stdout.write(`Rust i18n: ${stale.length} files below baseline; --update can tighten it after merging.\n`);
  io.stdout.write(`Rust i18n check ok: ${totalLiterals(actual)} literals in ${Object.keys(actual).length} files\n`);
  return 0;
}

if (process.argv[1] && pathToFileURL(path.resolve(process.argv[1])).href === import.meta.url) {
  process.exitCode = runCheck(process.argv.slice(2));
}
