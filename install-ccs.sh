#!/bin/bash
#
# CC-Switch CLI v3.11.4+ 一键部署脚本（增强版）
# 支持 CLI/无头部署、可选 GUI 部署、权重轮询与当前分支更新
#
# 功能特性:
#   - 自动检测和安装系统依赖（支持多发行版）
#   - 智能端口冲突处理
#   - 网络下载重试机制
#   - 自动安装 Rust 和 Node.js
#   - 编译缓存优化
#   - 健壮的服务启动和验证
#   - 默认启用 claude/codex/gemini 的权重轮询
#
# 使用方法:
#   ./install-ccs.sh              # CLI模式部署（默认）
#   ./install-ccs.sh --gui        # GUI模式部署
#   ./install-ccs.sh --update     # 拉取当前分支最新代码后部署
#   CLI_MODE=false ./install-ccs.sh --gui  # 环境变量方式
#
# 环境变量:
#   CLI_MODE=true|false          # 设置部署模式（默认: true）
#   CC_SWITCH_PORT=端口号         # 指定代理服务端口（默认: 15721）
#   CC_SWITCH_HOST=监听地址       # 指定代理监听地址（默认: 0.0.0.0，允许局域网访问）
#   CC_SWITCH_CLIENT_HOST=访问地址 # 写入本机 CLI 配置的访问地址（默认: 127.0.0.1）
#
# 支持的Linux发行版:
#   - Ubuntu/Debian (apt)
#   - CentOS/RHEL/Fedora (dnf/yum)
#   - Arch Linux (pacman)
#   - openSUSE (zypper)
#

set -Eeuo pipefail
# 保持默认分词行为（包含空格）；脚本中有大量以空格分隔的依赖列表
IFS=$' \n\t'

# 非交互/非 TTY 环境下避免清屏、进度条刷屏等问题
IS_TTY=0
if [ -t 1 ]; then
    IS_TTY=1
fi

# sudo 兼容：root 环境下不需要 sudo；无 sudo 时给出明确错误
SUDO=""
if [ "$(id -u)" -ne 0 ]; then
    if command -v sudo >/dev/null 2>&1; then
        SUDO="sudo"
    fi
fi

# 依赖安装日志（系统依赖阶段尚未初始化 ~/.cc-switch/logs，因此先落到 /tmp）
DEPS_INSTALL_LOG="${DEPS_INSTALL_LOG:-/tmp/cc-switch-deps-install.log}"

# 错误处理陷阱
error_handler() {
    local line_no=$1
    local exit_code=$2
    echo ""
    echo -e "${RED:-}========================================${NC:-}"
    echo -e "${RED:-}  部署过程中发生错误${NC:-}"
    echo -e "${RED:-}========================================${NC:-}"
    echo -e "错误位置: 第 ${line_no} 行"
    echo -e "退出代码: ${exit_code}"
    echo ""
    echo -e "${YELLOW:-}建议:${NC:-}"
    echo "  1. 查看日志文件: ls -lh ~/.cc-switch/logs/"
    echo "  2. 检查系统依赖: apt list --installed | grep -E 'rust|node|webkit'"
    echo "  3. 手动重试或查阅文档"
    echo ""
    exit $exit_code
}

trap 'error_handler ${LINENO} $?' ERR

# 颜色定义
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color

# ============================================================================
# 系统检测函数
# ============================================================================

# 检测Linux发行版
detect_os() {
    if [ -f /etc/os-release ]; then
        . /etc/os-release
        OS=$ID
        OS_VERSION=$VERSION_ID
    elif [ -f /etc/redhat-release ]; then
        OS="rhel"
    elif [ -f /etc/debian_version ]; then
        OS="debian"
    else
        OS="unknown"
    fi
    echo "$OS"
}

# 检测包管理器
detect_package_manager() {
    if command -v apt-get &> /dev/null; then
        echo "apt"
    elif command -v dnf &> /dev/null; then
        echo "dnf"
    elif command -v yum &> /dev/null; then
        echo "yum"
    elif command -v pacman &> /dev/null; then
        echo "pacman"
    elif command -v zypper &> /dev/null; then
        echo "zypper"
    elif command -v apk &> /dev/null; then
        echo "apk"
    else
        echo "unknown"
    fi
}

# 检测图形/无头环境
#   返回 "graphical" 或 "headless"
#   依据：存在 X11 (DISPLAY) 或 Wayland (WAYLAND_DISPLAY) 显示服务即视为图形环境；
#   否则（典型如 SSH 远程、服务器、容器）视为无头环境。
#   可用环境变量 CC_SWITCH_FORCE_ENV=graphical|headless 强制覆盖检测结果。
detect_display_environment() {
    case "${CC_SWITCH_FORCE_ENV:-}" in
        graphical|headless)
            echo "$CC_SWITCH_FORCE_ENV"
            return 0
            ;;
    esac
    if [ -n "${DISPLAY:-}" ] || [ -n "${WAYLAND_DISPLAY:-}" ]; then
        echo "graphical"
    else
        echo "headless"
    fi
}

# 安装系统依赖
install_system_deps() {
    local pkg_manager=$1
    local deps=$2

    if [ -z "${deps// }" ]; then
        return 0
    fi

    if [ "$(id -u)" -ne 0 ] && [ -z "$SUDO" ]; then
        echo -e "${RED}需要 root 权限来安装系统依赖，但当前既不是 root 也未安装 sudo${NC}" >&2
        echo -e "${YELLOW}请手动安装依赖后重试：${deps}${NC}" >&2
        return 1
    fi

    case $pkg_manager in
        apt)
            DEBIAN_FRONTEND=noninteractive $SUDO apt-get update -qq >> "$DEPS_INSTALL_LOG" 2>&1 || true

            set +e
            DEBIAN_FRONTEND=noninteractive $SUDO apt-get install -y --no-install-recommends $deps >> "$DEPS_INSTALL_LOG" 2>&1
            local rc=$?
            set -e

            if [ $rc -ne 0 ]; then
                echo -e "${RED}✗ 系统依赖安装失败，原因请查看：$DEPS_INSTALL_LOG${NC}" >&2
                echo -e "${YELLOW}最近日志（末 80 行）：${NC}" >&2
                tail -n 80 "$DEPS_INSTALL_LOG" >&2 || true
                return $rc
            fi
            ;;
        dnf)
            $SUDO dnf install -y $deps > /dev/null 2>&1
            ;;
        yum)
            $SUDO yum install -y $deps > /dev/null 2>&1
            ;;
        pacman)
            $SUDO pacman -S --noconfirm $deps > /dev/null 2>&1
            ;;
        zypper)
            $SUDO zypper install -y $deps > /dev/null 2>&1
            ;;
        apk)
            $SUDO apk add --no-cache $deps > /dev/null 2>&1
            ;;
    esac
}

# 网络下载重试函数
download_with_retry() {
    local url=$1
    local output=$2
    local max_retries=${3:-3}
    local retry=0

    while [ $retry -lt $max_retries ]; do
        if command -v curl >/dev/null 2>&1; then
            if curl -fsSL --connect-timeout 10 --max-time 300 "$url" -o "$output"; then
                return 0
            fi
        elif command -v wget >/dev/null 2>&1; then
            if wget -q --timeout=10 --tries=1 -O "$output" "$url"; then
                return 0
            fi
        else
            return 1
        fi
        retry=$((retry + 1))
        if [ $retry -lt $max_retries ]; then
            echo -e "${YELLOW}下载失败，${retry}/${max_retries} 次重试中...${NC}" >&2
            sleep 2
        fi
    done
    return 1
}

# 智能查找可用端口
is_port_free() {
    local port="$1"
    local host="${2:-127.0.0.1}"

    # 最可靠：直接尝试 bind（不依赖 ss/netstat/lsof，也不依赖连接握手）
    if command -v python3 >/dev/null 2>&1; then
        python3 - "$port" "$host" <<'PY'
import errno
import socket
import sys

port = int(sys.argv[1])
host = sys.argv[2]

def can_bind_ipv4():
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    try:
        s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        s.bind((host, port))
        return True
    except OSError as e:
        if e.errno == errno.EADDRINUSE:
            return False
        # 其他错误：不确定（例如系统策略/环境异常），不要把端口误判为“被占用”
        # 交给后续启动阶段做最终判定（Address already in use 会自动换端口重试）
        return True
    finally:
        try:
            s.close()
        except Exception:
            pass

sys.exit(0 if can_bind_ipv4() else 1)
PY
        return $?
    fi

    # 次优：lsof/ss/netstat 查询
    if command -v lsof >/dev/null 2>&1; then
        ! lsof -Pi :"$port" -sTCP:LISTEN -t >/dev/null 2>&1
        return $?
    fi
    if command -v ss >/dev/null 2>&1; then
        ! ss -tln | grep -Eq ":[[:space:]]*${port}([[:space:]]|$)"
        return $?
    fi
    if command -v netstat >/dev/null 2>&1; then
        ! netstat -tln | grep -Eq ":[[:space:]]*${port}([[:space:]]|$)"
        return $?
    fi

    # 兜底：尝试连接（可能受 backlog/防火墙/拥塞影响，仅作为最后手段）
    local connect_host="$host"
    [ "$connect_host" = "0.0.0.0" ] && connect_host="127.0.0.1"
    if command -v timeout >/dev/null 2>&1; then
        ! timeout 1 bash -c "cat < /dev/null > /dev/tcp/$connect_host/$port" 2>/dev/null
        return $?
    fi
    ! bash -c "cat < /dev/null > /dev/tcp/$connect_host/$port" 2>/dev/null
    return $?
}

find_available_port() {
    local start_port=$1
    local host="${2:-127.0.0.1}"
    local max_attempts=100
    local port=$start_port

    while [ $((port - start_port)) -lt $max_attempts ]; do
        if is_port_free "$port" "$host"; then
            echo "$port"
            return 0
        fi
        port=$((port + 1))
    done

    # 未找到可用端口，返回原始端口
    echo "$start_port"
    return 0
}

