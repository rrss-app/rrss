import fs from "node:fs";
import path from "node:path";
import ts from "typescript";
import {
  DESKTOP_ROOT,
  diffBaseline,
  isDirectRun,
  loadBaseline,
  parseArgs,
  printDiagnostics,
  relPosix,
  sourceFingerprint,
  toPosix,
  writeJson,
} from "./cli.mjs";
import { classifyFile, isProductionRole, walkSourceFiles } from "./classify.mjs";

export const DEFAULT_DEPS_BASELINE = path.join(DESKTOP_ROOT, "tooling", "deps-baseline.json");

const CODE_EXT = new Set([".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs"]);
const ASSET_EXT = new Set([".css", ".svg", ".png", ".jpg", ".jpeg", ".webp", ".gif", ".json", ".md"]);

const NODE_BARE = new Set([
  "assert",
  "async_hooks",
  "buffer",
  "child_process",
  "cluster",
  "console",
  "constants",
  "crypto",
  "dgram",
  "diagnostics_channel",
  "dns",
  "domain",
  "events",
  "fs",
  "http",
  "http2",
  "https",
  "inspector",
  "module",
  "net",
  "os",
  "path",
  "perf_hooks",
  "process",
  "punycode",
  "querystring",
  "readline",
  "repl",
  "stream",
  "string_decoder",
  "sys",
  "timers",
  "tls",
  "trace_events",
  "tty",
  "url",
  "util",
  "v8",
  "vm",
  "wasi",
  "worker_threads",
  "zlib",
]);

const TEST_PACKAGES = new Set(["vitest", "@playwright/test", "playwright"]);

export function packageName(spec) {
  if (spec.startsWith("@")) {
    const parts = spec.split("/");
    return parts.slice(0, 2).join("/");
  }
  return spec.split("/")[0] ?? spec;
}

export function isNodeOnlySpec(spec) {
  if (spec.startsWith("node:")) return true;
  return NODE_BARE.has(packageName(spec));
}

export function isTestPackage(spec) {
  return TEST_PACKAGES.has(packageName(spec));
}

export function isTauriSpec(spec) {
  return spec === "@tauri-apps/api" || spec.startsWith("@tauri-apps/");
}

export function resolveRelative(fromFile, spec) {
  const abs = path.resolve(path.dirname(fromFile), spec);
  const ext = path.extname(abs);
  if (ASSET_EXT.has(ext)) return { kind: "asset", file: abs };
  const candidates = [];
  // `./dev-server.config` 里的 `.config` 不是扩展名，照样补全
  if (CODE_EXT.has(ext)) candidates.push(abs);
  else {
    candidates.push(`${abs}.ts`, `${abs}.tsx`, `${abs}.d.ts`, abs);
    candidates.push(path.join(abs, "index.ts"), path.join(abs, "index.tsx"));
  }
  for (const candidate of candidates) {
    if (fs.existsSync(candidate) && fs.statSync(candidate).isFile()) {
      return { kind: "file", file: candidate };
    }
  }
  return { kind: "unresolved", file: abs };
}

function scriptKind(filePath) {
  return path.extname(filePath) === ".tsx" ? ts.ScriptKind.TSX : ts.ScriptKind.TS;
}

function literalFrom(node) {
  if (!node) return null;
  if (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node)) return node.text;
  return null;
}

export function collectModuleRefs(filePath, text) {
  const sourceFile = ts.createSourceFile(filePath, text, ts.ScriptTarget.Latest, true, scriptKind(filePath));
  const refs = [];

  function add(kind, spec, node) {
    const start = sourceFile.getLineAndCharacterOfPosition(node.getStart());
    refs.push({
      kind,
      spec,
      line: start.line + 1,
      column: start.character + 1,
      source: node.getText(sourceFile),
    });
  }

  function visit(node) {
    if (ts.isImportDeclaration(node) && node.moduleSpecifier) {
      const spec = literalFrom(node.moduleSpecifier);
      if (spec !== null) add(node.importClause?.isTypeOnly ? "import-type" : "import", spec, node.moduleSpecifier);
    } else if (ts.isExportDeclaration(node) && node.moduleSpecifier) {
      const spec = literalFrom(node.moduleSpecifier);
      if (spec !== null) add("re-export", spec, node.moduleSpecifier);
    } else if (ts.isImportEqualsDeclaration(node) && ts.isExternalModuleReference(node.moduleReference)) {
      const spec = literalFrom(node.moduleReference.expression);
      if (spec !== null) add("import-equals", spec, node.moduleReference.expression);
      else add("import-equals-unknown", null, node);
    } else if (ts.isImportTypeNode(node)) {
      const argument = node.argument;
      if (ts.isLiteralTypeNode(argument)) {
        const spec = literalFrom(argument.literal);
        if (spec !== null) add("import-type-node", spec, argument.literal);
        else add("import-type-node-unknown", null, node);
      } else {
        add("import-type-node-unknown", null, node);
      }
    } else if (ts.isCallExpression(node)) {
      if (node.expression.kind === ts.SyntaxKind.ImportKeyword) {
        const spec = literalFrom(node.arguments[0]);
        if (spec !== null) add("dynamic-import", spec, node.arguments[0]);
        else add("dynamic-import-unknown", null, node);
      } else if (ts.isIdentifier(node.expression) && node.expression.text === "require") {
        const spec = literalFrom(node.arguments[0]);
        if (spec !== null) add("require", spec, node.arguments[0]);
        else add("require-unknown", null, node);
      }
    }
    ts.forEachChild(node, visit);
  }

  visit(sourceFile);
  return refs;
}

function featureOf(rel) {
  const match = /^src\/features\/([^/]+)\//.exec(rel);
  return match ? match[1] : null;
}

