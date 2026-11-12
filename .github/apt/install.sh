#!/usr/bin/env bash
# CI 安装 Tauri 的系统依赖。软件列表与 .deb 放在 $APT_CACHE_DIR，由 actions/cache 在任务间保存：
# 有缓存时先离线安装（--no-download，不碰镜像）；缺包或版本对不上再联网，联网每次限时并最多重试 3 次。
# 用法：install.sh <包列表文件>
set -euo pipefail

packages_file="$1"
cache="${APT_CACHE_DIR:-$HOME/.apt-cache}"
mapfile -t packages < <(grep -v '^\s*#' "$packages_file" | grep -v '^\s*$')

mkdir -p "$cache/archives/partial" "$cache/lists/partial"
opts=(
  -o "Dir::Cache::Archives=$cache/archives"
  -o "Dir::State::Lists=$cache/lists"
  -o APT::Keep-Downloaded-Packages=true
  -o APT::Sandbox::User=root
  -o Acquire::Retries=3
  -o Acquire::http::Timeout=30
  -o Acquire::https::Timeout=30
  -o DPkg::Lock::Timeout=60
)

install() {
  sudo DEBIAN_FRONTEND=noninteractive timeout 240 \
    apt-get "${opts[@]}" install -y -qq --no-install-recommends "$@" "${packages[@]}"
}

finish() {
  # 只留仍能从当前列表下载到的包，免得缓存越积越大；归还属主，actions/cache 才能打包。
  sudo apt-get "${opts[@]}" autoclean -qq || true
  sudo chown -R "$(id -u):$(id -g)" "$cache"
}

sudo rm -f /var/lib/man-db/auto-update

if compgen -G "$cache/lists/*_Packages*" > /dev/null && install --no-download; then
  echo "apt: installed from cache without network"
  finish
  exit 0
fi

for attempt in 1 2 3; do
  if sudo timeout 120 apt-get "${opts[@]}" update -q && install; then
    echo "apt: installed from mirror (attempt $attempt)"
    finish
    exit 0
  fi
  echo "::warning::apt attempt $attempt failed or timed out; retrying"
  # 超时可能打断在 dpkg 中途，先收拾再重来。
  sudo dpkg --configure -a || true
  sleep 5
done
exit 1