# 进度条函数
show_progress() {
    local current=$1
    local total=$2
    local step_name=$3
    local status=$4  # "running" 或 "done" 或 "error"

    if [ "$IS_TTY" -ne 1 ]; then
        case "$status" in
            "running") echo "[$current/$total] ... $step_name" ;;
            "done") echo "[$current/$total] OK  $step_name" ;;
            "error") echo "[$current/$total] ERR $step_name" ;;
        esac
        return 0
    fi

    local percent=$((current * 100 / total))
    local filled=$((current * 40 / total))
    local empty=$((40 - filled))

    # 移动到行首并清除行
    printf "\r\033[K"

    # 显示进度条
    printf "["
    printf "%${filled}s" | tr ' ' '='
    printf "%${empty}s" | tr ' ' '-'
    printf "] %3d%% " "$percent"

    # 显示状态
    case "$status" in
        "running")
            printf "${CYAN}◷${NC} [%d/%d] %s" "$current" "$total" "$step_name"
            ;;
        "done")
            printf "${GREEN}✓${NC} [%d/%d] %s" "$current" "$total" "$step_name"
            ;;
        "error")
            printf "${RED}✗${NC} [%d/%d] %s" "$current" "$total" "$step_name"
            ;;
    esac
}

step_done() {
    local current=$1
    local total=$2
    local step_name=$3
    show_progress "$current" "$total" "$step_name" "done"
}

step_running() {
    local current=$1
    local total=$2
    local step_name=$3
    show_progress "$current" "$total" "$step_name" "running"
}

step_error() {
    local current=$1
    local total=$2
    local step_name=$3
    show_progress "$current" "$total" "$step_name" "error"
    echo ""
}

# pgrep 在极简系统上可能不存在，提供一个后备实现
list_pids_matching() {
    local pattern="$1"
    if command -v pgrep >/dev/null 2>&1; then
        pgrep -f "$pattern" 2>/dev/null || true
        return 0
    fi
    ps ax -o pid= -o command= 2>/dev/null | awk -v pat="$pattern" '$0 ~ pat {print $1}' || true
}

# 清屏并显示标题
if [ "$IS_TTY" -eq 1 ] && [ "${TERM:-}" != "dumb" ]; then
    clear
fi
echo -e "${CYAN}========================================${NC}"
echo -e "${CYAN}  CC-Switch 部署${NC}"
echo -e "${CYAN}========================================${NC}"
echo ""

# 参数解析和帮助信息
ARG_GUI=0
ARG_UPDATE=0
ARG_PREBUILT_BIN=""

usage() {
    echo "用法: $0 [选项]"
    echo ""
    echo "选项:"
    echo "  --gui              使用GUI模式部署"
    echo "  --update, -u       拉取官方更新后再部署"
    echo "  --prebuilt <path>  使用预构建二进制（跳过编译）"
    echo "  --help, -h         显示此帮助信息"
    echo ""
    echo "环境变量:"
    echo "  CLI_MODE=true|false    设置部署模式（默认: true）"
    echo "  CC_SWITCH_PORT=端口号   指定代理端口（默认: 15721）"
    echo "  CC_SWITCH_HOST=监听地址 指定代理监听地址（默认: 0.0.0.0，允许局域网访问）"
    echo "  CC_SWITCH_CLIENT_HOST=访问地址 写入本机 CLI 配置的访问地址（默认: 127.0.0.1）"
    echo ""
    echo "示例:"
    echo "  $0                         # CLI模式部署"
    echo "  $0 --gui                   # GUI模式部署"
    echo "  $0 --update                # 更新并部署"
    echo "  $0 --prebuilt ./cc-switch  # 使用预构建二进制部署"
    echo "  CC_SWITCH_PORT=8080 $0     # 使用8080端口"
    echo "  CC_SWITCH_HOST=127.0.0.1 $0 # 仅监听本机"
    echo ""
}

while [ $# -gt 0 ]; do
    case "${1:-}" in
        --help|-h)
            usage
            exit 0
            ;;
        --gui)
            ARG_GUI=1
            ;;
        --update|-u)
            ARG_UPDATE=1
            ;;
        --prebuilt)
            shift
            ARG_PREBUILT_BIN="${1:-}"
            if [ -z "$ARG_PREBUILT_BIN" ]; then
                echo -e "${RED}✗ --prebuilt 需要提供二进制路径${NC}"
                exit 1
            fi
            ;;
        *)
            echo -e "${YELLOW}⚠ 忽略未知参数: ${1}${NC}"
            ;;
    esac
    shift
done

# 兼容：用户可能传入相对路径
if [ -n "$ARG_PREBUILT_BIN" ] && [ -f "$ARG_PREBUILT_BIN" ]; then
    ARG_PREBUILT_BIN="$(cd "$(dirname "$ARG_PREBUILT_BIN")" && pwd)/$(basename "$ARG_PREBUILT_BIN")"
fi

if [ -n "$ARG_PREBUILT_BIN" ] && [ ! -f "$ARG_PREBUILT_BIN" ]; then
    echo -e "${RED}✗ 预构建二进制不存在: $ARG_PREBUILT_BIN${NC}"
    exit 1
fi

if [ -n "$ARG_PREBUILT_BIN" ] && [ ! -x "$ARG_PREBUILT_BIN" ]; then
    chmod +x "$ARG_PREBUILT_BIN" 2>/dev/null || true
fi

if [ -n "$ARG_PREBUILT_BIN" ] && [ ! -x "$ARG_PREBUILT_BIN" ]; then
    echo -e "${RED}✗ 预构建二进制不可执行: $ARG_PREBUILT_BIN${NC}"
    exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
IS_GIT_REPO=0
if [ -d "$SCRIPT_DIR/.git" ]; then
    IS_GIT_REPO=1
fi

# 预构建 tar.gz「解压即用」：未显式 --prebuilt、且当前目录无源码 (src-tauri)、
# 但脚本同目录存在可执行的预构建二进制时，自动按预构建二进制安装，免去手动 --prebuilt。
if [ -z "${ARG_PREBUILT_BIN:-}" ] && [ ! -d "$SCRIPT_DIR/src-tauri" ]; then
    if [ -x "$SCRIPT_DIR/cc-switch-cli" ]; then
        ARG_PREBUILT_BIN="$SCRIPT_DIR/cc-switch-cli"
        echo -e "${BLUE}检测到同目录预构建二进制，采用预构建安装: $ARG_PREBUILT_BIN${NC}"
    elif [ -x "$SCRIPT_DIR/cc-switch" ]; then
        ARG_PREBUILT_BIN="$SCRIPT_DIR/cc-switch"
        echo -e "${BLUE}检测到同目录预构建二进制，采用预构建安装: $ARG_PREBUILT_BIN${NC}"
    fi
fi

# ============================================================================
# 预检查: 系统环境和依赖
# ============================================================================
echo -e "${BLUE}检测系统环境...${NC}"

DETECTED_OS=$(detect_os)
PKG_MANAGER=$(detect_package_manager)

echo -e "  操作系统: ${GREEN}${DETECTED_OS}${NC}"
echo -e "  包管理器: ${GREEN}${PKG_MANAGER}${NC}"
echo ""

# Alpine 下构建 Tauri 依赖较复杂（musl/gtk/webkit），提前给出明确提示
if [ "$PKG_MANAGER" = "apk" ]; then
    echo -e "${RED}当前检测到 apk (Alpine). CC-Switch (Tauri) 在 Alpine 上构建/运行通常需要额外适配。${NC}"
    echo -e "${YELLOW}建议使用 Debian/Ubuntu/Fedora/CentOS/Arch/openSUSE 等发行版环境运行本脚本。${NC}"
    exit 1
fi

# 检查必要的系统依赖
REQUIRED_DEPS=""
MISSING_DEPS=""

# 检查下载工具（curl/wget 任意一个即可）
if ! command -v curl &> /dev/null && ! command -v wget &> /dev/null; then
    MISSING_DEPS="$MISSING_DEPS curl"
fi

# 检查 git（git 安装版/更新才需要）
if { [ "${ARG_UPDATE:-0}" -eq 1 ] || [ "${IS_GIT_REPO:-0}" -eq 1 ]; } && ! command -v git &> /dev/null; then
    MISSING_DEPS="$MISSING_DEPS git"
fi

# 检查sqlite3
if ! command -v sqlite3 &> /dev/null; then
    MISSING_DEPS="$MISSING_DEPS sqlite3"
fi

# 预构建二进制模式无需编译，可跳过构建依赖
if [ -z "${ARG_PREBUILT_BIN:-}" ]; then
    # 检查基础构建工具
    if ! command -v gcc &> /dev/null && ! command -v cc &> /dev/null; then
        case $PKG_MANAGER in
            apt) MISSING_DEPS="$MISSING_DEPS build-essential" ;;
            dnf|yum) MISSING_DEPS="$MISSING_DEPS gcc gcc-c++ make" ;;
            pacman) MISSING_DEPS="$MISSING_DEPS base-devel" ;;
            zypper) MISSING_DEPS="$MISSING_DEPS gcc gcc-c++ make" ;;
        esac
    fi

    # 检查pkg-config
    if ! command -v pkg-config &> /dev/null; then
        MISSING_DEPS="$MISSING_DEPS pkg-config"
    fi
fi

# pgrep/ps 等进程工具（停止旧服务用）
if ! command -v pgrep &> /dev/null; then
    case $PKG_MANAGER in
        apt) MISSING_DEPS="$MISSING_DEPS procps" ;;
        dnf|yum) MISSING_DEPS="$MISSING_DEPS procps-ng" ;;
        pacman) MISSING_DEPS="$MISSING_DEPS procps-ng" ;;
        zypper) MISSING_DEPS="$MISSING_DEPS procps" ;;
    esac
fi

