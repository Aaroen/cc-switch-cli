#!/bin/bash
#
# CC Switch 快速部署脚本
# 从已编译的二进制文件快速部署 CC Switch 服务
#
# 使用方法：
#   ./deploy.sh                                    # 默认配置部署
#   ./deploy.sh --binary /path/to/cc-switch-cli    # 指定二进制文件
#   ./deploy.sh --port 15722 --host 0.0.0.0        # 自定义端口和监听地址
#   ./deploy.sh --headless                         # 强制无头模式
#   ./deploy.sh --gui                              # 强制GUI模式
#
# 环境变量：
#   CC_SWITCH_PORT=端口号         # 代理服务端口（默认: 15721）
#   CC_SWITCH_HOST=监听地址       # 代理监听地址（默认: 0.0.0.0）
#   CC_SWITCH_WEB_PORT=端口号     # Web控制台端口（默认: 8888）
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
DEFAULT_WEB_PORT=8888
DEFAULT_PROXY_HOST="0.0.0.0"
DEFAULT_WEB_BIND="0.0.0.0"
MAX_PORT_RETRY=10

# 全局变量
BINARY_PATH=""
PROXY_PORT=""
PROXY_HOST=""
WEB_PORT=""
WEB_BIND=""
FORCE_CLI=false
FORCE_GUI=false
CLI_MODE=false
CONFIG_ONLY=false
REMOTE_PROXY=""

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
  --binary <path>        指定二进制文件路径
  --port <port>          代理服务端口（默认: 15721）
  --host <addr>          代理监听地址（默认: 0.0.0.0）
  --web-port <port>      Web控制台端口（默认: 8888）
  --web-bind <addr>      Web控制台监听地址（默认: 0.0.0.0）
  --remote-proxy <url>   远程代理地址（如: 100.106.96.110:15722）
  --config-only          仅修改配置文件，不安装服务
  --headless             强制无头模式（当前为默认模式）
  --gui                  强制GUI模式（尚未完全实现）
  -h, --help             显示此帮助信息

模式说明:
  默认模式: 下载/安装二进制 + 启动本地服务 + 配置CLI指向本地
  --config-only: 仅修改Claude/Codex配置文件
  --remote-proxy: 配置CLI指向远程代理服务器

示例:
  ./deploy.sh                                         # 完整部署本地服务
  ./deploy.sh --port 15722                            # 使用不同端口
  ./deploy.sh --config-only --remote-proxy 10.0.0.1:15722   # 仅配置指向远程
  ./deploy.sh --remote-proxy 10.0.0.1:15722           # 本地服务 + 指向远程

环境变量:
  CC_SWITCH_PORT         代理服务端口
  CC_SWITCH_HOST         代理监听地址
  CC_SWITCH_WEB_PORT     Web控制台端口
  CC_SWITCH_WEB_BIND     Web控制台监听地址
  CC_SWITCH_REMOTE_PROXY 远程代理地址
  CC_SWITCH_CONFIG_ONLY  设为true启用仅配置模式

一行命令示例:
  # 仅配置指向远程代理
  curl -fsSL https://raw.githubusercontent.com/Aaroen/cc-switch-cli/main/deploy.sh | \\
    env CC_SWITCH_CONFIG_ONLY=true CC_SWITCH_REMOTE_PROXY=10.0.0.1:15722 bash

  # 完整部署 + 配置指向远程
  curl -fsSL https://raw.githubusercontent.com/Aaroen/cc-switch-cli/main/deploy.sh | \\
    env CC_SWITCH_REMOTE_PROXY=10.0.0.1:15722 bash
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
            --remote-proxy)
                REMOTE_PROXY="$2"
                shift 2
                ;;
            --config-only)
                CONFIG_ONLY=true
                shift
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

    PROXY_PORT="${PROXY_PORT:-${CC_SWITCH_PORT:-$DEFAULT_PROXY_PORT}}"
    PROXY_HOST="${PROXY_HOST:-${CC_SWITCH_HOST:-$DEFAULT_PROXY_HOST}}"
    WEB_PORT="${WEB_PORT:-${CC_SWITCH_WEB_PORT:-$DEFAULT_WEB_PORT}}"
    WEB_BIND="${WEB_BIND:-${CC_SWITCH_WEB_BIND:-$DEFAULT_WEB_BIND}}"
    REMOTE_PROXY="${REMOTE_PROXY:-${CC_SWITCH_REMOTE_PROXY:-}}"

    # 环境变量支持
    if [ "${CC_SWITCH_CONFIG_ONLY:-false}" = "true" ]; then
        CONFIG_ONLY=true
    fi

    # 检查冲突的参数
    if [ "$FORCE_CLI" = "true" ] && [ "$FORCE_GUI" = "true" ]; then
        error "不能同时指定 --headless 和 --gui 参数"
    fi

    if [ "$CONFIG_ONLY" = "true" ] && [ -z "$REMOTE_PROXY" ]; then
        error "--config-only 必须配合 --remote-proxy 使用"
    fi
}

