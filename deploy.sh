#!/bin/bash
#
# CC Switch 快速部署脚本
# 从已编译的二进制文件快速部署 CC Switch 服务
#
# 使用方法：
#   ./deploy.sh                                    # 默认配置部署
#   ./deploy.sh --binary /path/to/cc-switch        # 指定二进制文件
#   ./deploy.sh --port 15722 --host 0.0.0.0        # 自定义端口和监听地址
#   ./deploy.sh --headless                         # 强制无头模式
#   ./deploy.sh --gui                              # 强制GUI模式
#
# 环境变量：
#   CC_SWITCH_PORT=端口号         # 代理服务端口（默认: 15721）
#   CC_SWITCH_HOST=监听地址       # 代理监听地址（默认: 0.0.0.0）
#   CC_SWITCH_WEB_PORT=端口号     # Web控制台端口（默认: 15720）
#   CC_SWITCH_WEB_BIND=监听地址   # Web控制台监听地址（默认: 0.0.0.0）

set -Eeuo pipefail

# 颜色定义
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

# 默认配置
DEFAULT_PROXY_PORT=15721
DEFAULT_WEB_PORT=15720
DEFAULT_PROXY_HOST="0.0.0.0"
DEFAULT_WEB_BIND="0.0.0.0"
MAX_PORT_RETRY=3

# 全局变量
BINARY_PATH=""
PROXY_PORT=""
PROXY_HOST=""
WEB_PORT=""
WEB_BIND=""
FORCE_CLI=false
FORCE_GUI=false
HAS_GUI=false
CLI_MODE=false

# 错误处理
error() {
    echo -e "${RED}错误: $1${NC}" >&2
    exit 1
}

# 显示帮助
show_help() {
    cat <<EOF
CC Switch 快速部署脚本

使用方法:
  ./deploy.sh [选项]

选项:
  --binary <path>      指定二进制文件路径
  --port <port>        代理服务端口（默认: 15721）
  --host <addr>        代理监听地址（默认: 0.0.0.0）
  --web-port <port>    Web控制台端口（默认: 15720）
  --web-bind <addr>    Web控制台监听地址（默认: 0.0.0.0）
  --headless           强制无头模式
  --gui                强制GUI模式
  -h, --help           显示此帮助信息

示例:
  ./deploy.sh
  ./deploy.sh --port 15722
  ./deploy.sh --binary ./cc-switch --headless

环境变量:
  CC_SWITCH_PORT       代理服务端口
  CC_SWITCH_HOST       代理监听地址
  CC_SWITCH_WEB_PORT   Web控制台端口
  CC_SWITCH_WEB_BIND   Web控制台监听地址
EOF
}

# 解析命令行参数
parse_arguments() {
    while [[ $# -gt 0 ]]; do
        case $1 in
            --binary)
                BINARY_PATH="$2"
                shift 2
                ;;
            --port)
                PROXY_PORT="$2"
                shift 2
                ;;
            --host)
                PROXY_HOST="$2"
                shift 2
                ;;
            --web-port)
                WEB_PORT="$2"
                shift 2
                ;;
            --web-bind)
                WEB_BIND="$2"
                shift 2
                ;;
            --headless)
                FORCE_CLI=true
                shift
                ;;
            --gui)
                FORCE_GUI=true
                shift
                ;;
            --help|-h)
                show_help
                exit 0
                ;;
            *)
                error "未知参数: $1\n使用 --help 查看帮助"
                ;;
        esac
    done

    # 设置默认值（优先级：命令行参数 > 环境变量 > 默认值）
    PROXY_PORT="${PROXY_PORT:-${CC_SWITCH_PORT:-$DEFAULT_PROXY_PORT}}"
    PROXY_HOST="${PROXY_HOST:-${CC_SWITCH_HOST:-$DEFAULT_PROXY_HOST}}"
    WEB_PORT="${WEB_PORT:-${CC_SWITCH_WEB_PORT:-$DEFAULT_WEB_PORT}}"
    WEB_BIND="${WEB_BIND:-${CC_SWITCH_WEB_BIND:-$DEFAULT_WEB_BIND}}"
}