# strings 用于二进制特征检测（可选）
if ! command -v strings &> /dev/null; then
    case $PKG_MANAGER in
        apt|dnf|yum|zypper) MISSING_DEPS="$MISSING_DEPS binutils" ;;
        pacman) MISSING_DEPS="$MISSING_DEPS binutils" ;;
    esac
fi

# Tauri 编译依赖（预构建二进制模式不需要）
if [ -z "${ARG_PREBUILT_BIN:-}" ]; then
    case $PKG_MANAGER in
        apt)
            # Ubuntu 22.04: libwebkit2gtk-4.0-dev; Ubuntu 24.04/Debian 12+: libwebkit2gtk-4.1-dev
            WEBKIT_PKG="libwebkit2gtk-4.0-dev"
            if apt-cache show libwebkit2gtk-4.1-dev > /dev/null 2>&1; then
                WEBKIT_PKG="libwebkit2gtk-4.1-dev"
            fi

            # 不同发行版可能使用 libayatana-appindicator3-dev 或 libappindicator3-dev
            APPIND_PKG="libayatana-appindicator3-dev"
            if ! apt-cache show "$APPIND_PKG" > /dev/null 2>&1; then
                APPIND_PKG="libappindicator3-dev"
            fi

            # 检查 webkit2gtk（按实际可用包名）
            if ! dpkg -s "$WEBKIT_PKG" > /dev/null 2>&1; then
                MISSING_DEPS="$MISSING_DEPS $WEBKIT_PKG libssl-dev libgtk-3-dev $APPIND_PKG librsvg2-dev"
            fi
            ;;
        dnf|yum)
            if ! rpm -qa | grep -q webkit2gtk3-devel; then
                MISSING_DEPS="$MISSING_DEPS webkit2gtk3-devel openssl-devel gtk3-devel libappindicator-gtk3-devel librsvg2-devel"
            fi
            ;;
        pacman)
            if ! pacman -Qi webkit2gtk &> /dev/null; then
                MISSING_DEPS="$MISSING_DEPS webkit2gtk gtk3 libappindicator-gtk3 librsvg"
            fi
            ;;
    esac
fi

# 安装缺失的依赖
if [ -n "$MISSING_DEPS" ]; then
    echo -e "${YELLOW}检测到缺失的系统依赖，正在安装...${NC}"
    echo -e "  依赖: ${MISSING_DEPS}"

    if [ "$PKG_MANAGER" = "unknown" ]; then
        echo -e "${RED}无法识别包管理器，请手动安装以下依赖: ${MISSING_DEPS}${NC}"
        exit 1
    fi

    if install_system_deps "$PKG_MANAGER" "$MISSING_DEPS"; then
        echo -e "${GREEN}✓ 系统依赖已安装${NC}"
    else
        echo -e "${RED}✗ 系统依赖安装失败，请检查权限或手动安装${NC}"
        exit 1
    fi
    echo ""
fi

# ============================================================================
# 部署模式决策：显式 --gui > 显式环境变量 CLI_MODE > 预构建(无头) > 自动环境检测
#   自动检测：图形环境默认 GUI；无头环境(SSH/服务器/容器)默认无头 CLI
# ============================================================================
DISPLAY_ENV="$(detect_display_environment)"
echo -e "  显示环境: ${GREEN}${DISPLAY_ENV}${NC}"

# 在套用默认值之前，判断用户是否显式设置了 CLI_MODE 环境变量
CLI_MODE_EXPLICIT=0
if [ -n "${CLI_MODE+x}" ]; then
    CLI_MODE_EXPLICIT=1
fi

if [ "${ARG_GUI:-0}" -eq 1 ]; then
    CLI_MODE="false"                       # 显式 --gui
elif [ "$CLI_MODE_EXPLICIT" -eq 1 ]; then
    CLI_MODE="${CLI_MODE}"                 # 显式环境变量，原样尊重
elif [ -n "${ARG_PREBUILT_BIN:-}" ]; then
    CLI_MODE="true"                        # 预构建二进制仅含无头版，强制无头
    if [ "$DISPLAY_ENV" = "graphical" ]; then
        echo -e "${YELLOW}提示: 预构建包仅含无头版本；如需图形 GUI 请使用 .deb/.AppImage 或从源码 --gui 构建${NC}"
    fi
elif [ "$DISPLAY_ENV" = "graphical" ]; then
    CLI_MODE="false"                       # 自动：图形环境 → GUI
else
    CLI_MODE="true"                        # 自动：无头环境 → CLI
fi

if [ "$CLI_MODE" = "false" ]; then
    echo -e "${YELLOW}图形 (GUI) 模式${NC}"
else
    echo -e "${GREEN}无头 (CLI) 模式${NC}"
fi
echo ""

if [ -n "${ARG_PREBUILT_BIN:-}" ] && [ "$CLI_MODE" != "true" ]; then
    echo -e "${YELLOW}⚠ 检测到 --prebuilt，已自动切换为 CLI 模式（预构建包仅支持无头部署）${NC}"
    CLI_MODE="true"
fi

# Web 控制台（仅无头/CLI 模式）：交互式询问是否启用；非交互场景可用环境变量
#   CC_SWITCH_WEB_PORT 指定端口（设置即启用），CC_SWITCH_WEB_BIND 指定监听地址（默认 0.0.0.0，允许局域网访问）。
WEB_PANEL_ENABLED=0
WEB_PORT=""
WEB_BIND="${CC_SWITCH_WEB_BIND:-0.0.0.0}"
if [ "$CLI_MODE" = "true" ]; then
    if [ -n "${CC_SWITCH_WEB_PORT:-}" ]; then
        WEB_PANEL_ENABLED=1
        WEB_PORT="$CC_SWITCH_WEB_PORT"
        echo -e "${GREEN}已按环境变量启用 Web 控制台 (端口 $WEB_PORT, 监听 $WEB_BIND)${NC}"
        echo ""
    elif [ "$IS_TTY" -eq 1 ]; then
        printf "是否启用 Web 控制台？(浏览器管理供应商/用量统计) [y/N] "
        _ans=""
        read -r _ans < /dev/tty 2>/dev/null || _ans=""
        case "$_ans" in
            y|Y|yes|YES)
                WEB_PANEL_ENABLED=1
                printf "Web 控制台端口 [默认 8888]: "
                _port=""
                read -r _port < /dev/tty 2>/dev/null || _port=""
                WEB_PORT="${_port:-8888}"
                echo -e "${GREEN}将启用 Web 控制台 (端口 $WEB_PORT, 监听 $WEB_BIND, 允许局域网访问)${NC}"
                echo ""
                ;;
            *)
                echo -e "${BLUE}已跳过 Web 控制台 (之后可执行: ccs server start --web-port 8888 --web-bind 0.0.0.0)${NC}"
                echo ""
                ;;
        esac
    fi
fi

# 总步骤数
TOTAL_STEPS=12

# 目录定义
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CC_SWITCH_DIR="$HOME/.cc-switch"
LOG_DIR="$CC_SWITCH_DIR/logs"
DB_PATH="$CC_SWITCH_DIR/cc-switch.db"
INSTALL_DIR="$HOME/.local/bin"

mkdir -p "$LOG_DIR"
mkdir -p "$CC_SWITCH_DIR"

# ============================================================================
# 步骤 1: 检查 Git 
# ============================================================================
CURRENT_STEP=1
step_running $CURRENT_STEP $TOTAL_STEPS "检查 Git 版本控制"

if [ -d "$SCRIPT_DIR/.git" ]; then
    GIT_BRANCH=$(cd "$SCRIPT_DIR" && git rev-parse --abbrev-ref HEAD 2>/dev/null || echo "unknown")
    GIT_COMMIT=$(cd "$SCRIPT_DIR" && git rev-parse --short HEAD 2>/dev/null || echo "unknown")

    # 可选：拉取当前分支最新提交
    if [ "${ARG_UPDATE:-0}" -eq 1 ]; then
        echo ""
        echo -e "${YELLOW}正在拉取当前分支更新...${NC}"
        cd "$SCRIPT_DIR"

        # 保存本地修改
        if ! git diff --quiet || ! git diff --cached --quiet; then
            echo -e "${YELLOW}检测到本地修改，正在暂存...${NC}"
            git stash push -m "Auto-stash before update $(date +%Y%m%d_%H%M%S)"
        fi

        # 拉取并 rebase（跟随当前分支）
        if git pull --rebase 2>&1 | tee "$LOG_DIR/git_update.log"; then
            echo -e "${GREEN}✓ 当前分支更新已拉取${NC}"

            # 恢复本地修改
            if git stash list | grep -q "Auto-stash before update"; then
                echo -e "${YELLOW}正在恢复本地修改...${NC}"
                if git stash pop; then
                    echo -e "${GREEN}✓ 本地修改已恢复${NC}"
                else
                    echo -e "${RED}⚠ 合并冲突，请手动解决: git status${NC}"
                    echo -e "${YELLOW}冲突解决后运行: git stash drop${NC}"
                fi
            fi
        else
            echo -e "${RED}✗ 更新失败，查看日志: cat $LOG_DIR/git_update.log${NC}"
            exit 1
        fi
    fi

    step_done $CURRENT_STEP $TOTAL_STEPS "Git 仓库 ($GIT_BRANCH@$GIT_COMMIT)"
else
    step_done $CURRENT_STEP $TOTAL_STEPS "Git 未初始化 (非仓库模式)"
fi

# ============================================================================
# 步骤 2: 检查并安装 Rust 工具链
# ============================================================================
CURRENT_STEP=2
step_running $CURRENT_STEP $TOTAL_STEPS "检查 Rust 工具链"

if [ -n "${ARG_PREBUILT_BIN:-}" ]; then
    step_done $CURRENT_STEP $TOTAL_STEPS "跳过 Rust（使用预构建二进制）"
