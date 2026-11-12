import fs from "node:fs";
import path from "node:path";
import { DESKTOP_ROOT, isDirectRun } from "./cli.mjs";

// 前端 `invoke("名", { 参数 })` 与 Rust 命令靠字符串对上；这里在不编译 Rust 的前提下核对三处：
// 命令已注册、参数名与签名一致（Tauri 把 snake_case 参数转成 camelCase）、模拟宿主没有已删除的命令。

const COMMANDS_DIR = path.resolve(DESKTOP_ROOT, "../../crates/rrss-desktop/src/commands");
const LIB_RS = path.resolve(DESKTOP_ROOT, "../../crates/rrss-desktop/src/lib.rs");
const API_TS = path.join(DESKTOP_ROOT, "src/lib/api.ts");
const EVENTS_RS = path.resolve(DESKTOP_ROOT, "../../crates/rrss-desktop/src/events.rs");
const MOCK_HOSTS = ["e2e/mockHost.ts", "perf/host.ts"].map((file) => path.join(DESKTOP_ROOT, file));
/** 由 Tauri 注入、不走前端参数的类型。 */
const INJECTED = /^(tauri::)?(State|AppHandle|Window|WebviewWindow|Webview|ipc::Request|ipc::Channel)\b/;

const camel = (name) => name.replace(/_([a-z0-9])/g, (_, c) => c.toUpperCase());

/** 按顶层逗号切分，跳过括号、泛型与字符串内部。Rust 的 `'_` 是生命周期不是字符串，只认双引号。 */
function splitTop(text, quotes = "\"'`") {
  const parts = [];
  let depth = 0;
  let quote = "";
  let current = "";
  for (let i = 0; i < text.length; i++) {
    const ch = text[i];
    if (quote) {
      if (ch === "\\") { current += ch + text[++i]; continue; }
      if (ch === quote) quote = "";
    } else if (quotes.includes(ch)) quote = ch;
    else if ("([{<".includes(ch)) depth++;
    else if (")]}>".includes(ch) && !(ch === ">" && "=-".includes(text[i - 1]))) depth--;
    else if (ch === "," && depth === 0) { parts.push(current.trim()); current = ""; continue; }
    current += ch;
  }
  if (current.trim()) parts.push(current.trim());
  return parts;
}

/** 从 `open` 位置起取配对括号内的文本。 */
function balanced(text, open, quotes = "\"'`") {
  const pairs = { "(": ")", "{": "}" };
  const close = pairs[text[open]];
  let depth = 0;
  let quote = "";
  for (let i = open; i < text.length; i++) {
    const ch = text[i];
    if (quote) {
      if (ch === "\\") i++;
      else if (ch === quote) quote = "";
      continue;
    }
    if (quotes.includes(ch)) quote = ch;
    else if (ch === text[open]) depth++;
    else if (ch === close && --depth === 0) return text.slice(open + 1, i);
  }
  throw new Error(`unbalanced ${text[open]} at ${open}`);
}

function params(list) {
  return splitTop(list, "\"").map((param) => {
    const [name, ...type] = param.split(":");
    return { name: name.trim().replace(/^mut\s+/, ""), type: type.join(":").trim() };
  }).filter((p) => !INJECTED.test(p.type)).map((p) => ({ key: camel(p.name), optional: p.type.startsWith("Option<") }));
}

