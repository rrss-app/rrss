#!/usr/bin/env bash
# 自托管 runner 缓存与离线复用辅助脚本；GitHub 托管 runner 按子命令安全跳过或走默认路径。
# 用法：
#   prepare-runner.sh [--target-scope <name>] [node] [rust]
#   prepare-runner.sh pnpm-install
#   prepare-runner.sh ensure-playwright <version>
#   prepare-runner.sh ensure-rust-target <target>
#   prepare-runner.sh fetch-rust
#   prepare-runner.sh cache-mold
#   prepare-runner.sh clean-target [--keep-workspace N] [--keep-deps N] [--max-age-days N]
set -euo pipefail

is_self_hosted() {
  [[ "${RUNNER_ENVIRONMENT:-}" == "self-hosted" ]]
}

cache="${RRSS_CI_CACHE:-/ci-cache}"
runner_cache="${RRSS_CI_RUNNER_CACHE:-$cache}"

with_lock() {
  local lock_file="$1"
  shift
  if command -v flock >/dev/null 2>&1; then
    mkdir -p "$(dirname "$lock_file")"
    (
      exec 9>"$lock_file"
      flock 9
      "$@"
    )
  else
    "$@"
  fi
}

cmd_pnpm_install() {
  if is_self_hosted; then
    if pnpm install --frozen-lockfile --offline; then
      echo "pnpm: installed offline from local store"
      return 0
    fi
    echo "pnpm: offline install missed new packages, falling back to --prefer-offline"
  fi
  pnpm install --frozen-lockfile --prefer-offline
}

playwright_expected_rev() {
  if ! command -v node >/dev/null 2>&1; then
    return 1
  fi
  node -e '
    const fs = require("fs");
    const path = require("path");
    const { createRequire } = require("module");
    try {
      const r1 = createRequire(path.resolve("package.json"));
      const pw = r1.resolve("@playwright/test");
      const r2 = createRequire(pw);
      const coreDir = path.dirname(r2.resolve("playwright-core"));
      const data = JSON.parse(fs.readFileSync(path.join(coreDir, "browsers.json"), "utf8"));
      const entry = (data.browsers || []).find((b) => b.name === "chromium-headless-shell");
      if (entry && entry.revision) process.stdout.write(String(entry.revision));
    } catch {}
  ' 2>/dev/null
}

playwright_browser_ready() {
  local browsers_dir="$1"
  local ver="$2"
  [[ -d "$browsers_dir" ]] || return 1
  local rev
  rev="$(playwright_expected_rev || true)"
  if [[ -n "$rev" ]]; then
    [[ -d "$browsers_dir/chromium_headless_shell-${rev}" ]]
    return $?
  fi
  [[ -f "$browsers_dir/.browser-ready-${ver}" ]] &&
    find "$browsers_dir" -maxdepth 2 -type d -name 'chromium_headless_shell-*' | grep -q .
}

playwright_sysdeps_ready() {
  local ver="$1"
  local marker="${RRSS_PW_MARKER_DIR:-/var/tmp}/.rrss-pw-deps-${ver}"
  if [[ -f "$marker" ]]; then
    return 0
  fi
  # 核心 Chromium 运行库已装齐时直接标记就绪，避免每次 job 重复 apt-get update。
  local pkgs=(libnss3 libgbm1 libasound2 libatk-bridge2.0-0 libxkbcommon0)
  local status
  if command -v dpkg-query >/dev/null 2>&1 &&
    status="$(dpkg-query -W -f='${db:Status-Status}\n' -- "${pkgs[@]}" 2>/dev/null)" &&
    [[ -n "$status" ]] && ! grep -qvx installed <<< "$status"; then
    mkdir -p "$(dirname "$marker")" 2>/dev/null || true
    touch "$marker" 2>/dev/null || true
    return 0
  fi
  return 1
}