else
if ! command -v cargo &> /dev/null; then
    echo ""
    echo -e "${YELLOW}Rust 未安装，正在自动安装...${NC}"

    # 下载Rust安装脚本
    RUSTUP_INIT="$(mktemp -t rustup-init.XXXXXX.sh)"
    if download_with_retry "https://sh.rustup.rs" "$RUSTUP_INIT" 3; then
        if sh "$RUSTUP_INIT" -y --default-toolchain stable 2>&1 | tee "$LOG_DIR/rust_install.log" | grep -E "info:|Updating|Installing" | tail -5; then
            rm -f "$RUSTUP_INIT"

            if [ -f "$HOME/.cargo/env" ]; then
                source "$HOME/.cargo/env"
            fi

            if command -v cargo &> /dev/null; then
                RUST_VERSION=$(rustc --version)
                step_done $CURRENT_STEP $TOTAL_STEPS "安装 Rust 工具链 ($RUST_VERSION)"
            else
                step_error $CURRENT_STEP $TOTAL_STEPS "Rust 安装失败"
                echo "查看日志: cat $LOG_DIR/rust_install.log"
                exit 1
            fi
        else
            rm -f "$RUSTUP_INIT"
            step_error $CURRENT_STEP $TOTAL_STEPS "Rust 安装失败"
            echo "查看日志: cat $LOG_DIR/rust_install.log"
            exit 1
        fi
    else
        step_error $CURRENT_STEP $TOTAL_STEPS "无法下载 Rust 安装脚本"
        echo "请检查网络连接或手动安装: https://rustup.rs/"
        exit 1
    fi
else
    RUST_VERSION=$(rustc --version)
    step_done $CURRENT_STEP $TOTAL_STEPS "Rust 工具链 ($RUST_VERSION)"
fi
fi

# ============================================================================
# 步骤 3: 检查 Node.js 环境 (前端构建)
# ============================================================================
CURRENT_STEP=3
step_running $CURRENT_STEP $TOTAL_STEPS "检查 Node.js 环境"

if [ "$CLI_MODE" = "true" ]; then
    step_done $CURRENT_STEP $TOTAL_STEPS "跳过 Node.js（CLI 模式不需要前端构建）"
else
NODE_CMD=""
if command -v node &> /dev/null; then
    NODE_VERSION=$(node --version)
    NODE_MAJOR_VERSION=$(echo "$NODE_VERSION" | sed 's/v\([0-9]*\).*/\1/')

    if [ "$NODE_MAJOR_VERSION" -lt 16 ]; then
        echo ""
        echo -e "${YELLOW}Node.js 版本过低 ($NODE_VERSION)，需要 16+${NC}"
        echo -e "${YELLOW}正在尝试自动安装最新版本...${NC}"
    else
        NODE_CMD="node"
        step_done $CURRENT_STEP $TOTAL_STEPS "Node.js 环境 ($NODE_VERSION)"
    fi
fi

# 如果Node.js未安装或版本过低，尝试自动安装
if [ -z "$NODE_CMD" ]; then
    echo ""
    echo -e "${YELLOW}正在自动安装 Node.js...${NC}"

    # 尝试使用 nvm 安装
    if [ -f "$HOME/.nvm/nvm.sh" ]; then
        source "$HOME/.nvm/nvm.sh"
        if nvm install --lts > "$LOG_DIR/node_install.log" 2>&1; then
            nvm use --lts
            NODE_VERSION=$(node --version)
            step_done $CURRENT_STEP $TOTAL_STEPS "安装 Node.js ($NODE_VERSION)"
            NODE_CMD="node"
        fi
    fi

    # 如果nvm不可用，尝试使用NodeSource仓库
    if [ -z "$NODE_CMD" ]; then
        case $PKG_MANAGER in
            apt)
                # 使用NodeSource仓库安装Node.js 18 LTS
                NODESOURCE_SETUP="$(mktemp -t nodesource-setup.XXXXXX.sh)"
                if download_with_retry "https://deb.nodesource.com/setup_18.x" "$NODESOURCE_SETUP" 3; then
                    if $SUDO bash "$NODESOURCE_SETUP" > "$LOG_DIR/node_install.log" 2>&1; then
                        if DEBIAN_FRONTEND=noninteractive $SUDO apt-get install -y --no-install-recommends nodejs > "$LOG_DIR/node_install.log" 2>&1; then
                            NODE_VERSION=$(node --version)
                            step_done $CURRENT_STEP $TOTAL_STEPS "安装 Node.js ($NODE_VERSION)"
                            NODE_CMD="node"
                        fi
                    fi
                    rm -f "$NODESOURCE_SETUP"
                fi
                ;;
            dnf|yum)
                # 使用NodeSource仓库安装Node.js 18 LTS
                if $SUDO $PKG_MANAGER module -y reset nodejs > /dev/null 2>&1; then
                    $SUDO $PKG_MANAGER module -y enable nodejs:18 > /dev/null 2>&1
                fi
                if $SUDO $PKG_MANAGER install -y nodejs > "$LOG_DIR/node_install.log" 2>&1; then
                    NODE_VERSION=$(node --version)
                    step_done $CURRENT_STEP $TOTAL_STEPS "安装 Node.js ($NODE_VERSION)"
                    NODE_CMD="node"
                fi
                ;;
            pacman)
                if $SUDO pacman -S --noconfirm nodejs npm > "$LOG_DIR/node_install.log" 2>&1; then
                    NODE_VERSION=$(node --version)
                    step_done $CURRENT_STEP $TOTAL_STEPS "安装 Node.js ($NODE_VERSION)"
                    NODE_CMD="node"
                fi
                ;;
        esac
    fi

    # 如果仍然无法安装
    if [ -z "$NODE_CMD" ]; then
        step_error $CURRENT_STEP $TOTAL_STEPS "Node.js 安装失败"
        echo ""
        echo -e "${RED}无法自动安装 Node.js${NC}"
        echo -e "${YELLOW}请手动安装 Node.js 16+ :${NC}"
        echo "  - 使用 nvm: https://github.com/nvm-sh/nvm"
        echo "  - 官方下载: https://nodejs.org/"
        echo "  - 查看日志: cat $LOG_DIR/node_install.log"
        exit 1
    fi
fi
fi

# 检查 pnpm
ensure_pnpm() {
    local PNPM_SETUP_LOG="$LOG_DIR/pnpm_setup.log"
    if command -v pnpm >/dev/null 2>&1; then
        return 0
    fi

    echo ""
    echo -e "${YELLOW}pnpm 未安装，正在安装/启用...${NC}"

    # 为了兼容 tauri.conf.json 中的 beforeBuildCommand（直接调用 pnpm），
    # 在 pnpm 不在 PATH 时创建一个本地 shim，让子进程也能找到 pnpm。
    local PNPM_SHIM_DIR="$SCRIPT_DIR/.ccs-shims"
    mkdir -p "$PNPM_SHIM_DIR" 2>/dev/null || true

    # 优先使用 corepack（Node 16+ 自带），避免系统缺少 npm 时失败
    if command -v corepack >/dev/null 2>&1; then
        {
            echo "== corepack enable =="
            corepack enable
            echo "== corepack prepare pnpm@latest --activate =="
            corepack prepare pnpm@latest --activate
        } >> "$PNPM_SETUP_LOG" 2>&1 || true

        # 如果 pnpm 仍不在 PATH，则创建 shim：pnpm -> corepack pnpm
        if ! command -v pnpm >/dev/null 2>&1; then
            cat > "$PNPM_SHIM_DIR/pnpm" <<'EOF'
#!/bin/sh
exec corepack pnpm "$@"
EOF
            chmod +x "$PNPM_SHIM_DIR/pnpm" 2>/dev/null || true
            export PATH="$PNPM_SHIM_DIR:$PATH"
        fi
    fi

    # 回退：使用 npm 全局安装
    if ! command -v pnpm >/dev/null 2>&1 && command -v npm >/dev/null 2>&1; then
        {
            echo "== npm install -g pnpm =="
            npm install -g pnpm
        } >> "$PNPM_SETUP_LOG" 2>&1 || true
    fi

    # 回退：系统安装 npm（某些 Ubuntu 环境可能只有 node 没有 npm）
    if ! command -v pnpm >/dev/null 2>&1 && ! command -v npm >/dev/null 2>&1; then
        case "$PKG_MANAGER" in
            apt)
                {
                    echo "== apt-get install npm =="
                    DEBIAN_FRONTEND=noninteractive $SUDO apt-get update -qq || true
                    DEBIAN_FRONTEND=noninteractive $SUDO apt-get install -y --no-install-recommends npm
                    echo "== npm install -g pnpm =="
                    npm install -g pnpm
                } >> "$PNPM_SETUP_LOG" 2>&1 || true
                ;;
        esac
    fi

    if command -v pnpm >/dev/null 2>&1; then
        echo -e "${GREEN}✓ pnpm 已就绪${NC}"
        return 0
    fi

    if command -v corepack >/dev/null 2>&1; then
        echo -e "${YELLOW}⚠ pnpm 命令未出现在 PATH，将尝试使用 corepack pnpm 执行后续步骤${NC}"
        if [ -x "$PNPM_SHIM_DIR/pnpm" ]; then
            echo -e "${YELLOW}提示：已创建本地 shim: $PNPM_SHIM_DIR/pnpm（供 tauri beforeBuildCommand 使用）${NC}"
            export PATH="$PNPM_SHIM_DIR:$PATH"
        fi
        return 0
    fi

    echo -e "${YELLOW}⚠ pnpm 未能自动安装/启用，将尝试使用 npm 安装依赖（如可用）${NC}"
    if ! command -v npm >/dev/null 2>&1; then
        echo -e "${RED}✗ 未找到 npm，无法继续安装前端依赖${NC}"
        echo "查看日志: cat $PNPM_SETUP_LOG"
        echo ""
        echo -e "${YELLOW}建议（任选其一）：${NC}"
        echo "  1. 安装 npm: sudo apt-get install -y npm"
        echo "  2. 启用 corepack: corepack enable && corepack prepare pnpm@latest --activate"
        echo "  3. 重新运行本脚本"
        exit 1
    fi
}