# 确定部署模式
determine_mode() {
    # 当前仅支持 CLI 无头模式
    # GUI 模式的支持正在开发中
    if [ "$FORCE_GUI" = "true" ]; then
        echo -e "${YELLOW}⚠ 注意: GUI 模式尚未完全实现，将使用 CLI 模式${NC}"
    fi

    CLI_MODE=true
}

# 检测Python
check_python() {
    if ! command -v python3 &>/dev/null; then
        error "未找到 python3，无法修改配置文件\n请安装: sudo apt install python3"
    fi
}

# 下载最新的二进制文件
download_binary() {

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


    # 清理旧文件
    rm -rf cc-switch-cli-linux-x86_64 cc-switch-cli 2>/dev/null || true

    # 下载并解压
    if ! curl -fsSL "https://github.com/Aaroen/cc-switch-cli/releases/latest/download/cc-switch-cli-linux-x86_64.tar.gz" | tar -xz; then
        error "下载失败，请检查网络连接或手动下载：
  https://github.com/Aaroen/cc-switch-cli/releases/latest"
    fi

    # 查找解压后的二进制文件 - 优先查找 cc-switch-cli
    local binary=""
    if [ -f "cc-switch-cli-linux-x86_64/cc-switch-cli" ]; then
        binary="$download_dir/cc-switch-cli-linux-x86_64/cc-switch-cli"
    elif [ -f "cc-switch-cli" ]; then
        binary="$download_dir/cc-switch-cli"
    elif [ -f "cc-switch-cli-linux-x86_64/cc-switch" ]; then
        binary="$download_dir/cc-switch-cli-linux-x86_64/cc-switch"
    elif [ -f "cc-switch" ]; then
        binary="$download_dir/cc-switch"
    else
        error "下载的文件中未找到 cc-switch-cli 或 cc-switch 二进制"
    fi

    chmod +x "$binary"

    # 同时复制 deploy.sh 到下载目录（如果存在）
    if [ -f "cc-switch-cli-linux-x86_64/deploy.sh" ]; then
        cp "cc-switch-cli-linux-x86_64/deploy.sh" "$download_dir/deploy.sh" 2>/dev/null || true
    fi


    # 只输出二进制路径到 stdout（供 find_binary 捕获）
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

    # 优先级2：编译输出目录 - 查找 cc-switch-cli
    if [ -z "$binary" ] && [ -f "src-tauri/target/release/cc-switch-cli" ]; then
        binary="src-tauri/target/release/cc-switch-cli"
    fi

    # 优先级3：当前目录
    if [ -z "$binary" ] && [ -f "./cc-switch-cli" ]; then
        binary="./cc-switch-cli"
    fi

    # 优先级4：脚本同级目录
    if [ -z "$binary" ]; then
        local script_dir="$(cd "$(dirname "$0")" && pwd)"
        if [ -f "$script_dir/cc-switch-cli" ]; then
            binary="$script_dir/cc-switch-cli"
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
}