ensure_playwright_locked() {
  local ver="$1"
  local browsers_dir="$2"
  local marker="${RRSS_PW_MARKER_DIR:-/var/tmp}/.rrss-pw-deps-${ver}"
  local has_browser=0
  local has_deps=0
  playwright_browser_ready "$browsers_dir" "$ver" && has_browser=1 || true
  playwright_sysdeps_ready "$ver" && has_deps=1 || true

  if [[ "$has_browser" -eq 1 && "$has_deps" -eq 1 ]]; then
    echo "playwright: Chromium ${ver} and system deps already ready (0 network)"
    return 0
  fi
  if [[ "$has_browser" -eq 1 ]]; then
    echo "playwright: Chromium ${ver} cached, installing missing system deps only"
    pnpm exec playwright install-deps chromium
    touch "$marker" 2>/dev/null || true
    return 0
  fi
  if [[ "$has_deps" -eq 1 ]]; then
    echo "playwright: system deps ready, downloading Chromium ${ver} shell only"
    pnpm exec playwright install --only-shell chromium
    touch "$browsers_dir/.browser-ready-${ver}" 2>/dev/null || true
    return 0
  fi
  pnpm exec playwright install --with-deps --only-shell chromium
  touch "$browsers_dir/.browser-ready-${ver}" 2>/dev/null || true
  touch "$marker" 2>/dev/null || true
}

cmd_ensure_playwright() {
  local ver="${1:-unknown}"
  if ! is_self_hosted; then
    pnpm exec playwright install --with-deps --only-shell chromium
    return 0
  fi
  local browsers_dir="${PLAYWRIGHT_BROWSERS_PATH:-$runner_cache/ms-playwright}"
  mkdir -p "$browsers_dir"
  if playwright_browser_ready "$browsers_dir" "$ver" && playwright_sysdeps_ready "$ver"; then
    echo "playwright: Chromium ${ver} and system deps already ready (0 network)"
    return 0
  fi
  with_lock "$browsers_dir/.install.lock" ensure_playwright_locked "$ver" "$browsers_dir"
}

ensure_rust_target_locked() {
  local target="$1"
  if rustup target list --installed | grep -qx "$target"; then
    echo "rustup: target ${target} already installed (0 network)"
    return 0
  fi
  rustup target add "$target"
}

cmd_ensure_rust_target() {
  local target="${1:?missing target triple}"
  if ! is_self_hosted; then
    return 0
  fi
  if rustup target list --installed | grep -qx "$target"; then
    echo "rustup: target ${target} already installed (0 network)"
    return 0
  fi
  local rustup_home="${RUSTUP_HOME:-$cache/rustup}"
  with_lock "$rustup_home/.targets.lock" ensure_rust_target_locked "$target"
}

cmd_fetch_rust() {
  if ! is_self_hosted; then
    return 0
  fi
  if cargo fetch --locked --offline 2>/dev/null; then
    echo "cargo: all locked crates cached locally (0 network)"
  else
    echo "cargo: fetching missing crates from configured registry"
    with_lock "$cache/.cargo-fetch.lock" cargo fetch --locked
  fi
  if [[ -n "${GITHUB_ENV:-}" ]]; then
    echo "CARGO_NET_OFFLINE=true" >> "$GITHUB_ENV"
  fi
}

cache_mold_locked() {
  local src="$1"
  local dst="$cache/mold/bin/mold"
  mkdir -p "$(dirname "$dst")"
  cp -f "$src" "$dst.tmp"
  chmod +x "$dst.tmp"
  mv -f "$dst.tmp" "$dst"
  echo "mold: cached $src -> $dst"
}

cmd_cache_mold() {
  if ! is_self_hosted; then
    return 0
  fi
  if [[ -x "$cache/mold/bin/mold" ]]; then
    return 0
  fi
  local src
  src="$(command -v mold 2>/dev/null || true)"
  if [[ -z "$src" || ! -x "$src" ]]; then
    return 0
  fi
  with_lock "$cache/.prepare.lock" cache_mold_locked "$src"
}

prune_build_crate_hashes() {
  local crate_dir="$1"
  local keep="$2"
  local min_age_days="$3"
  [[ -d "$crate_dir" ]] || return 0

  # 按 mtime 倒序列出子目录，保留最新 keep 个；超出的若满足年龄条件则删除。
  local idx=0
  local dir
  while IFS= read -r dir; do
    [[ -n "$dir" ]] || continue
    idx=$((idx + 1))
    if (( idx <= keep )); then
      continue
    fi
    if (( min_age_days <= 0 )) || find "$dir" -maxdepth 0 -mtime +"$min_age_days" | grep -q .; then
      rm -rf "$dir"
    fi
  done < <(find "$crate_dir" -mindepth 1 -maxdepth 1 -type d -printf '%T@\t%p\n' 2>/dev/null | sort -rn | cut -f2-)
}