PNPM_RUN=()
if [ "$CLI_MODE" != "true" ]; then
    ensure_pnpm

    # 选择 pnpm 执行器：优先 pnpm，其次 corepack pnpm
    if command -v pnpm >/dev/null 2>&1; then
        PNPM_RUN=(pnpm)
    elif command -v corepack >/dev/null 2>&1; then
        PNPM_RUN=(corepack pnpm)
    fi
fi

# ============================================================================
# 步骤 4: 安装前端依赖
# ============================================================================
CURRENT_STEP=4
step_running $CURRENT_STEP $TOTAL_STEPS "安装前端依赖"

cd "$SCRIPT_DIR"
FRONTEND_INSTALL_LOG="$LOG_DIR/frontend_install.log"

if [ "$CLI_MODE" = "true" ]; then
    step_done $CURRENT_STEP $TOTAL_STEPS "跳过前端依赖（CLI 模式）"
else
INSTALL_CMD=()
if [ ${#PNPM_RUN[@]} -gt 0 ]; then
    INSTALL_CMD=("${PNPM_RUN[@]}" install)
else
    INSTALL_CMD=(npm install)
fi

if [ ! -d "node_modules" ] || [ "package.json" -nt "node_modules" ]; then
    # 尝试安装，最多重试2次
    RETRY=0
    MAX_RETRIES=2
    SUCCESS=false

    while [ $RETRY -le $MAX_RETRIES ] && [ "$SUCCESS" = false ]; do
        if [ $RETRY -gt 0 ]; then
            echo ""
            echo -e "${YELLOW}依赖安装失败，重试 ${RETRY}/${MAX_RETRIES}...${NC}"
            # 清理node_modules可能损坏的文件
            rm -rf node_modules/.cache 2>/dev/null || true
        fi

        if [ "${INSTALL_CMD[0]}" = "npm" ] && ! command -v npm >/dev/null 2>&1; then
            step_error $CURRENT_STEP $TOTAL_STEPS "前端依赖安装失败"
            echo ""
            echo -e "${RED}未找到 npm 命令，无法执行依赖安装${NC}"
            echo -e "${YELLOW}请先安装 npm 或启用 corepack/pnpm 后重试${NC}"
            echo "  - 安装 npm: sudo apt-get install -y npm"
            echo "  - 或启用 pnpm: corepack prepare pnpm@latest --activate"
            echo "查看日志: cat $LOG_DIR/pnpm_setup.log"
            exit 1
        fi

        if "${INSTALL_CMD[@]}" > "$FRONTEND_INSTALL_LOG" 2>&1; then
            SUCCESS=true
            step_done $CURRENT_STEP $TOTAL_STEPS "安装前端依赖"
        else
            RETRY=$((RETRY + 1))
        fi
    done

    if [ "$SUCCESS" = false ]; then
        step_error $CURRENT_STEP $TOTAL_STEPS "前端依赖安装失败"
        echo ""
        echo -e "${RED}前端依赖安装失败，已尝试 ${MAX_RETRIES} 次重试${NC}"
        echo "查看日志: cat $FRONTEND_INSTALL_LOG"
        echo ""
        echo -e "${YELLOW}建议尝试:${NC}"
        echo "  1. 检查网络连接"
        echo "  2. 清理缓存: rm -rf node_modules package-lock.json pnpm-lock.yaml"
        echo "  3. 手动安装: cd $SCRIPT_DIR && (pnpm install || corepack pnpm install || npm install)"
        echo "  4. 如缺少 npm: sudo apt-get install -y npm"
        echo "  5. 如需使用 pnpm: corepack enable && corepack prepare pnpm@latest --activate"
        exit 1
    fi
else
    step_done $CURRENT_STEP $TOTAL_STEPS "前端依赖 (使用缓存)"
fi
fi

# ============================================================================
# 步骤 5: 编译 Tauri 应用
# ============================================================================
CURRENT_STEP=5
step_running $CURRENT_STEP $TOTAL_STEPS "编译 Tauri 应用，这可能需要几分钟..."

cd "$SCRIPT_DIR"

if [ -n "${ARG_PREBUILT_BIN:-}" ]; then
    BIN_PATH="$ARG_PREBUILT_BIN"
    step_done $CURRENT_STEP $TOTAL_STEPS "使用预构建二进制"
else

# 统一指定 Cargo Target 目录，避免：
# - 仓库路径变更导致 tauri permissions 绝对路径缓存失效（典型报错：failed to read plugin permissions）
# - 与其它同名项目/工作区 target 相互污染
#
# 允许用户通过环境变量覆盖：
# - CC_SWITCH_CARGO_TARGET_DIR（优先）
# - CARGO_TARGET_DIR（次优先）
#
# 默认使用仓库内的 .cargo-target（保留历史 src-tauri/target 不动，避免“删除文件”争议）
CCS_CARGO_TARGET_DIR="${CC_SWITCH_CARGO_TARGET_DIR:-${CARGO_TARGET_DIR:-$SCRIPT_DIR/.cargo-target/src-tauri}}"
export CARGO_TARGET_DIR="$CCS_CARGO_TARGET_DIR"

# 检查是否需要重新编译
#
# 说明：
# - 不能用目录 mtime 判断（不可靠），改为以最终二进制文件作为基准。
# - 增加“构建戳”机制：记录当前 git commit，避免出现“代码更新了但仍复用旧二进制”的情况。
# - 增加“特征字符串”检测：如果二进制里还包含调试期标记（如 request trace），强制重编译。
BIN_PATH="$CARGO_TARGET_DIR/release/cc-switch"
STAMP_FILE="$CARGO_TARGET_DIR/release/.build_git_commit"
NEED_REBUILD=false

CURRENT_COMMIT=""
if [ -d ".git" ]; then
    CURRENT_COMMIT=$(git rev-parse HEAD 2>/dev/null || echo "")
fi

if [ ! -f "$BIN_PATH" ]; then
    NEED_REBUILD=true
else
    # 1) 源码/配置文件比二进制新，触发重编译
    if [ -n "$(find src-tauri/src -name '*.rs' -newer "$BIN_PATH" 2>/dev/null)" ]; then
        NEED_REBUILD=true
    elif [ -f "src-tauri/Cargo.toml" ] && [ "src-tauri/Cargo.toml" -nt "$BIN_PATH" ]; then
        NEED_REBUILD=true
    elif [ -f "src-tauri/Cargo.lock" ] && [ "src-tauri/Cargo.lock" -nt "$BIN_PATH" ]; then
        NEED_REBUILD=true
    fi

    # 2) git commit 变化，触发重编译（更可靠地覆盖“文件时间戳异常”的情况）
    if [ -n "$CURRENT_COMMIT" ]; then
        if [ ! -f "$STAMP_FILE" ] || [ "$(cat "$STAMP_FILE" 2>/dev/null)" != "$CURRENT_COMMIT" ]; then
            NEED_REBUILD=true
        fi
    fi

    # 3) 二进制仍包含调试期特征字符串，触发重编译（避免部署后仍输出 request trace）
    if command -v strings >/dev/null 2>&1; then
        if strings "$BIN_PATH" 2>/dev/null | grep -q "\[Codex\] request trace"; then
            NEED_REBUILD=true
        fi
        if strings "$BIN_PATH" 2>/dev/null | grep -q "\[Forwarder\] invalid_claude_config"; then
            NEED_REBUILD=true
        fi
    fi
fi

if [ "$NEED_REBUILD" = true ]; then
    # 强制删除旧二进制，避免后续步骤误用旧文件
    rm -f "$BIN_PATH" 2>/dev/null || true

    BUILD_START=$(date +%s)
    if [ "$CLI_MODE" = "true" ]; then
        # CLI 模式只需要 Rust 二进制，直接 cargo build 可规避 updater 签名要求
        if cargo build --release --manifest-path src-tauri/Cargo.toml > "$LOG_DIR/tauri_build.log" 2>&1; then
            BUILD_SUCCESS=true
        else
            BUILD_SUCCESS=false
            if [ -f "$LOG_DIR/tauri_build.log" ]; then
                LAST_ERROR=$(tail -20 "$LOG_DIR/tauri_build.log" | grep -i "error" | head -5)
            fi
        fi
    else
        # 运行Tauri构建，跳过 AppImage 打包（避免网络下载问题）
        # 只生成 deb 和 rpm 包
        if [ ${#PNPM_RUN[@]} -eq 0 ]; then
            step_error $CURRENT_STEP $TOTAL_STEPS "编译失败"
            echo -e "${RED}未找到 pnpm/corepack，无法执行 tauri build${NC}"
            echo -e "${YELLOW}请先安装 pnpm 或启用 corepack:${NC}"
            echo "  corepack prepare pnpm@latest --activate"
            exit 1
        fi

        if "${PNPM_RUN[@]}" tauri build --bundles deb,rpm > "$LOG_DIR/tauri_build.log" 2>&1; then
            BUILD_SUCCESS=true
        else
            BUILD_SUCCESS=false
            # 编译失败，尝试读取错误信息
            if [ -f "$LOG_DIR/tauri_build.log" ]; then
                LAST_ERROR=$(tail -20 "$LOG_DIR/tauri_build.log" | grep -i "error" | head -5)
            fi
        fi
    fi
    BUILD_END=$(date +%s)
    BUILD_TIME=$((BUILD_END - BUILD_START))

    # 检查二进制文件是否成功生成
    if [ -f "$BIN_PATH" ] && [ "$BUILD_SUCCESS" = true ]; then
        # 写入构建戳
        if [ -n "$CURRENT_COMMIT" ]; then
            echo -n "$CURRENT_COMMIT" > "$STAMP_FILE" 2>/dev/null || true
        fi

        # 验证二进制文件是否可执行
        if [ -x "$BIN_PATH" ]; then
            step_done $CURRENT_STEP $TOTAL_STEPS "编译完成 (耗时 ${BUILD_TIME}s)"
        else
            chmod +x "$BIN_PATH"
            step_done $CURRENT_STEP $TOTAL_STEPS "编译完成 (耗时 ${BUILD_TIME}s)"
        fi
    else
        step_error $CURRENT_STEP $TOTAL_STEPS "编译失败"
        echo ""
        echo -e "${RED}编译失败${NC}"
        echo "查看完整日志: cat $LOG_DIR/tauri_build.log"

        if [ -n "$LAST_ERROR" ]; then
            echo ""
            echo -e "${YELLOW}最近的错误信息:${NC}"
            echo "$LAST_ERROR"
        fi

        echo ""
        echo -e "${YELLOW}建议尝试:${NC}"
        echo "  1. 检查系统依赖是否完整安装"
        echo "  2. 清理构建缓存: rm -rf src-tauri/target \"$CARGO_TARGET_DIR\""
        echo "  3. 手动编译（CLI）: cd $SCRIPT_DIR && cargo build --release --manifest-path src-tauri/Cargo.toml"
        echo "  4. 手动编译（GUI）: cd $SCRIPT_DIR && (pnpm tauri build || corepack pnpm tauri build)"
        exit 1
    fi
else
    step_done $CURRENT_STEP $TOTAL_STEPS "Tauri 应用 (使用缓存)"
fi
fi

# ============================================================================
# 步骤 6: 数据库
# ============================================================================
CURRENT_STEP=6
step_running $CURRENT_STEP $TOTAL_STEPS "数据库迁移检查"

if [ -f "$DB_PATH" ]; then
    # 读取当前数据库版本
    DB_VERSION=$(sqlite3 "$DB_PATH" "PRAGMA user_version" 2>/dev/null || echo "0")

    if [ "$DB_VERSION" -lt 4 ]; then
        # 迁移将在应用启动时自动执行，不再创建备份
        step_done $CURRENT_STEP $TOTAL_STEPS "数据库迁移"
    else
        step_done $CURRENT_STEP $TOTAL_STEPS "数据库版本 (v$DB_VERSION)"
    fi
else
    step_done $CURRENT_STEP $TOTAL_STEPS "数据库"
fi

# ============================================================================
# 步骤 7: 停止旧服务
# ============================================================================
CURRENT_STEP=7
step_running $CURRENT_STEP $TOTAL_STEPS "停止旧服务"

# 临时禁用 set -e，因为停止服务的命令可能返回非零退出码
set +e
# 同时临时禁用 ERR trap（stop 过程中会有竞态：进程消失导致 /proc 读取失败）
PREV_ERR_TRAP="$(trap -p ERR || true)"
trap '' ERR

# 获取当前脚本的 PID，避免误杀自己
CURRENT_SCRIPT_PID=$$

# 通过 PID 文件停止旧服务（更可靠）
if [ -f "$CC_SWITCH_DIR/server.pid" ]; then
    OLD_PID=$(cat "$CC_SWITCH_DIR/server.pid" 2>/dev/null)
    if [ -n "$OLD_PID" ] && [ "$OLD_PID" != "$CURRENT_SCRIPT_PID" ] && ps -p "$OLD_PID" > /dev/null 2>&1; then
        kill "$OLD_PID" 2>/dev/null
    fi
    rm -f "$CC_SWITCH_DIR/server.pid" 2>/dev/null
fi

if [ -f "$CC_SWITCH_DIR/proxy.pid" ]; then
    OLD_PID=$(cat "$CC_SWITCH_DIR/proxy.pid" 2>/dev/null)
    if [ -n "$OLD_PID" ] && [ "$OLD_PID" != "$CURRENT_SCRIPT_PID" ] && ps -p "$OLD_PID" > /dev/null 2>&1; then
        kill "$OLD_PID" 2>/dev/null
    fi
    rm -f "$CC_SWITCH_DIR/proxy.pid" 2>/dev/null
fi

# 通用进程清理 - 只杀死 cc-switch 二进制进程，不杀死包含 cc-switch 路径的脚本
# 使用更精确的匹配：只匹配以 cc-switch 结尾的可执行文件或 "cc-switch server" 命令
for pid in $(list_pids_matching "cc-switch"); do
    # 跳过当前脚本进程
    if [ "$pid" = "$CURRENT_SCRIPT_PID" ]; then
        continue
    fi
    # 检查进程命令行，只杀死真正的 cc-switch 服务进程
    CMDLINE="$(cat "/proc/$pid/cmdline" 2>/dev/null | tr '\0' ' ' || true)"
    if echo "$CMDLINE" | grep -qE "(^|/)cc-switch(-cli)?( |$)|(cc-switch|cc-switch-cli) server" 2>/dev/null; then
        kill "$pid" 2>/dev/null || true
    fi
done

# 等待进程完全退出
sleep 2

# 确认进程已停止，如果还在运行则强制终止
for pid in $(list_pids_matching "cc-switch"); do
    if [ "$pid" = "$CURRENT_SCRIPT_PID" ]; then
        continue
    fi
    CMDLINE="$(cat "/proc/$pid/cmdline" 2>/dev/null | tr '\0' ' ' || true)"
    if echo "$CMDLINE" | grep -qE "(^|/)cc-switch(-cli)?( |$)|(cc-switch|cc-switch-cli) server" 2>/dev/null; then
        echo ""
        echo -e "${YELLOW}强制终止残留进程 (PID: $pid)...${NC}"
        kill -9 "$pid" 2>/dev/null || true
    fi
done

sleep 1

# 重新启用 set -e
set -e
# 恢复 ERR trap
if [ -n "$PREV_ERR_TRAP" ]; then
    eval "$PREV_ERR_TRAP" || true
fi

step_done $CURRENT_STEP $TOTAL_STEPS "停止旧服务"

# ============================================================================
# 步骤 8: 安装二进制到系统路径
# ============================================================================
CURRENT_STEP=8
step_running $CURRENT_STEP $TOTAL_STEPS "安装到系统路径"

mkdir -p "$INSTALL_DIR"

# 查找编译后的二进制文件（源码编译或 --prebuilt）
SOURCE_GUI_BIN=""
SOURCE_CLI_BIN=""

if [ -n "${ARG_PREBUILT_BIN:-}" ]; then
    PREBUILT_DIR="$(dirname "$ARG_PREBUILT_BIN")"
    PREBUILT_BASE="$(basename "$ARG_PREBUILT_BIN")"

    case "$PREBUILT_BASE" in
        cc-switch)
            SOURCE_GUI_BIN="$ARG_PREBUILT_BIN"
            ;;
        cc-switch-cli)
            SOURCE_CLI_BIN="$ARG_PREBUILT_BIN"
            ;;
    esac

    if [ -z "$SOURCE_GUI_BIN" ] && [ -f "$PREBUILT_DIR/cc-switch" ]; then
        SOURCE_GUI_BIN="$PREBUILT_DIR/cc-switch"
    fi
    if [ -z "$SOURCE_CLI_BIN" ] && [ -f "$PREBUILT_DIR/cc-switch-cli" ]; then
        SOURCE_CLI_BIN="$PREBUILT_DIR/cc-switch-cli"
    fi
elif [ -n "${CARGO_TARGET_DIR:-}" ]; then
    [ -f "$CARGO_TARGET_DIR/release/cc-switch" ] && SOURCE_GUI_BIN="$CARGO_TARGET_DIR/release/cc-switch"
    [ -f "$CARGO_TARGET_DIR/release/cc-switch-cli" ] && SOURCE_CLI_BIN="$CARGO_TARGET_DIR/release/cc-switch-cli"
else
    [ -f "src-tauri/target/release/cc-switch" ] && SOURCE_GUI_BIN="src-tauri/target/release/cc-switch"
    [ -f "src-tauri/target/release/cc-switch-cli" ] && SOURCE_CLI_BIN="src-tauri/target/release/cc-switch-cli"
fi

CLI_LAUNCHER="$INSTALL_DIR/cc-switch"

if [ "$CLI_MODE" = "true" ]; then
    ACTIVE_CLI_BIN="$SOURCE_CLI_BIN"
    if [ -z "$ACTIVE_CLI_BIN" ] && [ -n "$SOURCE_GUI_BIN" ]; then
        ACTIVE_CLI_BIN="$SOURCE_GUI_BIN"
    fi

    if [ -z "$ACTIVE_CLI_BIN" ] || [ ! -f "$ACTIVE_CLI_BIN" ]; then
        step_error $CURRENT_STEP $TOTAL_STEPS "安装失败"
        echo -e "${RED}未找到可安装的 CLI 二进制文件${NC}"
        echo -e "${YELLOW}提示：请先编译 cc-switch-cli，或使用 --prebuilt 指向包含 cc-switch-cli 的目录${NC}"
        exit 1
    fi

    install -m 755 "$ACTIVE_CLI_BIN" "$INSTALL_DIR/cc-switch-cli"
    install -m 755 "$ACTIVE_CLI_BIN" "$INSTALL_DIR/cc-switch"
    ln -sfn "$INSTALL_DIR/cc-switch-cli" "$INSTALL_DIR/ccs"
    ln -sfn "$INSTALL_DIR/cc-switch-cli" "$INSTALL_DIR/csc"
    CLI_LAUNCHER="$INSTALL_DIR/cc-switch-cli"

    if [ -n "$SOURCE_GUI_BIN" ] && [ "$SOURCE_GUI_BIN" != "$ACTIVE_CLI_BIN" ] && [ -f "$SOURCE_GUI_BIN" ]; then
        install -m 755 "$SOURCE_GUI_BIN" "$INSTALL_DIR/cc-switch-gui"
    fi
else
    ACTIVE_GUI_BIN="$SOURCE_GUI_BIN"
    if [ -z "$ACTIVE_GUI_BIN" ] && [ -n "$SOURCE_CLI_BIN" ]; then
        ACTIVE_GUI_BIN="$SOURCE_CLI_BIN"
    fi

    if [ -z "$ACTIVE_GUI_BIN" ] || [ ! -f "$ACTIVE_GUI_BIN" ]; then
        step_error $CURRENT_STEP $TOTAL_STEPS "安装失败"
        echo -e "${RED}未找到可安装的 GUI 二进制文件${NC}"
        echo -e "${YELLOW}提示：请先编译 cc-switch，或使用 --prebuilt 指向包含 cc-switch 的目录${NC}"
        exit 1
    fi

    install -m 755 "$ACTIVE_GUI_BIN" "$INSTALL_DIR/cc-switch"

    if [ -n "$SOURCE_CLI_BIN" ] && [ "$SOURCE_CLI_BIN" != "$ACTIVE_GUI_BIN" ] && [ -f "$SOURCE_CLI_BIN" ]; then
        install -m 755 "$SOURCE_CLI_BIN" "$INSTALL_DIR/cc-switch-cli"
        ln -sfn "$INSTALL_DIR/cc-switch-cli" "$INSTALL_DIR/ccs"
        ln -sfn "$INSTALL_DIR/cc-switch-cli" "$INSTALL_DIR/csc"
        CLI_LAUNCHER="$INSTALL_DIR/cc-switch-cli"
    else
        ln -sfn "$INSTALL_DIR/cc-switch" "$INSTALL_DIR/ccs"
        ln -sfn "$INSTALL_DIR/cc-switch" "$INSTALL_DIR/csc"
    fi
fi

# 添加到 PATH（兼容 bash/zsh 以及 login shell 场景）
declare -a SHELL_CONFIGS=()
declare -A __SHELL_CONFIG_SEEN=()

# bash：非 login shell 通常读取 ~/.bashrc；login shell 通常读取 ~/.profile 或 ~/.bash_profile
for f in "$HOME/.bashrc" "$HOME/.profile" "$HOME/.bash_profile"; do
    if [ -n "${__SHELL_CONFIG_SEEN[$f]:-}" ]; then
        continue
    fi
    __SHELL_CONFIG_SEEN["$f"]=1
    SHELL_CONFIGS+=("$f")
done

# zsh：读取 ~/.zshrc（仅当用户使用 zsh 或文件已存在）
if echo "${SHELL:-}" | grep -qi "zsh" || [ -f "$HOME/.zshrc" ]; then
    f="$HOME/.zshrc"
    if [ -z "${__SHELL_CONFIG_SEEN[$f]:-}" ]; then
        __SHELL_CONFIG_SEEN["$f"]=1
        SHELL_CONFIGS+=("$f")
    fi
fi

# 确保配置文件存在并写入 PATH
for cfg in "${SHELL_CONFIGS[@]}"; do
    touch "$cfg" 2>/dev/null || true

    # 兼容旧版本：清理历史遗留的重复片段
    sed -i '/^# CC-Switch PATH$/d' "$cfg" 2>/dev/null || true
    sed -i '/^export PATH="\\$HOME\\/\\.local\\/bin:\\$PATH"$/d' "$cfg" 2>/dev/null || true

    # 新版本：用 begin/end 标记保证幂等（每次部署都会先删除旧块，再写入一次）
    CCS_PATH_BEGIN="# >>> CC-Switch PATH >>>"
    CCS_PATH_END="# <<< CC-Switch PATH <<<"
    sed -i "/^${CCS_PATH_BEGIN}\$/,/^${CCS_PATH_END}\$/d" "$cfg" 2>/dev/null || true

    {
        echo ""
        echo "$CCS_PATH_BEGIN"
        echo 'export PATH="$HOME/.local/bin:$PATH"'
        echo "$CCS_PATH_END"
    } >> "$cfg"
done

step_done $CURRENT_STEP $TOTAL_STEPS "安装到系统路径"

# 将 PROXY_BASE 写入各 CLI 的配置（用于首次写入/端口变更后的重写）
write_cli_configs() {
    local proxy_base="$1"

    # Claude Code：写入独立 env 文件，再由 shell rc source（避免 bash -lc 等 login shell 不读取 ~/.bashrc 导致环境变量丢失）
    local env_file="$CC_SWITCH_DIR/env.sh"
    mkdir -p "$CC_SWITCH_DIR"
cat > "$env_file" <<EOF
# Generated by CC-Switch. Do not edit.
export ANTHROPIC_BASE_URL="${proxy_base}"
export ANTHROPIC_API_KEY="sk-ant-cc-switch-placeholder"
EOF
    chmod 600 "$env_file" 2>/dev/null || true

    # 在各 shell 配置文件中确保 source env_file（尽量不破坏用户已有配置）
    for cfg in "${SHELL_CONFIGS[@]}"; do
        touch "$cfg" 2>/dev/null || true
        # 清理旧的直写配置（仅清理带 CC-Switch 标记的行）
        sed -i '/# CC-Switch Claude CLI 配置/d' "$cfg" 2>/dev/null || true
        sed -i '/export ANTHROPIC_BASE_URL="http:\/\/127\.0\.0\.1:[0-9]\+"/d' "$cfg" 2>/dev/null || true
        sed -i '/ANTHROPIC_API_KEY.*sk-placeholder-managed-by-cc-switch/d' "$cfg" 2>/dev/null || true
        sed -i '/ANTHROPIC_API_KEY.*sk-ant-cc-switch-placeholder/d' "$cfg" 2>/dev/null || true
        sed -i '/# CC-Switch Claude Code env/d' "$cfg" 2>/dev/null || true
        sed -i '/\\.cc-switch\\/env\\.sh/d' "$cfg" 2>/dev/null || true

        CCS_CLAUDE_ENV_BEGIN="# >>> CC-Switch Claude Code env >>>"
        CCS_CLAUDE_ENV_END="# <<< CC-Switch Claude Code env <<<"
        sed -i "/^${CCS_CLAUDE_ENV_BEGIN}\$/,/^${CCS_CLAUDE_ENV_END}\$/d" "$cfg" 2>/dev/null || true

        {
            echo ""
            echo "$CCS_CLAUDE_ENV_BEGIN"
            echo '[ -f "$HOME/.cc-switch/env.sh" ] && . "$HOME/.cc-switch/env.sh"'
            echo "$CCS_CLAUDE_ENV_END"
        } >> "$cfg"
    done

    # Codex CLI
    CODEX_CONFIG_DIR="$HOME/.codex"
    CODEX_CONFIG_FILE="$CODEX_CONFIG_DIR/config.toml"
    mkdir -p "$CODEX_CONFIG_DIR"
    cat > "$CODEX_CONFIG_FILE" <<EOF
model = "gpt-5"
model_provider = "cc-switch"

[model_providers.cc-switch]
name = "CC-Switch Proxy"
base_url = "$proxy_base/v1"
wire_api = "responses"
EOF

    # Gemini CLI
    GEMINI_CONFIG_DIR="$HOME/.gemini"
    GEMINI_ENV_FILE="$GEMINI_CONFIG_DIR/.env"
    mkdir -p "$GEMINI_CONFIG_DIR"
    cat > "$GEMINI_ENV_FILE" <<EOF
GEMINI_API_BASE_URL=$proxy_base
GEMINI_API_KEY=sk-placeholder-managed-by-cc-switch
EOF
}

# ============================================================================
# 步骤 9: 配置环境变量
# ============================================================================
CURRENT_STEP=9
step_running $CURRENT_STEP $TOTAL_STEPS "配置环境变量"

DEFAULT_PORT=15721
PROXY_BIND_HOST="${CC_SWITCH_HOST:-0.0.0.0}"
PROXY_CLIENT_HOST="${CC_SWITCH_CLIENT_HOST:-$PROXY_BIND_HOST}"
if [ "$PROXY_CLIENT_HOST" = "0.0.0.0" ]; then
    PROXY_CLIENT_HOST="127.0.0.1"
fi
REQUESTED_PORT="${CC_SWITCH_PORT:-$DEFAULT_PORT}"

# 使用智能端口查找函数
PROXY_PORT=$(find_available_port "$REQUESTED_PORT" "$PROXY_BIND_HOST")

if [ "$PROXY_PORT" != "$REQUESTED_PORT" ]; then
    echo ""
    echo -e "${YELLOW}端口 $REQUESTED_PORT 已被占用，已自动切换到端口 $PROXY_PORT${NC}"
fi

PROXY_BASE="http://${PROXY_CLIENT_HOST}:${PROXY_PORT}"

write_cli_configs "$PROXY_BASE"

step_done $CURRENT_STEP $TOTAL_STEPS "配置环境变量"

# ============================================================================
# 步骤 10: 启动代理服务
# ============================================================================
CURRENT_STEP=10
step_running $CURRENT_STEP $TOTAL_STEPS "启动代理服务"

cd "$SCRIPT_DIR"

# 检查是否使用 CLI 模式
if [ "$CLI_MODE" = "true" ]; then
    SERVER_STARTED=false
    PORT_RETRY=0
    MAX_PORT_RETRY=10

    while [ $PORT_RETRY -le $MAX_PORT_RETRY ]; do
        # CLI 模式：自动启动无头服务器（按需附加 Web 控制台 --web-port/--web-bind）
        CCS_START_ARGS=(server start --host "$PROXY_BIND_HOST" --port "$PROXY_PORT")
        if [ "${WEB_PANEL_ENABLED:-0}" -eq 1 ] && [ -n "${WEB_PORT:-}" ]; then
            CCS_START_ARGS+=(--web-port "$WEB_PORT" --web-bind "$WEB_BIND")
        fi
        nohup "$CLI_LAUNCHER" "${CCS_START_ARGS[@]}" >> "$LOG_DIR/server.log" 2>&1 &

        # 等待服务启动（最多10秒）
        WAIT_TIME=0
        MAX_WAIT=10

        while [ $WAIT_TIME -lt $MAX_WAIT ]; do
            sleep 1
            WAIT_TIME=$((WAIT_TIME + 1))

            if [ -f "$CC_SWITCH_DIR/server.pid" ]; then
                SERVER_PID=$(cat "$CC_SWITCH_DIR/server.pid")
                if ps -p "$SERVER_PID" > /dev/null 2>&1; then
                    if command -v ss &> /dev/null; then
                        if ss -tln | grep -Eq ":[[:space:]]*${PROXY_PORT}([[:space:]]|$)"; then
                            SERVER_STARTED=true
                            break
                        fi
                    elif command -v netstat &> /dev/null; then
                        if netstat -tln | grep -Eq ":[[:space:]]*${PROXY_PORT}([[:space:]]|$)"; then
                            SERVER_STARTED=true
                            break
                        fi
                    elif command -v lsof &> /dev/null; then
                        if lsof -Pi :"$PROXY_PORT" -sTCP:LISTEN -t >/dev/null 2>&1; then
                            SERVER_STARTED=true
                            break
                        fi
                    else
                        # 无工具可用，仅检查进程存在
                        SERVER_STARTED=true
                        break
                    fi
                fi
            fi
        done

        if [ "$SERVER_STARTED" = true ]; then
            break
        fi

        # 若日志提示端口占用，则自动换端口再试（避免“检测端口空闲”误判）
        if [ -f "$LOG_DIR/server.log" ] && tail -50 "$LOG_DIR/server.log" | grep -qiE 'Address already in use|os error 98|地址绑定失败'; then
            PORT_RETRY=$((PORT_RETRY + 1))
            NEW_PORT=$(find_available_port $((PROXY_PORT + 1)) "$PROXY_BIND_HOST")
            if [ "$NEW_PORT" = "$PROXY_PORT" ]; then
                break
            fi
            PROXY_PORT="$NEW_PORT"
            PROXY_BASE="http://${PROXY_CLIENT_HOST}:${PROXY_PORT}"
            echo -e "${YELLOW}检测到端口占用，自动切换到端口 $PROXY_PORT 并重试启动...${NC}"
            write_cli_configs "$PROXY_BASE"
            continue
        fi

        break
    done

    if [ "$SERVER_STARTED" = true ]; then
        SERVER_PID=$(cat "$CC_SWITCH_DIR/server.pid")
        step_done $CURRENT_STEP $TOTAL_STEPS "启动代理服务 (CLI模式, PID:$SERVER_PID, 监听:$PROXY_BIND_HOST:$PROXY_PORT)"
    else
        step_error $CURRENT_STEP $TOTAL_STEPS "代理服务启动失败"
        echo ""
        echo -e "${RED}代理服务启动失败或超时${NC}"
        echo "查看日志: tail -f $LOG_DIR/server.log"
        echo ""

        if [ -f "$LOG_DIR/server.log" ]; then
            echo -e "${YELLOW}最近的日志:${NC}"
            tail -10 "$LOG_DIR/server.log"
        fi

        echo ""
        echo -e "${YELLOW}建议尝试:${NC}"
        echo "  1. 检查端口 $PROXY_PORT 是否被占用: lsof -i :$PROXY_PORT"
        echo "  2. 手动启动: $CLI_LAUNCHER server start --host $PROXY_BIND_HOST --port $PROXY_PORT"
        echo "  3. 查看详细日志: tail -f $LOG_DIR/server.log"
        exit 1
    fi
else
    # GUI 模式：启动 Tauri 代理 (自动启动 GUI 界面)
    nohup "$INSTALL_DIR/cc-switch" > "$LOG_DIR/proxy.log" 2>&1 &
    PROXY_PID=$!

    sleep 3

    if ps -p $PROXY_PID > /dev/null 2>&1; then
        echo $PROXY_PID > "$CC_SWITCH_DIR/proxy.pid"
        step_done $CURRENT_STEP $TOTAL_STEPS "启动代理服务 (GUI 模式, PID: $PROXY_PID)"
    else
        step_error $CURRENT_STEP $TOTAL_STEPS "代理服务启动失败"
        echo ""
        echo -e "${RED}GUI模式启动失败${NC}"
        echo "查看日志: tail -f $LOG_DIR/proxy.log"
        exit 1
    fi
fi

# ============================================================================
# 步骤 11: 配置权重轮询（默认启用）
# ============================================================================
CURRENT_STEP=11
step_running $CURRENT_STEP $TOTAL_STEPS "配置权重轮询"

# 等待服务完全启动
sleep 2

# 默认为所有应用启用权重轮询
# 使用 CLI 命令启用权重轮询（配置存储在数据库中）
WEIGHT_RR_ENABLED=0
for app in claude codex gemini; do
    if "$INSTALL_DIR/ccs" config lb --app "$app" --enabled true > /dev/null 2>&1; then
        WEIGHT_RR_ENABLED=$((WEIGHT_RR_ENABLED + 1))
    fi
done

if [ "$WEIGHT_RR_ENABLED" -gt 0 ]; then
    step_done $CURRENT_STEP $TOTAL_STEPS "权重轮询 (已启用 $WEIGHT_RR_ENABLED 个应用)"
else
    step_done $CURRENT_STEP $TOTAL_STEPS "权重轮询 (配置中)"
fi

# 提示：如需禁用权重轮询，可使用以下命令：
# ccs config lb --app claude --enabled false
# ccs config lb --app codex --enabled false
# ccs config lb --app gemini --enabled false

# ============================================================================
# 步骤 12: 验证部署状态
# ============================================================================
CURRENT_STEP=12
step_running $CURRENT_STEP $TOTAL_STEPS "验证部署状态"

sleep 2

PROXY_OK=false
DB_OK=false

# 检查 CLI 模式（server.pid）或 GUI 模式（proxy.pid）
if [ -f "$CC_SWITCH_DIR/server.pid" ]; then
    PID=$(cat "$CC_SWITCH_DIR/server.pid")
    if ps -p "$PID" > /dev/null 2>&1; then
        PROXY_OK=true
    fi
elif [ -f "$CC_SWITCH_DIR/proxy.pid" ]; then
    PID=$(cat "$CC_SWITCH_DIR/proxy.pid")
    if ps -p "$PID" > /dev/null 2>&1; then
        PROXY_OK=true
    fi
fi

if [ -f "$DB_PATH" ]; then
    DB_OK=true
fi

if [ "$PROXY_OK" = true ]; then
    step_done $CURRENT_STEP $TOTAL_STEPS "部署成功"
    echo ""
else
    step_error $CURRENT_STEP $TOTAL_STEPS "部署失败"
fi

echo ""
echo ""
echo -e "${GREEN}服务状态${NC}"
echo ""

# 代理服务状态
echo -e "${BLUE}   代理服务 (端口 ${PROXY_PORT})${NC}"
if [ "$PROXY_OK" = true ]; then
    echo -e "   状态: ${GREEN}✓ 运行中${NC}"
    echo -e "   监听: ${PROXY_BIND_HOST}:${PROXY_PORT}"
    echo -e "   本机访问: http://${PROXY_CLIENT_HOST}:${PROXY_PORT}"
    case "$PROXY_BIND_HOST" in
        0.0.0.0)
            _lan_ip="$(hostname -I 2>/dev/null | awk '{print $1}')"
            [ -n "$_lan_ip" ] && echo -e "   局域网访问: http://${_lan_ip}:${PROXY_PORT}"
            ;;
        127.*|localhost)
            ;;
        *)
            echo -e "   局域网访问: http://${PROXY_BIND_HOST}:${PROXY_PORT}"
            ;;
    esac
    echo -e "   查看日志:  tail -n 300 -F ~/.cc-switch/logs/server.log"