# 检测GUI环境
detect_gui() {
    if [ -n "${DISPLAY:-}" ] || [ -n "${WAYLAND_DISPLAY:-}" ]; then
        HAS_GUI=true
        echo -e "${BLUE}✓ 检测到 GUI 环境${NC}"
    else
        HAS_GUI=false
        echo -e "${BLUE}✓ 检测到无头环境${NC}"
    fi
}

# 确定部署模式
determine_mode() {
    if [ "$FORCE_CLI" = "true" ]; then
        CLI_MODE=true
        echo -e "${CYAN}▸ 模式: CLI（强制）${NC}"
    elif [ "$FORCE_GUI" = "true" ]; then
        CLI_MODE=false
        echo -e "${CYAN}▸ 模式: GUI（强制）${NC}"
    elif [ "$HAS_GUI" = "false" ]; then
        CLI_MODE=true
        echo -e "${CYAN}▸ 模式: CLI（自动检测）${NC}"
    else
        CLI_MODE=false
        echo -e "${CYAN}▸ 模式: GUI（自动检测）${NC}"
    fi
}

# 检测Python
check_python() {
    if ! command -v python3 &>/dev/null; then
        error "未找到 python3，无法修改配置文件\n请安装: sudo apt install python3"
    fi
}

# 下载最新的二进制文件
download_binary() {
    echo -e "${BLUE}未找到本地二进制文件，正在从 GitHub Release 下载...${NC}"

    # 检测架构
    local arch="$(uname -m)"
    local os="$(uname -s | tr '[:upper:]' '[:lower:]')"

    if [ "$arch" != "x86_64" ] || [ "$os" != "linux" ]; then
        error "当前仅支持 Linux x86_64 架构，检测到: $os $arch"
    fi

    # 使用 CC Switch 数据目录
    local download_dir="$HOME/.cc-switch/downloads"
    mkdir -p "$download_dir"
    cd "$download_dir" || error "无法创建下载目录"

    echo -e "${CYAN}下载地址: https://github.com/Aaroen/cc-switch-cli/releases/latest/download/cc-switch-cli-linux-x86_64.tar.gz${NC}"

    # 清理旧文件
    rm -rf cc-switch-cli-linux-x86_64 cc-switch 2>/dev/null || true

    # 下载并解压
    if ! curl -fsSL "https://github.com/Aaroen/cc-switch-cli/releases/latest/download/cc-switch-cli-linux-x86_64.tar.gz" | tar -xz; then
        error "下载失败，请检查网络连接或手动下载：
  https://github.com/Aaroen/cc-switch-cli/releases/latest"
    fi

    # 查找解压后的二进制文件
    local binary=""
    if [ -f "cc-switch-cli-linux-x86_64/cc-switch" ]; then
        binary="$download_dir/cc-switch-cli-linux-x86_64/cc-switch"
    elif [ -f "cc-switch" ]; then
        binary="$download_dir/cc-switch"
    else
        error "下载的文件中未找到 cc-switch 二进制"
    fi

    chmod +x "$binary"

    # 同时复制 deploy.sh 到下载目录（如果存在）
    if [ -f "cc-switch-cli-linux-x86_64/deploy.sh" ]; then
        cp "cc-switch-cli-linux-x86_64/deploy.sh" "$download_dir/deploy.sh" 2>/dev/null || true
    fi

    echo -e "${GREEN}✓ 下载完成: $binary${NC}"
    echo -e "${GREEN}✓ 文件保存在: $download_dir${NC}"
    echo "$binary"
}

# 查找二进制文件
find_binary() {
    local binary=""

    # 优先级1：命令行参数指定
    if [ -n "$BINARY_PATH" ]; then
        if [ -f "$BINARY_PATH" ]; then
            binary="$BINARY_PATH"
        else
            error "指定的二进制文件不存在: $BINARY_PATH"
        fi
    fi

    # 优先级2：编译输出目录
    if [ -z "$binary" ] && [ -f "src-tauri/target/release/cc-switch" ]; then
        binary="src-tauri/target/release/cc-switch"
    fi

    # 优先级3：当前目录
    if [ -z "$binary" ] && [ -f "./cc-switch" ]; then
        binary="./cc-switch"
    fi

    # 优先级4：脚本同级目录
    if [ -z "$binary" ]; then
        local script_dir="$(cd "$(dirname "$0")" && pwd)"
        if [ -f "$script_dir/cc-switch" ]; then
            binary="$script_dir/cc-switch"
        fi
    fi

    # 优先级5：自动从 GitHub Release 下载
    if [ -z "$binary" ]; then
        binary="$(download_binary)"
    fi

    echo "$binary"
}