function isFeatureIndex(rel, feature) {
  return rel === `src/features/${feature}/index.ts` || rel === `src/features/${feature}/index.tsx`;
}

function isSharedLayer(layer) {
  return layer === "lib" || layer === "components" || layer === "shared";
}

function layerForbidden(fromLayer, toLayer) {
  if (isSharedLayer(fromLayer) && (toLayer === "app" || toLayer.startsWith("feature:"))) {
    return `${fromLayer} may not import ${toLayer}`;
  }
  if (fromLayer === "lib" && (toLayer === "components" || toLayer === "settings")) {
    return `lib may not import ${toLayer}`;
  }
  if (fromLayer === "components" && toLayer === "settings") {
    return `components may not import ${toLayer}`;
  }
  return null;
}

function nonProductionRule(to) {
  if (to.role === "test" || to.layer === "e2e" || to.layer === "perf") return "deps/test-import";
  return "deps/non-production";
}

function diagnostic(file, rule, message, source, to) {
  return {
    file,
    to: to ?? "",
    rule,
    message,
    source: String(source ?? "").replace(/\s+/g, " ").trim(),
    fingerprint: sourceFingerprint(source, `${file}:${to}:${rule}:${message}`),
    count: 1,
  };
}

export function collectDepDiagnostics(options = {}) {
  const root = options.root ?? DESKTOP_ROOT;
  const { files, unclassified } = walkSourceFiles(root);
  const items = [];

  for (const group of unclassified) {
    for (const file of group.files) {
      items.push(
        diagnostic(
          file,
          "deps/unclassified",
          `source in unclassified directory ${group.dir}; unknown dirs are not skipped`,
          group.dir,
          group.dir,
        ),
      );
    }
  }

  for (const abs of files) {
    const from = classifyFile(root, abs);
    if (from.role === "tooling") continue;
    const text = fs.readFileSync(abs, "utf8");
    const refs = collectModuleRefs(abs, text);
    for (const ref of refs) {
      if (ref.spec === null) {
        if (isProductionRole(from.role)) {
          items.push(
            diagnostic(from.rel, "deps/dynamic-unknown", `non-literal ${ref.kind} in production`, ref.source, ref.kind),
          );
        }
        continue;
      }

      const spec = ref.spec;
      const relative = spec.startsWith(".") || spec.startsWith("/");

      if (!relative) {
        if (isProductionRole(from.role) && isNodeOnlySpec(spec)) {
          items.push(diagnostic(from.rel, "deps/node-only", `production may not import Node-only ${spec}`, spec, spec));
        }
        if (isProductionRole(from.role) && isTestPackage(spec)) {
          items.push(diagnostic(from.rel, "deps/test-package", `production may not import ${spec}`, spec, spec));
        }
        if (isProductionRole(from.role) && isTauriSpec(spec) && from.rel !== "src/lib/api.ts") {
          items.push(
            diagnostic(
              from.rel,
              "deps/tauri-bypass",
              `production Tauri calls must go through src/lib/api.ts, not ${spec}`,
              spec,
              spec,
            ),
          );
        }
        continue;
      }

      const resolved = resolveRelative(abs, spec);
      if (resolved.kind === "asset") continue;
      if (resolved.kind === "unresolved") {
        items.push(
          diagnostic(from.rel, "deps/unresolved", `unresolved import ${spec}`, spec, toPosix(path.relative(root, resolved.file))),
        );
        continue;
      }

      const toRel = relPosix(root, resolved.file);
      const to = classifyFile(root, resolved.file);

      if (isProductionRole(from.role) && !isProductionRole(to.role)) {
        items.push(
          diagnostic(
            from.rel,
            nonProductionRule(to),
            `production may not import ${to.role} path ${toRel}`,
            spec,
            toRel,
          ),
        );
      }

      if (from.role === "production" && to.role === "production") {
        const layerMsg = layerForbidden(from.layer, to.layer);
        if (layerMsg) {
          items.push(diagnostic(from.rel, "deps/layer", layerMsg, spec, toRel));
        }

        const fromFeature = from.feature;
        const toFeature = to.feature ?? featureOf(toRel);
        if (toFeature && fromFeature !== toFeature && !isFeatureIndex(toRel, toFeature)) {
          items.push(
            diagnostic(
              from.rel,
              "deps/feature-deep",
              `must import feature "${toFeature}" through its public index, not ${toRel}`,
              spec,
              toRel,
            ),
          );
        }
      }
    }
  }

  return items;
}

function usage() {
  return "usage: check-deps.mjs [--report] [--write-baseline] [--root dir] [--baseline file]";
}

export function runCheckDeps(argv, io = process) {
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
  const baselinePath = path.resolve(args.values.baseline ?? DEFAULT_DEPS_BASELINE);
  const actual = collectDepDiagnostics({ root });

  if (args.flags["write-baseline"]) {
    writeJson(baselinePath, {
      version: 1,
      note: "Stock from→to import exemptions only. New edges fail. Stale entries fail. Do not auto-update.",
      items: actual,
    });
    io.stdout.write(`wrote ${actual.length} dependency diagnostics to ${baselinePath}\n`);
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
  if (added.length) printDiagnostics("deps new (fail)", added, io.stdout);
  if (stale.length) printDiagnostics("deps stale exemptions (fail)", stale, io.stdout);
  if (added.length || stale.length) {
    io.stderr.write(`deps check failed: ${added.length} new, ${stale.length} stale; baseline is ${baselinePath}\n`);
    return 1;
  }
  io.stdout.write(`deps check ok: ${actual.length} stock, 0 new, 0 stale\n`);
  return 0;
}

if (isDirectRun(import.meta.url)) {
  process.exit(runCheckDeps(process.argv.slice(2)));
}
