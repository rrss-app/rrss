#!/usr/bin/env bash
# 只在自托管 runner 上把编译和下载缓存放到持久卷。GitHub 托管立刻退出。
# 用法：prepare-runner.sh [node] [rust]
set -euo pipefail

if [[ "${RUNNER_ENVIRONMENT:-}" != "self-hosted" ]]; then
  exit 0
fi

cache="${RRSS_CI_CACHE:-/ci-cache}"
mkdir -p "$cache/cargo" "$cache/rustup" "$cache/target" "$cache/pnpm-store" "$cache/ms-playwright" "$cache/apt" "$cache/node"

if ! awk -v p="$cache" '$2 == p { found = 1 } END { exit !found }' /proc/mounts; then
  echo "::warning::${cache} 不是挂载卷，重建容器后 Cargo、pnpm 和浏览器缓存会丢。请加 -v rrss-ci-cache:${cache}，并且不要设 EPHEMERAL。"
fi

{
  echo "CARGO_HOME=$cache/cargo"
  echo "RUSTUP_HOME=$cache/rustup"
  echo "CARGO_TARGET_DIR=$cache/target"
  echo "APT_CACHE_DIR=$cache/apt"
  echo "npm_config_store_dir=$cache/pnpm-store"
  echo "PLAYWRIGHT_BROWSERS_PATH=$cache/ms-playwright"
} >> "${GITHUB_ENV:?}"

export CARGO_HOME="$cache/cargo"
export RUSTUP_HOME="$cache/rustup"

# 多个 runner 共用同一个缓存卷：冷缓存时几个 job 会同时下载 Node / 装 rustup，
# 一边 rm -rf 一边解压同一目录会互相删文件。整段准备持锁串行，脚本退出时随 fd 释放；
# 热缓存时只是几次版本检查，排队很短。
if command -v flock >/dev/null; then
  exec 9>"$cache/.prepare.lock"
  flock 9
else
  echo "::warning::没有 flock，共享缓存冷启动时并发 job 可能互相覆盖。"
fi

ensure_node() {
  local bindir="$cache/node/bin"
  # npm / pnpm 的 shebang 是 /usr/bin/env node。镜像里若有更旧的 node，会排在前面；
  # Node 22 自带的 npm 使用 node:path，旧 node 会报 MODULE_NOT_FOUND。
  export PATH="$bindir:$PATH"
  if [[ -x "$bindir/node" ]] && "$bindir/node" -v | grep -q '^v22\.'; then
    echo "node: $("$bindir/node" -v) (cached)"
  else
    local ver arch tar
    ver="$(curl -fsSL --retry 3 --retry-delay 2 https://nodejs.org/dist/index.json \
      | grep -oE '"version"[[:space:]]*:[[:space:]]*"v22\.[0-9]+\.[0-9]+"' \
      | head -1 | grep -oE 'v22\.[0-9]+\.[0-9]+')"
    [[ -n "$ver" ]] || { echo "::error::无法从 nodejs.org 解析 Node 22"; exit 1; }
    case "$(uname -m)" in
      x86_64) arch=linux-x64 ;;
      aarch64) arch=linux-arm64 ;;
      *) echo "::error::不支持的架构 $(uname -m)"; exit 1 ;;
    esac
    tar="node-${ver}-${arch}.tar.gz"
    echo "node: downloading $ver"
    curl -fL --retry 3 --retry-delay 2 -o "/tmp/${tar}" "https://nodejs.org/dist/${ver}/${tar}"
    # 先解压到旁边的临时目录，完整后再换上；中途失败不会留下半个 node 目录。
    rm -rf "$cache/node.tmp"
    mkdir -p "$cache/node.tmp"
    tar -xzf "/tmp/${tar}" -C "$cache/node.tmp" --strip-components=1
    rm -rf "$cache/node"
    mv "$cache/node.tmp" "$cache/node"
    rm -f "/tmp/${tar}"
  fi
  if ! "$bindir/pnpm" -v 2>/dev/null | grep -q '^10\.'; then
    "$bindir/node" "$cache/node/lib/node_modules/npm/bin/npm-cli.js" install -g pnpm@10
  fi
  echo "$bindir" >> "$GITHUB_PATH"
  echo "pnpm: $("$bindir/pnpm" -v)"
}

ensure_rust() {
  if [[ ! -x "$cache/cargo/bin/rustup" ]]; then
    echo "rustup: installing"
    curl --proto '=https' --tlsv1.2 --retry 3 --retry-delay 2 -fsSL \
      https://static.rust-lang.org/rustup/rustup-init.sh -o /tmp/rustup-init.sh
    sh /tmp/rustup-init.sh -y --no-modify-path --profile minimal --default-toolchain stable
    rm -f /tmp/rustup-init.sh
  else
    echo "rustup: cached $("$cache/cargo/bin/rustup" --version)"
  fi
  echo "$cache/cargo/bin" >> "$GITHUB_PATH"
}

for arg in "$@"; do
  case "$arg" in
    node) ensure_node ;;
    rust) ensure_rust ;;
    *) echo "::error::未知参数: $arg"; exit 1 ;;
  esac
done