# 验证二进制文件
verify_binary() {
    local binary="$1"

    chmod +x "$binary" 2>/dev/null || true

    # 尝试运行 --version，如果失败给出详细信息
    if ! "$binary" --version &>/dev/null; then
        echo -e "${YELLOW}⚠ 警告: 无法验证二进制文件版本${NC}"
        echo -e "${YELLOW}  文件: $binary${NC}"
        echo -e "${YELLOW}  尝试继续安装...${NC}"
        return 0
    fi

    local version="$("$binary" --version 2>&1 | head -1)"
    echo -e "${GREEN}✓ 检测到版本: ${version}${NC}"
}

# 检测端口是否被占用
check_port_available() {
    local port="$1"
    if command -v netstat &>/dev/null; then
        netstat -tuln 2>/dev/null | grep -q ":$port " && return 0 || return 1
    elif command -v ss &>/dev/null; then
        ss -tuln 2>/dev/null | grep -q ":$port " && return 0 || return 1
    else
        # 无法检测，假设可用
        return 1
    fi
}

# 自动解决端口冲突
resolve_port_conflict() {
    local port_var_name="$1"
    local port_value="${!port_var_name}"
    local original_port="$port_value"
    local attempt=0

    while check_port_available "$port_value"; do
        attempt=$((attempt + 1))
        if [ $attempt -gt $MAX_PORT_RETRY ]; then
            error "端口 $original_port 被占用，尝试了 $MAX_PORT_RETRY 次仍无可用端口"
        fi

        port_value=$((port_value + 1))
        echo -e "${YELLOW}⚠ 端口 $(($port_value - 1)) 被占用，尝试 $port_value ...${NC}"
    done

    if [ "$port_value" != "$original_port" ]; then
        echo -e "${GREEN}✓ 使用端口: $port_value ${YELLOW}(原端口 $original_port 被占用)${NC}"
        eval "$port_var_name=$port_value"
    fi
}

# 检测所有端口
check_ports() {
    echo -e "${BLUE}检查端口可用性...${NC}"
    resolve_port_conflict "PROXY_PORT"
    if [ "$CLI_MODE" = "true" ]; then
        resolve_port_conflict "WEB_PORT"
    fi
}

# 停止旧服务
stop_old_service() {
    echo -e "${BLUE}检查运行中的服务...${NC}"

    if ! pgrep -f "cc-switch|ccs server" >/dev/null 2>&1; then
        echo -e "${GREEN}✓ 无运行中的服务${NC}"
        return 0
    fi

    echo -e "${YELLOW}发现运行中的服务，正在停止...${NC}"

    # 优先使用 ccs 命令
    if command -v ccs &>/dev/null; then
        ccs server stop 2>/dev/null || pkill -f "cc-switch|ccs server"
    else
        pkill -f "cc-switch|ccs server"
    fi

    # 等待进程退出（最多10秒）
    local count=0
    while pgrep -f "cc-switch|ccs server" >/dev/null 2>&1; do
        sleep 1
        count=$((count + 1))
        if [ $count -gt 10 ]; then
            echo -e "${YELLOW}⚠ 进程未响应，强制结束...${NC}"
            pkill -9 -f "cc-switch|ccs server"
            sleep 1
            break
        fi
    done

    echo -e "${GREEN}✓ 旧服务已停止${NC}"
}

# 安装二进制文件
install_binary() {
    local src="$1"
    local dest_dir="$HOME/.local/bin"
    local dest="$dest_dir/cc-switch-cli"

    echo -e "${BLUE}安装二进制文件...${NC}"
    mkdir -p "$dest_dir"

    # 备份旧版本
    if [ -f "$dest" ]; then
        local backup="$dest.bak-$(date +%Y%m%d-%H%M%S)"
        cp "$dest" "$backup"
        echo -e "${GREEN}✓ 已备份旧版本: $(basename "$backup")${NC}"
    fi

    # 复制新二进制
    cp "$src" "$dest"
    chmod +x "$dest"

    # 创建软链接
    ln -sf "$dest" "$dest_dir/ccs"

    # 验证
    if ! "$dest" --version &>/dev/null; then
        error "安装失败：二进制文件无法运行"
    fi

    echo -e "${GREEN}✓ 安装成功: $dest${NC}"
}