else
    echo -e "   状态: ${RED}✗ 未运行${NC}"
fi
echo ""

# Web 控制台状态
if [ "${WEB_PANEL_ENABLED:-0}" -eq 1 ] && [ -n "${WEB_PORT:-}" ]; then
    echo -e "${BLUE}   Web 控制台 (端口 ${WEB_PORT})${NC}"
    echo -e "   本机访问: http://127.0.0.1:${WEB_PORT}"
    if [ "${WEB_BIND:-}" = "0.0.0.0" ]; then
        _lan_ip="$(hostname -I 2>/dev/null | awk '{print $1}')"
        [ -n "$_lan_ip" ] && echo -e "   局域网访问: http://${_lan_ip}:${WEB_PORT}"
    fi
    echo -e "   首次访问请在浏览器中设置访问密码"
    echo ""
fi

# 数据库状态
echo -e "${BLUE}   数据库${NC}"
if [ "$DB_OK" = true ]; then
    DB_VERSION=$(sqlite3 "$DB_PATH" "PRAGMA user_version" 2>/dev/null || echo "unknown")
    echo -e "   状态: ${GREEN}✓ 已初始化${NC}"
    echo -e "   位置: $DB_PATH"
else
    echo -e "   状态: ${YELLOW}⚠ 未初始化${NC}"
fi
echo ""

# 复制文档到配置目录
DOCS_DIR="$CC_SWITCH_DIR/docs"
mkdir -p "$DOCS_DIR"
if [ -f "$SCRIPT_DIR/docs/CLI-快速参考.md" ]; then
    cp "$SCRIPT_DIR/docs/CLI-快速参考.md" "$DOCS_DIR/"
fi
if [ -f "$SCRIPT_DIR/docs/CLI-使用教程.md" ]; then
    cp "$SCRIPT_DIR/docs/CLI-使用教程.md" "$DOCS_DIR/"
fi

# 常用命令提示
echo -e "${BLUE}   查看使用方法${NC}"
echo -e "   运行: ${YELLOW}head -60 ~/.cc-switch/docs/CLI-快速参考.md${NC}"
echo ""

echo -e "${BLUE}   超参数示例${NC}"
echo -e "   查看: ${YELLOW}ccs provider hp show --app opencode --id <PROVIDER_ID> --path agents.sisyphus${NC}"
echo -e "   设置: ${YELLOW}ccs provider hp set --app opencode --id <PROVIDER_ID> --path agents.sisyphus.temperature --json '0.5'${NC}"
echo ""

echo -e "${GREEN}部署完成！感谢使用 CC-Switch！${NC}"