# 检测端口是否被占用
# 返回值：0=端口可用，1=端口被占用
check_port_available() {
    local port="$1"
    if command -v ss &>/dev/null; then
        # 使用ss检测，更准确
        # -tuln: tcp, udp, listening, numeric
        ss -tuln 2>/dev/null | awk '{print $5}' | grep -q ":$port$" && return 1 || return 0
    elif command -v netstat &>/dev/null; then
        # 使用netstat作为fallback
        netstat -tuln 2>/dev/null | awk '{print $4}' | grep -q ":$port$" && return 1 || return 0
    else
        # 无netstat/ss，尝试lsof
        if command -v lsof &>/dev/null; then
            lsof -i ":$port" -sTCP:LISTEN >/dev/null 2>&1 && return 1 || return 0
        fi
        # 无法检测，假设可用
        return 0
    fi
}

# 自动解决端口冲突
resolve_port_conflict() {
    local port_var_name="$1"
    local port_value="${!port_var_name}"
    local original_port="$port_value"
    local attempt=0

    # check_port_available: 可用返回0，占用返回1
    while ! check_port_available "$port_value"; do
        attempt=$((attempt + 1))
        if [ $attempt -gt $MAX_PORT_RETRY ]; then
            error "端口 $original_port 及后续 $MAX_PORT_RETRY 个端口均被占用"
        fi
        port_value=$((port_value + 1))
    done

    if [ "$port_value" != "$original_port" ]; then
        echo "端口 $original_port 被占用，使用 $port_value"
        eval "$port_var_name=$port_value"
    fi
}

# 检测所有端口
check_ports() {
    resolve_port_conflict "PROXY_PORT"
    if [ "$CLI_MODE" = "true" ]; then
        resolve_port_conflict "WEB_PORT"
    fi
}

# 停止旧服务
stop_old_service() {
    # 更精确的进程匹配：使用完整路径或特定命令模式
    if ! pgrep -f "cc-switch-cli.*server|/ccs.*server" >/dev/null 2>&1; then
        return 0
    fi


    # 优先使用 ccs 命令
    if command -v ccs &>/dev/null; then
        ccs server stop 2>/dev/null || true
    fi

    # 等待进程退出（最多10秒）
    local count=0
    while pgrep -f "cc-switch-cli.*server|/ccs.*server" >/dev/null 2>&1; do
        sleep 1
        count=$((count + 1))
        if [ $count -gt 10 ]; then
            # 忽略权限错误，有些进程可能属于其他用户
            pkill -9 -f "cc-switch-cli.*server|/ccs.*server" 2>/dev/null || true
            sleep 1
            break
        fi
    done

    # 最终检查
    if pgrep -f "cc-switch-cli.*server|/ccs.*server" >/dev/null 2>&1; then
        echo "注意: 部分进程属于其他用户，无法停止"
    fi
}

# 安装二进制文件
# 安装二进制文件
install_binary() {
    local src="$1"
    local dest_dir="$HOME/.local/bin"
    local dest="$dest_dir/cc-switch-cli"

    mkdir -p "$dest_dir"

    # 如果源和目标是同一文件，跳过
    if [ "$(realpath "$src" 2>/dev/null)" = "$(realpath "$dest" 2>/dev/null)" ]; then
        echo "二进制文件已在目标位置"
        # 仍然需要创建软链接和验证
        ln -sf "$dest" "$dest_dir/ccs"
        if ! "$dest" --version &>/dev/null; then
            error "二进制文件无法运行"
        fi
        return 0
    fi

    # 备份旧版本
    if [ -f "$dest" ]; then
        local backup="$dest.bak-$(date +%Y%m%d-%H%M%S)"
        cp "$dest" "$backup" 2>/dev/null || true
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

}

# 配置PATH
configure_path() {
    local bin_dir="$HOME/.local/bin"

    if [[ ":$PATH:" == *":$bin_dir:"* ]]; then
        return 0
    fi


    for rc in ~/.bashrc ~/.zshrc ~/.profile; do
        if [ -f "$rc" ]; then
            if ! grep -q "export PATH=\"$bin_dir:\$PATH\"" "$rc" 2>/dev/null; then
                echo "export PATH=\"$bin_dir:\$PATH\"" >> "$rc"
            fi
        fi
    done

    # 导出到当前会话
    export PATH="$bin_dir:$PATH"
}

