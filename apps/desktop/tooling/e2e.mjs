import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import { DESKTOP_ROOT, isDirectRun } from "./cli.mjs";

export const LOCAL_TEST_LIMIT = 10;

/** Count collected executions, including projects, repeats and possible retries, without launching a browser. */
export function collectedTests(report) {
  if (!Array.isArray(report.suites) || report.errors?.length) throw new Error("Playwright collection failed");
  const retries = new Map((report.config?.projects ?? []).map((project) => [project.name, project.retries ?? 0]));
  const tests = [];
  function visit(suite) {
    for (const spec of suite.specs ?? []) {
      for (const test of spec.tests) {
        tests.push({ file: spec.file, line: spec.line, title: spec.title, executions: 1 + (retries.get(test.projectName) ?? 0) });
      }
    }
    for (const child of suite.suites ?? []) visit(child);
  }
  for (const suite of report.suites) visit(suite);
  return tests;
}

export function parseE2eArgs(argv) {
  const args = [];
  let full = false;
  let dry = false;
  let limit = LOCAL_TEST_LIMIT;
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    // A separator before our --list would turn it into a file filter and execute tests during preflight.
    if (arg === "--") continue;
    if (/^--(?:ui(?:-host|-port)?|debug)(?:=|$)/.test(arg)) throw new Error("Interactive modes are not supported by the bounded e2e runner");
    if (arg === "--full") {
      if (i !== 0) throw new Error("--full must be the first argument");
      full = true;
    } else if (arg === "--dry-run") dry = true;
    else if (arg === "--max-tests" || arg.startsWith("--max-tests=")) {
      const value = arg === "--max-tests" ? argv[++i] : arg.slice("--max-tests=".length);
      if (!/^\d+$/.test(value ?? "") || !Number.isSafeInteger(Number(value)) || Number(value) < 1) {
        throw new Error("--max-tests requires a positive integer");
      }
      limit = Number(value);
    } else args.push(arg);
  }
  return { args, full, dry, limit };
}

/** All standard local e2e entry points pass through this budget before starting Vite or Chromium. */
export function runE2e(argv = process.argv.slice(2), {
  spawn = spawnSync, stdout = process.stdout, stderr = process.stderr,
  cli = createRequire(import.meta.url).resolve("@playwright/test/cli"),
} = {}) {
  try {
    const { args, full, dry, limit } = parseE2eArgs(argv);
    const defaults = full ? [] : ["--workers=2"];
    const invoke = (extra, capture = false) => spawn(process.execPath, [cli, "test", ...defaults, ...extra], {
      cwd: DESKTOP_ROOT, stdio: capture ? ["ignore", "pipe", "pipe"] : "inherit",
      encoding: "utf8", maxBuffer: 16 * 1024 * 1024,
    });
    // --list / --help never run tests; an explicit --full is reserved for CI and requested full regressions.
    if (args.includes("--list") || args.includes("--help") || (full && !dry)) {
      return invoke(["--reporter=line", ...args]).status ?? 1;
    }
    if (!args.length && !full && !dry) {
      stderr.write("Specify an affected spec/case, or use test:affected --dry-run. Bare test:e2e does not run the full suite.\n");
      return 1;
    }
    const listed = invoke([...args, "--list", "--reporter=json"], true);
    if (listed.status !== 0) {
      stderr.write(listed.stderr || listed.stdout || "Playwright collection failed\n");
      return listed.status ?? 1;
    }
    const tests = collectedTests(JSON.parse(listed.stdout));
    const count = tests.reduce((sum, test) => sum + test.executions, 0);
    stdout.write(`e2e: ${count} planned executions (including retries); local limit ${limit}.\n`);
    if (!tests.length) {
      stderr.write("No matching e2e tests. Check the spec, line or --grep filter.\n");
      return 1;
    }
    if (dry || (!full && count > limit)) {
      const groups = new Map();
      for (const test of tests) groups.set(test.file, (groups.get(test.file) ?? 0) + test.executions);
      for (const [file, count] of groups) stdout.write(`  ${file}: ${count}\n`);
    }
    if (!full && count > limit) {
      stderr.write(`E2E budget exceeded (${count} > ${limit}); no browser started. Narrow to spec.ts:line or --grep. Full regression belongs in CI; use --max-tests N only for an intentional larger run.\n`);
      return 1;
    }
    if (dry) return 0;
    return invoke(["--reporter=line", ...args]).status ?? 1;
  } catch (error) {
    stderr.write(`${error instanceof Error ? error.message : error}\n`);
    return 1;
  }
}

if (isDirectRun(import.meta.url)) process.exit(runE2e());
