#!/usr/bin/env bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

#
# 在 Docker ubuntu:24.04 中运行指定的 E2E 测试脚本。
# 用法: bash scripts/docker-e2e-test.sh [test_script_name]
# 示例: bash scripts/docker-e2e-test.sh test_remote_file_relay_e2e.sh
#
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TEST_SCRIPT="${1:-test_remote_file_relay_e2e.sh}"

echo "==> 准备在 Docker ubuntu:24.04 中运行: $TEST_SCRIPT"

# 构建一个包含 Rust + Node.js 环境的镜像
DOCKER_IMAGE="bifrost-e2e-ubuntu"

docker build -t "$DOCKER_IMAGE" -f - "$ROOT_DIR" <<'DOCKERFILE'
FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive
ENV CARGO_HOME=/cargo
ENV RUSTUP_HOME=/rustup
ENV PATH="/cargo/bin:$PATH"

# 替换 apt 源为阿里云镜像（ubuntu-ports for arm64）
RUN sed -i 's|http://ports.ubuntu.com/ubuntu-ports|http://mirrors.aliyun.com/ubuntu-ports|g' /etc/apt/sources.list.d/ubuntu.sources 2>/dev/null || \
    sed -i 's|http://ports.ubuntu.com/ubuntu-ports|http://mirrors.aliyun.com/ubuntu-ports|g' /etc/apt/sources.list 2>/dev/null || true

# 基础工具
RUN apt-get update && apt-get install -y \
    curl wget git build-essential pkg-config libssl-dev \
    python3 jq ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Node.js 22 + pnpm（使用国内镜像）
RUN curl -fsSL https://deb.nodesource.com/setup_22.x | bash - \
    && apt-get install -y nodejs \
    && npm config set registry https://registry.npmmirror.com \
    && npm install -g pnpm@10.30.3 \
    && rm -rf /var/lib/apt/lists/*

# Rust stable（使用 rsproxy.cn 镜像）
ENV RUSTUP_DIST_SERVER="https://rsproxy.cn"
ENV RUSTUP_UPDATE_ROOT="https://rsproxy.cn/rustup"
RUN curl --proto '=https' --tlsv1.2 -sSf https://rsproxy.cn/rustup-init.sh | sh -s -- -y --default-toolchain stable

# 配置 cargo 使用国内镜像源
RUN mkdir -p /cargo && printf '[source.crates-io]\nreplace-with = "rsproxy-sparse"\n\n[source.rsproxy-sparse]\nregistry = "sparse+https://rsproxy.cn/index/"\n\n[registries.rsproxy]\nindex = "https://rsproxy.cn/crates.io-index"\n\n[net]\ngit-fetch-with-cli = true\n' > /cargo/config.toml

WORKDIR /workspace
DOCKERFILE

echo "==> Docker 镜像构建完成"
echo "==> 启动容器运行测试..."

# 使用 volume 缓存 cargo registry 和 target
docker run --rm \
    -v "$ROOT_DIR":/workspace:cached \
    -v bifrost-cargo-registry:/cargo/registry \
    -v bifrost-cargo-git:/cargo/git \
    -v bifrost-target-linux:/workspace/target \
    -e BIFROST_E2E_REPORT_DIR=/tmp/e2e-reports \
    -e BIFROST_DATA_DIR=/tmp/bifrost-e2e-ci \
    -e TIMEOUT=120 \
    -e PYTHONIOENCODING=utf-8 \
    "$DOCKER_IMAGE" \
    bash -c "
set -euo pipefail

echo '==> [容器内] 编译 bifrost release 二进制...'
cargo build --release --bin bifrost 2>&1

echo '==> [容器内] 安装 sync-server 依赖并构建...'
cd /workspace/packages/bifrost-sync-server
pnpm install --frozen-lockfile 2>&1
pnpm build 2>&1

echo '==> [容器内] 运行 E2E 测试: $TEST_SCRIPT'
cd /workspace
export BIFROST_BIN=/workspace/target/release/bifrost
export SKIP_BUILD=true
mkdir -p /tmp/e2e-reports /tmp/bifrost-e2e-ci

bash e2e-tests/tests/$TEST_SCRIPT
"
