#!/usr/bin/env bash
# RRSS 日常开发：Tauri Debug + Vite HMR（见 CONTRIBUTING.md「运行」）
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TAURI_BIN="$ROOT/apps/desktop/node_modules/.bin/tauri"
DESKTOP_CRATE="$ROOT/crates/rrss-desktop"
CONFIG_DEV="$DESKTOP_CRATE/tauri.dev.conf.json"

usage() {
  cat <<'EOF'
用法: scripts/dev-desktop.sh [--mcp] [--help]

  在 crates/rrss-desktop 下执行 `tauri dev`，合并 tauri.dev.conf.json（Vite :1420 + HMR）。
  默认使用本机真实数据目录（与 release 相同）；隔离库见 RRSS_DEV_ISOLATE。

  --mcp    启用 Cargo feature mcp-debug，并合并 app.withGlobalTauri=true（仅开发）
  --help   本说明

环境变量:
  RRSS_SKIP_FRONTEND=1     由本脚本强制设置，避免 build.rs 在 dev 循环里跑 pnpm build
  RRSS_DEV_ISOLATE=1       将 XDG_DATA_HOME 指到仓库内 .local/rrss-dev-xdg-data（仅 Linux/WSL）
  RRSS_DEV_XDG_DATA_HOME   自定义隔离根（仍会在其下使用 rrss/ 子目录，见 directories）
  RRSS_LOG                 传给应用（例: rrss_app=debug,rrss_core=debug）

EOF
}

MCP=0
for arg in "$@"; do
  case "$arg" in
    --mcp) MCP=1 ;;
    --help | -h)
      usage
      exit 0
      ;;
    *)
      echo "未知参数: $arg" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ ! -x "$TAURI_BIN" ]]; then
  echo "缺少 Tauri CLI: $TAURI_BIN — 先在 apps/desktop 执行 pnpm install" >&2
  exit 1
fi

if [[ ! -f "$CONFIG_DEV" ]]; then
  echo "缺少开发配置: $CONFIG_DEV" >&2
  exit 1
fi

export RRSS_SKIP_FRONTEND=1

if [[ "${RRSS_DEV_ISOLATE:-}" == "1" ]]; then
  export XDG_DATA_HOME="${RRSS_DEV_XDG_DATA_HOME:-$ROOT/.local/rrss-dev-xdg-data}"
  echo "dev: 隔离数据 XDG_DATA_HOME=$XDG_DATA_HOME → …/rrss/rrss.db"
else
  echo "dev: 使用本机默认数据目录（~/.local/share/rrss/ 等，与 release 相同）"
fi

cd "$DESKTOP_CRATE"

TAURI_ARGS=(dev --config "$CONFIG_DEV")

if [[ "$MCP" == "1" ]]; then
  TAURI_ARGS+=(--features mcp-debug --config "$DESKTOP_CRATE/tauri.mcp.conf.json")
fi

exec "$TAURI_BIN" "${TAURI_ARGS[@]}"
