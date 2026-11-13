import fs from "node:fs";
import path from "node:path";
import { relPosix, toPosix } from "./cli.mjs";

export const KNOWN_TOP_DIRS = new Set([
  "src",
  "e2e",
  "perf",
  "tooling",
  "node_modules",
  "dist",
  "test-results",
  "public",
]);

export const KNOWN_SRC_DIRS = new Set([
  "app",
  "components",
  "lib",
  "settings",
  "shared",
  "styles",
  "test",
  "features",
  "i18n",
  "locales",
]);

export const IGNORE_DIR_NAMES = new Set([
  "node_modules",
  "dist",
  "test-results",
  "screenshots",
]);

const SOURCE_EXT = new Set([".ts", ".tsx"]);

export function isSourceFile(filePath) {
  return SOURCE_EXT.has(path.extname(filePath));
}

export function walkSourceFiles(root) {
  const files = [];
  const unclassified = [];

  function walk(dir, relativeFromRoot) {
    let entries;
    try {
      entries = fs.readdirSync(dir, { withFileTypes: true });
    } catch (err) {
      throw new Error(`cannot read ${dir}: ${err instanceof Error ? err.message : err}`);
    }
    for (const entry of entries) {
      const abs = path.join(dir, entry.name);
      const rel = relativeFromRoot ? `${relativeFromRoot}/${entry.name}` : entry.name;
      if (entry.isDirectory()) {
        if (IGNORE_DIR_NAMES.has(entry.name) || entry.name.startsWith(".")) continue;
        if (!relativeFromRoot && !KNOWN_TOP_DIRS.has(entry.name)) {
          const nested = walkSourceFilesIn(abs);
          if (nested.length > 0) {
            unclassified.push({ dir: rel, files: nested.map((file) => toPosix(path.relative(root, file))) });
          }
          continue;
        }
        if (relativeFromRoot === "src" && !KNOWN_SRC_DIRS.has(entry.name)) {
          const nested = walkSourceFilesIn(abs);
          if (nested.length > 0) {
            unclassified.push({ dir: rel, files: nested.map((file) => toPosix(path.relative(root, file))) });
          }
          continue;
        }
        walk(abs, rel);
        continue;
      }
      if (!entry.isFile() || !isSourceFile(abs)) continue;
      if (!relativeFromRoot) {
        files.push(abs);
        continue;
      }
      files.push(abs);
    }
  }

  walk(root, "");
  return { files, unclassified };
}

function walkSourceFilesIn(dir) {
  const out = [];
  const stack = [dir];
  while (stack.length) {
    const current = stack.pop();
    let entries;
    try {
      entries = fs.readdirSync(current, { withFileTypes: true });
    } catch {
      continue;
    }
    for (const entry of entries) {
      const abs = path.join(current, entry.name);
      if (entry.isDirectory()) {
        if (IGNORE_DIR_NAMES.has(entry.name) || entry.name.startsWith(".")) continue;
        stack.push(abs);
      } else if (entry.isFile() && isSourceFile(abs)) {
        out.push(abs);
      }
    }
  }
  return out;
}

export function classifyFile(root, filePath) {
  const rel = relPosix(root, filePath);
  const parts = rel.split("/");
  const base = parts.at(-1) ?? rel;
  const testLike = /\.(test|spec)\.(ts|tsx)$/.test(base);

  if (parts[0] === "tooling") {
    return { rel, role: "tooling", layer: "tooling", feature: null, isIndex: false };
  }
  if (parts.length === 1 && /\.config\.(ts|tsx)$/.test(base)) {
    return { rel, role: "config", layer: "config", feature: null, isIndex: false };
  }
  if (parts[0] === "e2e" || testLike || (parts[0] === "src" && parts[1] === "test") || parts[0] === "perf") {
    const layer = parts[0] === "e2e" ? "e2e" : parts[0] === "perf" ? "perf" : "test";
    return { rel, role: "test", layer, feature: null, isIndex: false };
  }
  if (parts[0] !== "src") {
    return { rel, role: "unknown", layer: "unknown", feature: null, isIndex: false };
  }

  if (parts.length === 2 && (base === "App.tsx" || base === "main.tsx")) {
    return { rel, role: "production", layer: "app", feature: null, isIndex: false };
  }
  if (parts.length === 2 && base === "bindings.ts") {
    return { rel, role: "production", layer: "bindings", feature: null, isIndex: false };
  }
  if (parts.length === 2 && base.endsWith(".d.ts")) {
    return { rel, role: "production", layer: "types", feature: null, isIndex: false };
  }
  const srcDir = parts[1];
  if (srcDir && !KNOWN_SRC_DIRS.has(srcDir)) {
    return { rel, role: "unknown", layer: "unknown-src", feature: null, isIndex: false };
  }
  if (srcDir === "app") {
    return { rel, role: "production", layer: "app", feature: null, isIndex: false };
  }
  // 语言包与取文案的入口：各层都能用，自己不依赖业务代码。
  if (srcDir === "i18n" || srcDir === "locales") {
    return { rel, role: "production", layer: "shared", feature: null, isIndex: false };
  }
  if (srcDir === "shared") {
    return { rel, role: "production", layer: "shared", feature: null, isIndex: false };
  }
  if (srcDir === "lib") {
    return { rel, role: "production", layer: "lib", feature: null, isIndex: false };
  }
  if (srcDir === "components") {
    return { rel, role: "production", layer: "components", feature: null, isIndex: false };
  }
  if (srcDir === "settings") {
    return { rel, role: "production", layer: "settings", feature: null, isIndex: false };
  }
  if (srcDir === "styles") {
    return { rel, role: "production", layer: "styles", feature: null, isIndex: false };
  }
  if (srcDir === "features") {
    const feature = parts[2];
    if (!feature) {
      return { rel, role: "production", layer: "features", feature: null, isIndex: false };
    }
    const isIndex = parts.length === 4 && /^index\.(ts|tsx)$/.test(base);
    return { rel, role: "production", layer: `feature:${feature}`, feature, isIndex };
  }
  return { rel, role: "unknown", layer: "unknown-src", feature: null, isIndex: false };
}

export function isProductionRole(role) {
  return role === "production";
}
