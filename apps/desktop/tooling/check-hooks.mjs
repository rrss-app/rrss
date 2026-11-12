import fs from "node:fs";
import path from "node:path";
import { ESLint } from "eslint";
import {
  DESKTOP_ROOT,
  diffBaseline,
  isDirectRun,
  loadBaseline,
  parseArgs,
  printDiagnostics,
  relPosix,
  sourceFingerprint,
  writeJson,
} from "./cli.mjs";
import { hooksPluginBlock } from "./hooks-config.mjs";

export const DEFAULT_HOOKS_BASELINE = path.join(DESKTOP_ROOT, "tooling", "hooks-baseline.json");

function lineText(filePath, line) {
  if (!line || line < 1) return "";
  try {
    const rows = fs.readFileSync(filePath, "utf8").split(/\r?\n/);
    return rows[line - 1] ?? "";
  } catch {
    return "";
  }
}

function isFatalMessage(message) {
  return Boolean(message.fatal) || (!message.ruleId && message.severity === 2);
}

export function diagnosticsFromEslint(root, results) {
  const items = [];
  const fatals = [];
  for (const result of results) {
    const file = relPosix(root, result.filePath);
    for (const message of result.messages) {
      if (isFatalMessage(message)) {
        fatals.push({
          file,
          rule: "eslint/fatal",
          message: message.message,
          line: message.line ?? 0,
          fatal: true,
        });
        continue;
      }
      if (!message.ruleId || !message.ruleId.startsWith("react-hooks/")) continue;
      const source = message.source ?? lineText(result.filePath, message.line);
      items.push({
        file,
        rule: message.ruleId,
        message: message.message,
        source: source.replace(/\s+/g, " ").trim(),
        fingerprint: sourceFingerprint(source, `${message.line}:${message.column}:${message.message}`),
        count: 1,
      });
    }
  }
  return { items, fatals };
}

export async function collectHookDiagnostics(options = {}) {
  const root = options.root ?? DESKTOP_ROOT;
  const patterns = options.patterns ?? ["**/*.{ts,tsx}"];
  const eslint = new ESLint({
    cwd: root,
    allowInlineConfig: false,
    overrideConfigFile: true,
    overrideConfig: [
      {
        ignores: [
          "**/node_modules/**",
          "**/dist/**",
          "**/test-results/**",
          "src/bindings.ts",
        ],
      },
      {
        files: ["**/*.{ts,tsx}"],
        ...hooksPluginBlock(),
      },
    ],
    errorOnUnmatchedPattern: false,
  });
  const results = await eslint.lintFiles(patterns);
  return diagnosticsFromEslint(root, results);
}

function usage() {
  return "usage: check-hooks.mjs [--report] [--write-baseline] [--root dir] [--baseline file]";
}

function printFatals(fatals, io) {
  io.stderr.write(`hooks fatal/parser errors: ${fatals.length}\n`);
  for (const item of fatals) {
    io.stderr.write(`  ${item.file}:${item.line}  ${item.message}\n`);
  }
}

export async function runCheckHooks(argv, io = process) {
  let args;
  try {
    args = parseArgs(argv, {
      flags: ["report", "write-baseline"],
      values: ["root", "baseline"],
    });
  } catch (err) {
    io.stderr.write(`${err instanceof Error ? err.message : err}\n${usage()}\n`);
    return 1;
  }
  if (args.positional.length) {
    io.stderr.write(`unexpected argument: ${args.positional[0]}\n${usage()}\n`);
    return 1;
  }

  const root = path.resolve(args.values.root ?? DESKTOP_ROOT);
  const baselinePath = path.resolve(args.values.baseline ?? DEFAULT_HOOKS_BASELINE);
  const { items: actual, fatals } = await collectHookDiagnostics({ root });

  if (fatals.length) {
    printFatals(fatals, io);
    io.stderr.write("hooks check failed: parser/fatal errors cannot be baselined or ignored\n");
    return 1;
  }

  if (args.flags["write-baseline"]) {
    writeJson(baselinePath, {
      version: 1,
      note: "Stock react-hooks exemptions only. New mismatches fail. Stale entries fail. Do not auto-update.",
      items: actual,
    });
    io.stdout.write(`wrote ${actual.length} hook diagnostics to ${baselinePath}\n`);
    return 0;
  }

  if (args.flags.report) {
    io.stdout.write(`${JSON.stringify({ count: actual.length, items: actual }, null, 2)}\n`);
    return 0;
  }

  let baseline;
  try {
    baseline = loadBaseline(baselinePath);
  } catch (err) {
    io.stderr.write(`${err instanceof Error ? err.message : err}\n`);
    return 1;
  }

  const { added, stale } = diffBaseline(actual, baseline.items);
  if (added.length) printDiagnostics("hooks new (fail)", added, io.stdout);
  if (stale.length) printDiagnostics("hooks stale exemptions (fail)", stale, io.stdout);
  if (added.length || stale.length) {
    io.stderr.write(
      `hooks check failed: ${added.length} new, ${stale.length} stale; baseline is ${baselinePath}\n`,
    );
    return 1;
  }
  io.stdout.write(`hooks check ok: ${actual.length} stock, 0 new, 0 stale\n`);
  return 0;
}

if (isDirectRun(import.meta.url)) {
  const code = await runCheckHooks(process.argv.slice(2));
  process.exit(code);
}