# 配置 Claude Code CLI
configure_claude() {
    # 使用远程代理或本地代理
    local base_url
    if [ -n "$REMOTE_PROXY" ]; then
        base_url="http://${REMOTE_PROXY}"
    else
        base_url="http://127.0.0.1:${PROXY_PORT}"
    fi

    local settings_file="$HOME/.claude/settings.json"

    mkdir -p "$HOME/.claude"

    # 备份配置文件
    if [ -f "$settings_file" ]; then
        local backup="$settings_file.bak-$(date +%Y%m%d-%H%M%S)"
        cp "$settings_file" "$backup"
    fi

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
except Exception as e:
    print(f"错误：无法读取配置文件: {e}", file=sys.stderr)
    sys.exit(1)

if 'env' not in data:
    data['env'] = {}

data['env']['ANTHROPIC_BASE_URL'] = base_url
data['env']['ANTHROPIC_API_KEY'] = 'sk-ant-cc-switch-placeholder'

try:
    with open(settings_file, 'w') as f:
        json.dump(data, f, indent=2)
except Exception as e:
    print(f"错误：无法写入配置文件: {e}", file=sys.stderr)
    sys.exit(1)
PYEOF
        if [ $? -ne 0 ]; then
            error "配置文件更新失败"
        fi
    fi
}

# 配置 Codex CLI - config.toml
configure_codex_config() {
    # 使用远程代理或本地代理
    local base_url
    if [ -n "$REMOTE_PROXY" ]; then
        base_url="http://${REMOTE_PROXY}/v1"
    else
        base_url="http://127.0.0.1:${PROXY_PORT}/v1"
    fi

    local config_file="$HOME/.codex/config.toml"

    if [ ! -f "$config_file" ]; then
        return 0
    fi


    # 备份配置文件
    local backup="$config_file.bak-$(date +%Y%m%d-%H%M%S)"
    cp "$config_file" "$backup"

    python3 - "$config_file" "$base_url" <<'PYEOF'
import re
import sys

config_file = sys.argv[1]
base_url = sys.argv[2]

try:
    with open(config_file, 'r') as f:
        content = f.read()
except Exception as e:
    print(f"错误：无法读取配置文件: {e}", file=sys.stderr)
    sys.exit(1)

# 只替换 base_url 行
updated = re.sub(
    r'(base_url\s*=\s*")[^"]*(")',
    f'\\1{base_url}\\2',
    content
)

try:
    with open(config_file, 'w') as f:
        f.write(updated)
except Exception as e:
    print(f"错误：无法写入配置文件: {e}", file=sys.stderr)
    sys.exit(1)
PYEOF
    if [ $? -ne 0 ]; then
        error "配置文件更新失败"
    fi
}

# 配置 Codex CLI - auth.json
# 配置 Codex CLI auth.json
configure_codex_auth() {
    local auth_file="$HOME/.codex/auth.json"

    if [ ! -f "$auth_file" ]; then
        return 0
    fi

    # 备份配置文件
    local backup="$auth_file.bak-$(date +%Y%m%d-%H%M%S)"
    cp "$auth_file" "$backup"

    python3 - "$auth_file" <<'PYEOF'
import json
import sys

auth_file = sys.argv[1]

try:
    with open(auth_file, 'r') as f:
        data = json.load(f)
except Exception as e:
    print(f"错误：无法读取配置文件: {e}", file=sys.stderr)
    sys.exit(1)

# Codex使用OPENAI_API_KEY格式
if 'OPENAI_API_KEY' in data:
    data['OPENAI_API_KEY'] = 'sk-ant-cc-switch-placeholder'

try:
    with open(auth_file, 'w') as f:
        json.dump(data, f, indent=2)
except Exception as e:
    print(f"错误：无法写入配置文件: {e}", file=sys.stderr)
    sys.exit(1)
PYEOF
    if [ $? -ne 0 ]; then
        error "配置文件更新失败"
    fi
}

