#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd -- "$SCRIPT_DIR/.." && pwd)"
BINARY="$PROJECT_DIR/target/release/speak-it"
SERVICE_FILE="$SCRIPT_DIR/speak-it.service"
SERVICE_NAME="speak-it.service"
ENV_DIR="$HOME/.config/speak-it"
ENV_FILE="$ENV_DIR/env"
BIN_DIR="$HOME/.local/bin"
BIN_TARGET="$BIN_DIR/speak-it"

echo "=========================================="
echo " speak-it systemd user service 安装"
echo "=========================================="

# 1. 检查 release 二进制是否存在，不存在则构建
if [[ ! -x "$BINARY" ]]; then
    echo "[1/4] 构建 release 二进制..."
    cd "$PROJECT_DIR"
    cargo build --release
    echo "      构建完成: $BINARY"
else
    echo "[1/4] release 二进制已存在，跳过构建"
fi

# 2. 安装二进制到 ~/.local/bin
echo "[2/4] 安装二进制到 $BIN_DIR..."
mkdir -p "$BIN_DIR"
install -m 755 "$BINARY" "$BIN_TARGET"
echo "      已安装: $BIN_TARGET"

# 3. 创建环境变量配置目录和文件
echo "[3/4] 配置环境变量..."

if [[ ! -f "$ENV_FILE" ]]; then
    mkdir -p "$ENV_DIR"

    if [[ -n "${ZHIPUAI_API_KEY:-}" ]]; then
        echo "ZHIPUAI_API_KEY=$ZHIPUAI_API_KEY" > "$ENV_FILE"
        echo "      从当前环境读取 ZHIPUAI_API_KEY"
    else
        echo "# 请在这里填入你的 BigModel API Key" > "$ENV_FILE"
        echo "# 格式: ZHIPUAI_API_KEY=your_api_key_here" >> "$ENV_FILE"
        echo "      ⚠️  未检测到 ZHIPUAI_API_KEY 环境变量"
        echo "      已创建模板文件: $ENV_FILE"
        echo "      请编辑该文件填入你的 API Key"
    fi
    chmod 600 "$ENV_FILE"
else
    echo "      配置文件已存在: $ENV_FILE"
fi

# 4. 安装 systemd user service
echo "[4/4] 安装 systemd user service..."
mkdir -p "$HOME/.config/systemd/user"
install -m 644 "$SERVICE_FILE" "$HOME/.config/systemd/user/$SERVICE_NAME"
systemctl --user daemon-reload
systemctl --user enable "$SERVICE_NAME"
echo "      user service 已启用"

echo ""
echo "=========================================="
echo " 安装完成！"
echo "=========================================="
echo ""
echo "启动服务:"
echo "  systemctl --user start speak-it"
echo ""
echo "查看状态:"
echo "  systemctl --user status speak-it"
echo ""
echo "查看日志:"
echo "  journalctl --user -u speak-it -f"
echo ""
echo "停止服务:"
echo "  systemctl --user stop speak-it"
echo ""
echo "禁用开机自启:"
echo "  systemctl --user disable speak-it"