clean_target_locked() {
  local target_dir="$1"
  local keep_ws="$2"
  local keep_deps="$3"
  local max_age_days="$4"
  local ws_crates=(rrss-core rrss-focus rrss-render rrss-ai rrss-app rrss-desktop)

  local profile build_root crate_dir crate_name
  for profile in debug release x86_64-pc-windows-gnu/debug x86_64-pc-windows-gnu/release; do
    build_root="$target_dir/$profile/build"
    if [[ -d "$build_root" ]]; then
      for crate_dir in "$build_root"/*; do
        [[ -d "$crate_dir" ]] || continue
        crate_name="$(basename "$crate_dir")"
        case " ${ws_crates[*]} " in
          *" $crate_name "*)
            # 自家 crate 每次改代码都会产生新测试/编译 hash，只留最新几份防止无限累积。
            prune_build_crate_hashes "$crate_dir" "$keep_ws" 0
            ;;
          *)
            # 第三方依赖保留最新多份（覆盖多 feature 组合），仅清超出且超过天数的历史旧 hash。
            prune_build_crate_hashes "$crate_dir" "$keep_deps" "$max_age_days"
            ;;
        esac
      done
    fi

    # 兼容旧 Cargo 布局（debug/deps）：清超过 2 天的自家旧测试二进制与 rlib。
    local deps_root="$target_dir/$profile/deps"
    if [[ -d "$deps_root" ]]; then
      find "$deps_root" -maxdepth 1 -type f \
        \( -name 'rrss_*' -o -name 'librrss_*' -o -name 'rrss-*' \) \
        -mtime +2 -delete 2>/dev/null || true
    fi
  done
  echo "target-clean: pruned stale artifacts in $target_dir (keep_ws=$keep_ws, keep_deps=$keep_deps, max_age=${max_age_days}d)"
}

cmd_clean_target() {
  if ! is_self_hosted; then
    return 0
  fi
  local keep_ws="${RRSS_CI_KEEP_WORKSPACE_HASHES:-6}"
  local keep_deps="${RRSS_CI_KEEP_DEPS_HASHES:-8}"
  local max_age_days="${RRSS_CI_TARGET_MAX_AGE_DAYS:-14}"
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --keep-workspace) keep_ws="$2"; shift 2 ;;
      --keep-deps) keep_deps="$2"; shift 2 ;;
      --max-age-days) max_age_days="$2"; shift 2 ;;
      *) echo "::error::未知参数: $1"; exit 1 ;;
    esac
  done
  local target_dir="${CARGO_TARGET_DIR:-}"
  if [[ -z "$target_dir" || ! -d "$target_dir" ]]; then
    return 0
  fi
  with_lock "$target_dir/.clean.lock" clean_target_locked "$target_dir" "$keep_ws" "$keep_deps" "$max_age_days"
}

case "${1:-}" in
  pnpm-install) shift; cmd_pnpm_install "$@"; exit 0 ;;
  ensure-playwright) shift; cmd_ensure_playwright "$@"; exit 0 ;;
  ensure-rust-target) shift; cmd_ensure_rust_target "$@"; exit 0 ;;
  fetch-rust) shift; cmd_fetch_rust "$@"; exit 0 ;;
  cache-mold) shift; cmd_cache_mold "$@"; exit 0 ;;
  clean-target) shift; cmd_clean_target "$@"; exit 0 ;;
esac

if ! is_self_hosted; then
  exit 0
fi

target_scope=""
components=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --target-scope) target_scope="$2"; shift 2 ;;
    node|rust) components+=("$1"); shift ;;
    *) echo "::error::未知参数: $1"; exit 1 ;;
  esac
done

target_dir="$runner_cache/target"
if [[ "${RRSS_CI_SHARED_TARGET:-0}" == "1" && -n "$target_scope" ]]; then
  target_dir="$cache/target-$target_scope"
fi

if [[ "${RRSS_CI_SHARED_CACHE_ASSETS:-0}" == "1" ]]; then
  playwright_dir="$cache/ms-playwright"
  apt_dir="$cache/apt"
else
  playwright_dir="$runner_cache/ms-playwright"
  apt_dir="$runner_cache/apt"
fi

mkdir -p "$cache/cargo" "$cache/rustup" "$cache/pnpm-store" "$cache/node" "$cache/mold/bin" \
  "$target_dir" "$playwright_dir" "$apt_dir"

if ! awk -v p="$cache" '$2 == p { found = 1 } END { exit !found }' /proc/mounts; then
  echo "::warning::${cache} 不是挂载卷，重建容器后 Cargo、pnpm 和浏览器缓存会丢。请加 -v rrss-ci-cache:${cache}，并且不要设 EPHEMERAL。"
fi

{
  echo "CARGO_HOME=$cache/cargo"
  echo "RUSTUP_HOME=$cache/rustup"
  echo "CARGO_TARGET_DIR=$target_dir"
  echo "APT_CACHE_DIR=$apt_dir"
  echo "npm_config_store_dir=$cache/pnpm-store"
  echo "PLAYWRIGHT_BROWSERS_PATH=$playwright_dir"
} >> "${GITHUB_ENV:?}"

export CARGO_HOME="$cache/cargo"
export RUSTUP_HOME="$cache/rustup"

# 多个 runner 共用同一个缓存卷：冷缓存时几个 job 会同时下载 Node / 装 rustup，
# 一边 rm -rf 一边解压同一目录会互相删文件。整段准备持锁串行，脚本退出时随 fd 释放；
# 热缓存时只是几次版本检查，排队很短。
if command -v flock >/dev/null 2>&1; then
  exec 9>"$cache/.prepare.lock"
  flock 9
else
  echo "::warning::没有 flock，共享缓存冷启动时并发 job 可能互相覆盖。"
fi

ensure_node() {
  local bindir="$cache/node/bin"
  local dist_url="${RRSS_NODE_DIST_URL:-https://nodejs.org/dist}"
  dist_url="${dist_url%/}"
  # npm / pnpm 的 shebang 是 /usr/bin/env node。镜像里若有更旧的 node，会排在前面；
  # Node 22 自带的 npm 使用 node:path，旧 node 会报 MODULE_NOT_FOUND。
  export PATH="$bindir:$PATH"
  if [[ -x "$bindir/node" ]] && "$bindir/node" -v | grep -q '^v22\.'; then
    echo "node: $("$bindir/node" -v) (cached)"
  else
    local ver arch tar
    ver="$(curl -fsSL --retry 3 --retry-delay 2 "${dist_url}/index.json" \
      | grep -oE '"version"[[:space:]]*:[[:space:]]*"v22\.[0-9]+\.[0-9]+"' \
      | head -1 | grep -oE 'v22\.[0-9]+\.[0-9]+')"
    [[ -n "$ver" ]] || { echo "::error::无法从 ${dist_url} 解析 Node 22"; exit 1; }
    case "$(uname -m)" in
      x86_64) arch=linux-x64 ;;
      aarch64) arch=linux-arm64 ;;
      *) echo "::error::不支持的架构 $(uname -m)"; exit 1 ;;
    esac
    tar="node-${ver}-${arch}.tar.gz"
    echo "node: downloading $ver from ${dist_url}"
    curl -fL --retry 3 --retry-delay 2 -o "/tmp/${tar}" "${dist_url}/${ver}/${tar}"
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

  if [[ -x "$cache/mold/bin/mold" ]]; then
    echo "$cache/mold/bin" >> "$GITHUB_PATH"
    echo "RRSS_MOLD_READY=1" >> "$GITHUB_ENV"
    echo "mold: $("$cache/mold/bin/mold" --version) (cached)"
  elif command -v mold >/dev/null 2>&1; then
    echo "RRSS_MOLD_READY=1" >> "$GITHUB_ENV"
    echo "mold: $(mold --version) (system)"
  fi
}

for arg in "${components[@]}"; do
  case "$arg" in
    node) ensure_node ;;
    rust) ensure_rust ;;
  esac
done