# 启动服务
# 启动服务
start_service() {

    # 使用绝对路径确保能找到命令
    local ccs_bin="$HOME/.local/bin/ccs"
    if [ ! -x "$ccs_bin" ]; then
        ccs_bin="ccs"  # fallback to PATH
    fi

    if [ "$CLI_MODE" = "true" ]; then
        # CLI 模式（无头），启用 Web 控制台
        local cmd="$ccs_bin server start"
        cmd="$cmd --port $PROXY_PORT"
        cmd="$cmd --host $PROXY_HOST"
        cmd="$cmd --web-port $WEB_PORT"
        cmd="$cmd --web-bind $WEB_BIND"

        # 临时禁用 errexit 以处理启动失败
        set +e
        $cmd
        local start_exit_code=$?
        set -e

        if [ $start_exit_code -ne 0 ]; then
            error "服务启动失败（退出码: $start_exit_code）"
        fi

        # 等待服务启动，最多等待15秒
        local wait_count=0
        local max_wait=15
        sleep 3

        while [ $wait_count -lt $max_wait ]; do
            if "$ccs_bin" server status >/dev/null 2>&1; then
                return 0
            fi
            sleep 1
            wait_count=$((wait_count + 1))
        done

        # 超时仍未启动，报错
        echo "错误: 服务启动超时"
        echo "可能原因:"
        echo "  1. 端口 $PROXY_PORT 或 $WEB_PORT 被其他进程占用"
        echo "  2. 配置文件有误"
        echo ""
        echo "检查端口占用情况:"
        if command -v ss &>/dev/null; then
            ss -tuln | grep -E ":$PROXY_PORT |:$WEB_PORT " || echo "  未发现端口监听"
        fi
        error "服务启动失败"
    else
        # GUI 模式
        nohup cc-switch-gui >/dev/null 2>&1 &
        sleep 1
    fi
}

# 配置权重轮询
configure_weight_round_robin() {
    sleep 2  # 确保服务完全启动

    for app in claude codex gemini; do
        ccs config loadbalance --app "$app" --enabled true --strategy frequency >/dev/null 2>&1
    done
}

# 验证部署
verify_deployment() {
    # 检查CLI服务进程
    if ! pgrep -f "cc-switch-cli.*server|/ccs.*server" >/dev/null 2>&1; then
        error "服务未运行"
    fi
}

# 显示部署摘要
show_summary() {
    echo ""
    echo "部署完成"
    echo ""
    echo "服务信息:"
    echo "  代理端口: $PROXY_PORT"
    if [ "$CLI_MODE" = "true" ]; then
        echo "  Web端口: $WEB_PORT"
    fi
    echo ""
    echo "常用命令:"
    echo "  ccs status    - 查看状态"
    echo "  ccs stop      - 停止服务"
    echo "  ccs restart   - 重启服务"
    echo ""
}

# 主流程
main() {
    echo "CC Switch 部署"
    echo ""

    parse_arguments "$@"
    determine_mode

    # config-only模式：仅修改配置文件
    if [ "$CONFIG_ONLY" = "true" ]; then
        echo "[1/2] 配置CLI工具..."
        check_python
        configure_claude
        configure_codex_config
        configure_codex_auth

        echo "[2/2] 完成"
        echo ""
        echo "配置完成"
        echo ""
        echo "已配置Claude Code和Codex CLI指向: $REMOTE_PROXY"
        return 0
    fi

    # 完整部署模式
    echo "[1/8] 查找二进制文件..."
    BINARY=$(find_binary)
    verify_binary "$BINARY"

    echo "[2/8] 停止旧服务..."
    stop_old_service

    echo "[3/8] 检测端口..."
    check_ports

    echo "[4/8] 安装二进制..."
    install_binary "$BINARY"
    configure_path

    echo "[5/8] 配置CLI工具..."
    check_python
    configure_claude
    configure_codex_config
    configure_codex_auth

    echo "[6/8] 启动服务..."
    start_service

    echo "[7/8] 配置权重轮询..."
    configure_weight_round_robin

    echo "[8/8] 验证部署..."
    verify_deployment

    show_summary
}

main "$@"
