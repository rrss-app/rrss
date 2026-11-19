#!/usr/bin/env bash
# CI 安装 Tauri 的系统依赖。软件列表与 .deb 放在 $APT_CACHE_DIR，由 actions/cache 在任务间保存：
# 有缓存时先离线安装；缺包时分开更新索引、下载和离线安装，最多联网尝试 3 次。
# 所有阶段共用 13 分钟预算，给 workflow 的 15 分钟上限留出退出余量。
# 用法：install.sh <包列表文件>
set -euo pipefail

deadline=$((SECONDS + 780))
packages_file="$1"
cache="${APT_CACHE_DIR:-$HOME/.apt-cache}"
mapfile -t packages < <(grep -v '^\s*#' "$packages_file" | grep -v '^\s*$')

if [[ "${RUNNER_ENVIRONMENT:-}" == self-hosted ]] &&
  installed=$(dpkg-query -W -f='${db:Status-Status}\n' -- "${packages[@]}" 2>/dev/null) &&
  [[ -n $installed ]] && ! grep -qvx installed <<< "$installed"; then
  echo "apt: required packages already installed"
  exit 0
fi

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

run_stage() {
  local stage="$1" limit="$2" remaining status outcome
  shift 2
  # 预留 timeout 强制结束子进程的 5 秒；不能让重试重新获得整份预算。
  remaining=$((deadline - SECONDS - 5))
  if (( remaining <= 0 )); then
    echo "::error::apt stage=$stage outcome=budget_exhausted exit_code=124"
    return 124
  fi
  if (( limit > remaining )); then limit=$remaining; fi
  echo "::group::apt stage=$stage timeout_secs=$limit"
  if sudo DEBIAN_FRONTEND=noninteractive timeout --kill-after=5 "$limit" "$@"; then
    status=0
    outcome=success
  else
    status=$?
    case "$status" in
      124) outcome=timeout ;;
      137) outcome=killed ;;
      *) outcome=failed ;;
    esac
  fi
  echo "apt stage=$stage outcome=$outcome exit_code=$status"
  echo "::endgroup::"
  if (( status != 0 )); then
    echo "::warning::apt stage=$stage outcome=$outcome exit_code=$status"
  fi
  return "$status"
}

install() {
  run_stage "$1" 240 apt-get "${opts[@]}" install -y -q \
    --no-download --no-install-recommends "${packages[@]}"
}

recover() {
  # 只在安装失败后恢复；恢复失败就停止，避免继续操作未配置完成的软件包。
  run_stage recover 60 dpkg --configure -a
}

finish() {
  # 只留仍能从当前列表下载到的包，免得缓存越积越大；归还属主，actions/cache 才能打包。
  run_stage autoclean 30 apt-get "${opts[@]}" autoclean -q || true
  run_stage cache_permissions 15 chown -R "$(id -u):$(id -g)" "$cache"
}

run_stage prepare 10 rm -f /var/lib/man-db/auto-update

if compgen -G "$cache/lists/*_Packages*" > /dev/null; then
  if install cache_install; then
    echo "apt: installed from cache without network"
    finish
    exit 0
  fi
  recover || exit "$?"
fi

status=1
for attempt in 1 2 3; do
  if run_stage "update_$attempt" 120 apt-get "${opts[@]}" update -q &&
    run_stage "download_$attempt" 180 apt-get "${opts[@]}" install -y -q \
      --download-only --no-install-recommends "${packages[@]}"; then
    if install "install_$attempt"; then
      echo "apt: installed from mirror (attempt $attempt)"
      finish
      exit 0
    else
      status=$?
    fi
    recover || exit "$?"
  else
    status=$?
  fi
  # 最后一次失败直接退出；预算不足时不再睡眠或启动下一次尝试。
  if (( attempt == 3 || deadline - SECONDS <= 10 )); then break; fi
  echo "::warning::apt attempt=$attempt exit_code=$status; retrying"
  sleep 5
done
echo "::error::apt installation failed exit_code=$status"
exit "$status"
