import { createHash } from "node:crypto";
import { statSync } from "node:fs";
import { fileURLToPath } from "node:url";

/** Vite 开发服务器与 e2e 共用的地址；多个检出并行开发时端口按路径错开（可用 RRSS_DEV_PORT 覆盖）。
 * 端口依次取：`tauri dev` 下固定 1420（与 crates/rrss-desktop/tauri.dev.conf.json 的 devUrl 一致）；RRSS_DEV_PORT；
 * git 附属 worktree 按路径落在 1440–1499；主检出 1420。 */
const desktopRoot = fileURLToPath(new URL(".", import.meta.url));
const repoRoot = fileURLToPath(new URL("../..", import.meta.url));
const raw = process.env.RRSS_DEV_PORT;

function isLinkedWorktree(): boolean {
  try {
    // 附属 worktree 的 .git 是指向主仓库的文件
    return statSync(`${repoRoot}.git`).isFile();
  } catch {
    return false;
  }
}

function hash(text: string): string {
  return createHash("sha1").update(text).digest("hex");
}

function pickPort(): number {
  if (process.env.TAURI_ENV_PLATFORM) return 1420;
  if (raw) return Number(raw);
  if (!isLinkedWorktree()) return 1420;
  return 1440 + (parseInt(hash(repoRoot).slice(0, 8), 16) % 60);
}

export const DEV_PORT = pickPort();
if (!Number.isInteger(DEV_PORT) || DEV_PORT < 1 || DEV_PORT > 65535) {
  throw new Error(`RRSS_DEV_PORT 不是有效端口：${raw}`);
}
export const DEV_ORIGIN = `http://127.0.0.1:${DEV_PORT}`;

/** 只有本检出的 Vite 对这个路径回 200；e2e 据此只复用自己的服务器，端口被别的检出占用时报错而不是测错代码。 */
export const DEV_ID_PREFIX = "/__rrss_dev/";
export const DEV_ID_PATH = `${DEV_ID_PREFIX}${hash(desktopRoot).slice(0, 12)}`;