# 配置PATH
configure_path() {
    local bin_dir="$HOME/.local/bin"

    if [[ ":$PATH:" == *":$bin_dir:"* ]]; then
        return 0
    fi

    echo -e "${BLUE}配置 PATH...${NC}"

    for rc in ~/.bashrc ~/.zshrc ~/.profile; do
        if [ -f "$rc" ]; then
            if ! grep -q "export PATH=\"$bin_dir:\$PATH\"" "$rc" 2>/dev/null; then
                echo "export PATH=\"$bin_dir:\$PATH\"" >> "$rc"
                echo -e "${GREEN}✓ 已添加到 $(basename "$rc")${NC}"
            fi
        fi
    done

    # 导出到当前会话
    export PATH="$bin_dir:$PATH"
}

# 配置 Claude Code CLI
configure_claude() {
    local base_url="http://127.0.0.1:${PROXY_PORT}"
    local settings_file="$HOME/.claude/settings.json"

    echo -e "${BLUE}配置 Claude Code CLI...${NC}"
    mkdir -p "$HOME/.claude"

    if [ ! -f "$settings_file" ]; then
        # 文件不存在，创建
        cat > "$settings_file" <<JSON
{
  "env": {
    "ANTHROPIC_BASE_URL": "${base_url}",
    "ANTHROPIC_API_KEY": "sk-ant-cc-switch-placeholder"
  }
}
JSON
        echo -e "${GREEN}✓ 已创建配置文件${NC}"
    else
        # 文件存在，最小侵入修改
        python3 - "$settings_file" "$base_url" <<'PYEOF'
import json
import sys

settings_file = sys.argv[1]
base_url = sys.argv[2]

try:
    with open(settings_file, 'r') as f:
        data = json.load(f)
except:
    data = {}

if 'env' not in data:
    data['env'] = {}

data['env']['ANTHROPIC_BASE_URL'] = base_url
data['env']['ANTHROPIC_API_KEY'] = 'sk-ant-cc-switch-placeholder'

with open(settings_file, 'w') as f:
    json.dump(data, f, indent=2)
PYEOF
        echo -e "${GREEN}✓ 已更新配置（仅修改 2 个字段）${NC}"
    fi
}

# 配置 Codex CLI - config.toml
configure_codex_config() {
    local base_url="http://127.0.0.1:${PROXY_PORT}/v1"
    local config_file="$HOME/.codex/config.toml"

    if [ ! -f "$config_file" ]; then
        return 0
    fi

    echo -e "${BLUE}配置 Codex CLI (config.toml)...${NC}"

    python3 - "$config_file" "$base_url" <<'PYEOF'
import re
import sys

config_file = sys.argv[1]
base_url = sys.argv[2]

with open(config_file, 'r') as f:
    content = f.read()

# 只替换 base_url 行
updated = re.sub(
    r'(base_url\s*=\s*")[^"]*(")',
    f'\\1{base_url}\\2',
    content
)

with open(config_file, 'w') as f:
    f.write(updated)
PYEOF
    echo -e "${GREEN}✓ 已更新 base_url${NC}"
}

# 配置 Codex CLI - auth.json
configure_codex_auth() {
    local auth_file="$HOME/.codex/auth.json"

    if [ ! -f "$auth_file" ]; then
        return 0
    fi

    echo -e "${BLUE}配置 Codex CLI (auth.json)...${NC}"

    python3 - "$auth_file" <<'PYEOF'
import json
import sys

auth_file = sys.argv[1]

try:
    with open(auth_file, 'r') as f:
        data = json.load(f)
except:
    data = {}

# 最小侵入修改密钥
if 'api_key' in data:
    data['api_key'] = 'sk-ant-cc-switch-placeholder'

with open(auth_file, 'w') as f:
    json.dump(data, f, indent=2)
PYEOF
    echo -e "${GREEN}✓ 已更新密钥${NC}"
}