/** 手写的 `#[tauri::command] fn` 与 `db_command!` / `db_task!` 生成的命令。 */
export function rustCommands(source) {
  const found = new Map();
  for (const m of source.matchAll(/#\[tauri::command[^\]]*\]\s*pub\s+(?:async\s+)?fn\s+(\w+)\s*(?:<[^>]*>)?\s*\(/g)) {
    found.set(m[1], params(balanced(source, m.index + m[0].length - 1, "\"")));
  }
  for (const m of source.matchAll(/\bdb_(?:command|task)!\(\s*(?:\/\/\/[^\n]*\n\s*)*(\w+)\(/g)) {
    found.set(m[1], params(balanced(source, m.index + m[0].length - 1, "\"")));
  }
  return found;
}

export function registered(moduleSource) {
  const list = moduleSource.match(/\bcommand_handler!\s*\[([\s\S]*?)\]/);
  if (!list) throw new Error("command_handler! not found");
  return splitTop(list[1], "\"");
}

/** 只把 command_modules! 接入的模块算作已注册；定义和登记必须在同一模块。 */
export function commandRegistry(sources) {
  const list = sources.get("mod.rs")?.match(/\bcommand_modules!\s*\{([\s\S]*?)\}/);
  if (!list) throw new Error("command_modules! not found");
  const modules = [...list[1].matchAll(/\bmod\s+(\w+)\s*;/g)].map((m) => `${m[1]}.rs`);
  const commands = new Map();
  const handlers = new Set();
  const errors = [];
  for (const file of modules) {
    const source = sources.get(file);
    if (source === undefined) { errors.push(`command_modules! 的 ${file} 不存在`); continue; }
    const local = rustCommands(source);
    for (const [name, args] of local) {
      if (commands.has(name)) errors.push(`命令 ${name} 在多个模块定义`);
      commands.set(name, args);
    }
    let names;
    try { names = registered(source); }
    catch (error) { errors.push(`${file}: ${error.message}`); continue; }
    for (const name of names) {
      if (!local.has(name)) errors.push(`${file} 登记了 ${name}，但本模块找不到定义`);
      if (handlers.has(name)) errors.push(`命令 ${name} 重复登记`);
      handlers.add(name);
    }
    for (const name of local.keys()) {
      if (!names.includes(name)) errors.push(`${file} 定义的 ${name} 未登记`);
    }
  }
  for (const [file, source] of sources) {
    if (file !== "mod.rs" && !modules.includes(file) && rustCommands(source).size) {
      errors.push(`${file} 有命令定义，但未接入 command_modules!`);
    }
  }
  return { commands, handlers, errors };
}

/** `invoke<T>("名", { a, b: x })`：参数不是对象字面量（如变量）时 `keys` 为 null，不核对。 */
export function frontendCalls(source) {
  const calls = [];
  for (const m of source.matchAll(/\binvoke\b(?:<[^(]*>)?\(\s*"([\w:|]+)"\s*(,\s*)?/g)) {
    const line = source.slice(0, m.index).split("\n").length;
    let keys = [];
    if (m[2]) {
      const at = m.index + m[0].length;
      if (source[at] !== "{") keys = null;
      else {
        const entries = splitTop(balanced(source, at));
        keys = entries.some((e) => e.startsWith("...")) ? null
          : entries.map((e) => e.split(":")[0].trim().replace(/^["']|["']$/g, ""));
      }
    }
    calls.push({ name: m[1], keys, line });
  }
  return calls;
}

export function mockedCommands(source) {
  const names = new Set();
  for (const m of source.matchAll(/case\s+"([\w:|]+)"\s*:/g)) names.add(m[1]);
  return [...names].filter((name) => !name.startsWith("plugin:"));
}

export function checkIpc({ commands, handlers, calls, mocks }) {
  const errors = [];
  for (const name of handlers) if (!commands.has(name)) errors.push(`模块清单登记了 ${name}，但 commands/ 里找不到定义`);
  for (const call of calls) {
    const where = `api.ts:${call.line} ${call.name}`;
    if (!handlers.has(call.name)) { errors.push(`${where}：命令未注册`); continue; }
    if (call.keys === null) continue;
    const expected = commands.get(call.name) ?? [];
    for (const key of call.keys) {
      if (!expected.some((p) => p.key === key)) errors.push(`${where}：多出参数 ${key}（Rust 参数：${expected.map((p) => p.key).join(", ") || "无"}）`);
    }
    for (const p of expected) {
      if (!p.optional && !call.keys.includes(p.key)) errors.push(`${where}：缺少必填参数 ${p.key}`);
    }
  }
  for (const { file, names } of mocks) {
    for (const name of names) if (!handlers.has(name)) errors.push(`${file} 模拟了不存在的命令 ${name}`);
  }
  return errors;
}

export function checkChatEvents(api, host) {
  const listens = [...api.matchAll(/\blisten(?:<[^>]+>)?\("(chat:[\w]+)"/g)].map((m) => m[1]);
  const emits = [...host.matchAll(/\.emit\("(chat:[\w]+)"/g)].map((m) => m[1]);
  const errors = [];
  for (const name of new Set([...listens, ...emits])) {
    if (!listens.includes(name)) errors.push(`${name}：宿主事件缺少 api.ts 监听`);
    if (!emits.includes(name)) errors.push(`${name}：api.ts 监听缺少宿主事件`);
  }
  return errors;
}

export function runCheckIpc(io = { stdout: process.stdout, stderr: process.stderr }) {
  const sources = new Map(fs.readdirSync(COMMANDS_DIR).filter((f) => f.endsWith(".rs"))
    .map((file) => [file, fs.readFileSync(path.join(COMMANDS_DIR, file), "utf8")]));
  const { commands, handlers, errors } = commandRegistry(sources);
  if (!/\.invoke_handler\(\s*main_only\(\s*commands::invoke_handler\s*\)\s*\)/.test(fs.readFileSync(LIB_RS, "utf8"))) {
    errors.push("lib.rs 未接入 main_only(commands::invoke_handler)");
  }
  const calls = frontendCalls(fs.readFileSync(API_TS, "utf8"));
  const mocks = MOCK_HOSTS.map((file) => ({ file: path.relative(DESKTOP_ROOT, file), names: mockedCommands(fs.readFileSync(file, "utf8")) }));
  errors.push(...checkIpc({ commands, handlers, calls, mocks }), ...checkChatEvents(fs.readFileSync(API_TS, "utf8"), fs.readFileSync(EVENTS_RS, "utf8")));
  if (errors.length) {
    io.stderr.write(`ipc check failed:\n${errors.map((e) => `  ${e}`).join("\n")}\n`);
    return 1;
  }
  io.stdout.write(`ipc check ok: ${calls.length} calls, ${handlers.size} commands\n`);
  return 0;
}

if (isDirectRun(import.meta.url)) {
  process.exit(runCheckIpc());
}
