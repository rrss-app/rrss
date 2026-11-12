#!/usr/bin/env bash
# 编一份可独立运行的 Debug（见 CONTRIBUTING.md「运行」）。不要用 dev-desktop.sh 代替：那条强制跳过前端构建。
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  cat <<'EOF'
用法: scripts/build-debug.sh [--help]

  在仓库根目录执行 `cargo build --locked -p rrss-desktop`。
  不设置 RRSS_SKIP_FRONTEND：前端源比 dist 新时，build.rs 会先 pnpm build。
  产物: target/debug/rrss
  与 `scripts/dev-desktop.sh` 共用这个二进制；那个入口走 Vite，不会把界面打进包里。

EOF
}

for arg in "$@"; do
  case "$arg" in
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

unset RRSS_SKIP_FRONTEND
cd "$ROOT"
cargo build --locked -p rrss-desktop
echo "debug: $ROOT/target/debug/rrss"