# 启动服务
start_service() {
    echo -e "${BLUE}启动 CC Switch 服务...${NC}"

    if [ "$CLI_MODE" = "true" ]; then
        # CLI 模式（无头），启用 Web 控制台
        local cmd="ccs server start"
        cmd="$cmd --port $PROXY_PORT"
        cmd="$cmd --host $PROXY_HOST"
        cmd="$cmd --web-port $WEB_PORT"
        cmd="$cmd --web-bind $WEB_BIND"

        echo -e "${CYAN}执行: $cmd${NC}"
        $cmd

        sleep 2

        if pgrep -f "ccs server" >/dev/null 2>&1; then
            echo -e "${GREEN}✓ 服务启动成功${NC}"
        else
            error "服务启动失败"
        fi
    else
        # GUI 模式
        echo -e "${CYAN}启动 GUI 应用...${NC}"
        nohup cc-switch-gui >/dev/null 2>&1 &
        sleep 1
        echo -e "${GREEN}✓ GUI 已启动${NC}"
    fi
}

# 配置权重轮询
configure_weight_round_robin() {
    echo -e "${BLUE}配置权重轮询（默认启用）...${NC}"

    sleep 3  # 等待服务完全启动

    for app in claude codex gemini; do
        ccs config set weight_round_robin_enabled true --app $app 2>/dev/null && \
            echo -e "${GREEN}✓ 已为 $app 启用权重轮询${NC}"

        ccs config set load_balance_strategy frequency --app $app 2>/dev/null && \
            echo -e "${GREEN}✓ 已为 $app 设置负载均衡策略${NC}"
    done
}

# 验证部署
verify_deployment() {
    echo -e "${BLUE}验证部署...${NC}"

    if ! pgrep -f "ccs server|cc-switch-gui" >/dev/null 2>&1; then
        error "服务未运行"
    fi

    # 验证端口
    if command -v netstat &>/dev/null || command -v ss &>/dev/null; then
        sleep 1
        if ! check_port_available "$PROXY_PORT"; then
            echo -e "${GREEN}✓ 代理端口 $PROXY_PORT 正在监听${NC}"
        else
            echo -e "${YELLOW}⚠ 警告: 代理端口 $PROXY_PORT 未监听${NC}"
        fi
    fi

    echo -e "${GREEN}✓ 验证通过${NC}"
}

# 显示部署摘要
show_summary() {
    echo ""
    echo -e "${GREEN}========================================${NC}"
    echo -e "${GREEN}  ✓ CC Switch 部署完成${NC}"
    echo -e "${GREEN}========================================${NC}"
    echo ""
    echo -e "${CYAN}服务信息:${NC}"
    echo -e "  模式: $([ "$CLI_MODE" = "true" ] && echo "CLI（无头）" || echo "GUI")"
    echo -e "  代理服务: ${BLUE}http://$PROXY_HOST:$PROXY_PORT${NC}"

    if [ "$CLI_MODE" = "true" ]; then
        echo -e "  Web 控制台: ${BLUE}http://$WEB_BIND:$WEB_PORT${NC}"
        echo -e "    ${YELLOW}(首次访问需设置密码)${NC}"
    fi

    echo ""
    echo -e "${CYAN}常用命令:${NC}"
    echo -e "  ${GREEN}ccs server status${NC}   - 查看状态"
    echo -e "  ${GREEN}ccs server stop${NC}     - 停止服务"
    echo -e "  ${GREEN}ccs server restart${NC}  - 重启服务"
    echo ""
    echo -e "${CYAN}测试 CLI:${NC}"
    echo -e "  ${GREEN}claude \"你好\"${NC}"
    echo -e "  ${GREEN}codex \"你好\"${NC}"
    echo ""
}

# 主流程
main() {
    echo -e "${CYAN}========================================${NC}"
    echo -e "${CYAN}  CC Switch 快速部署${NC}"
    echo -e "${CYAN}========================================${NC}"
    echo ""

    parse_arguments "$@"
    detect_gui
    determine_mode
    check_python

    BINARY=$(find_binary)
    verify_binary "$BINARY"

    check_ports
    stop_old_service
    install_binary "$BINARY"
    configure_path

    configure_claude
    configure_codex_config
    configure_codex_auth

    start_service
    configure_weight_round_robin

    verify_deployment
    show_summary
}

main "$@"
