#!/bin/bash
#
# CC-Switch v3.9.1+ 一键部署脚本
# 支持官方更新拉取、分层转发器、负载均衡等特性
#

set -e

# 颜色定义
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color

# 进度条函数
show_progress() {
    local current=$1
    local total=$2
    local step_name=$3
    local status=$4  # "running" 或 "done" 或 "error"

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

# 清屏并显示标题
clear
echo -e "${CYAN}CC-Switch 一键部署脚本${NC}"
echo ""

# 检查部署模式（默认CLI）
CLI_MODE="${CLI_MODE:-true}"
if [ "$1" = "--gui" ]; then
    CLI_MODE="false"
    echo -e "${YELLOW}GUI 模式已启用${NC}"
    echo ""
elif [ "$CLI_MODE" = "true" ]; then
    echo -e "${GREEN}CLI 模式${NC}"
    echo ""
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

    # 询问是否拉取官方更新
    if [ "$1" = "--update" ] || [ "$1" = "-u" ]; then
        echo ""
        echo -e "${YELLOW}正在拉取官方更新...${NC}"
        cd "$SCRIPT_DIR"

        # 保存本地修改
        if ! git diff --quiet || ! git diff --cached --quiet; then
            echo -e "${YELLOW}检测到本地修改，正在暂存...${NC}"
            git stash push -m "Auto-stash before update $(date +%Y%m%d_%H%M%S)"
        fi

        # 拉取并rebase
        if git pull --rebase origin main 2>&1 | tee "$LOG_DIR/git_update.log"; then
            echo -e "${GREEN}✓ 官方更新已拉取${NC}"

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

if ! command -v cargo &> /dev/null; then
    echo ""
    echo -e "${YELLOW}Rust 未安装，正在自动安装...${NC}"

    if curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable > /dev/null 2>&1; then
        if [ -f "$HOME/.cargo/env" ]; then
            source "$HOME/.cargo/env"
        fi

        if command -v cargo &> /dev/null; then
            RUST_VERSION=$(rustc --version)
            step_done $CURRENT_STEP $TOTAL_STEPS "安装 Rust 工具链 ($RUST_VERSION)"
        else
            step_error $CURRENT_STEP $TOTAL_STEPS "Rust 安装失败"
            exit 1
        fi
    else
        step_error $CURRENT_STEP $TOTAL_STEPS "Rust 安装失败"
        exit 1
    fi
else
    RUST_VERSION=$(rustc --version)
    step_done $CURRENT_STEP $TOTAL_STEPS
fi

# ============================================================================
# 步骤 3: 检查 Node.js 环境 (前端构建)
# ============================================================================
CURRENT_STEP=3
step_running $CURRENT_STEP $TOTAL_STEPS "检查 Node.js 环境"

NODE_CMD=""
if command -v node &> /dev/null; then
    NODE_VERSION=$(node --version)
    NODE_CMD="node"
    step_done $CURRENT_STEP $TOTAL_STEPS "Node.js 环境 ($NODE_VERSION)"
else
    step_error $CURRENT_STEP $TOTAL_STEPS "Node.js 未安装"
    echo ""
    echo -e "${RED}错误: 需要 Node.js 16+ 来构建前端${NC}"
    echo "请安装: https://nodejs.org/"
    exit 1
fi

# 检查 pnpm
if ! command -v pnpm &> /dev/null; then
    echo -e "${YELLOW}pnpm 未安装，正在安装...${NC}"
    npm install -g pnpm > /dev/null 2>&1
fi

# ============================================================================
# 步骤 4: 安装前端依赖
# ============================================================================
CURRENT_STEP=4
step_running $CURRENT_STEP $TOTAL_STEPS "安装前端依赖"

cd "$SCRIPT_DIR"
if [ ! -d "node_modules" ] || [ "package.json" -nt "node_modules" ]; then
    if pnpm install > "$LOG_DIR/pnpm_install.log" 2>&1; then
        step_done $CURRENT_STEP $TOTAL_STEPS "安装前端依赖"
    else
        step_error $CURRENT_STEP $TOTAL_STEPS "前端依赖安装失败"
        echo "查看日志: cat $LOG_DIR/pnpm_install.log"
        exit 1
    fi
else
    step_done $CURRENT_STEP $TOTAL_STEPS "前端依赖 (使用缓存)"
fi

# ============================================================================
# 步骤 5: 编译 Tauri 应用
# ============================================================================
CURRENT_STEP=5
step_running $CURRENT_STEP $TOTAL_STEPS "编译 Tauri 应用"

cd "$SCRIPT_DIR"

# 检查是否需要重新编译
#
# 注意：用目录 mtime 做比较在某些情况下不可靠（目录时间戳不一定反映上次编译时间），
# 这里改为以最终二进制文件作为基准。
BIN_PATH="src-tauri/target/release/cc-switch"
NEED_REBUILD=false

if [ ! -f "$BIN_PATH" ]; then
    NEED_REBUILD=true
else
    # 任何 Rust 源码/配置文件比二进制新，都触发重编译
    if [ -n "$(find src-tauri/src -name '*.rs' -newer "$BIN_PATH" 2>/dev/null)" ]; then
        NEED_REBUILD=true
    elif [ -f "src-tauri/Cargo.toml" ] && [ "src-tauri/Cargo.toml" -nt "$BIN_PATH" ]; then
        NEED_REBUILD=true
    elif [ -f "src-tauri/Cargo.lock" ] && [ "src-tauri/Cargo.lock" -nt "$BIN_PATH" ]; then
        NEED_REBUILD=true
    fi
fi

if [ "$NEED_REBUILD" = true ]; then
    # 运行Tauri构建，跳过 AppImage 打包（避免网络下载问题）
    # 只生成 deb 和 rpm 包
    pnpm tauri build --bundles deb,rpm > "$LOG_DIR/tauri_build.log" 2>&1 || true

    # 检查二进制文件是否成功生成
    if [ -f "src-tauri/target/release/cc-switch" ]; then
        step_done $CURRENT_STEP $TOTAL_STEPS "编译 Tauri 应用 (新编译)"
    else
        step_error $CURRENT_STEP $TOTAL_STEPS "编译失败"
        echo "查看日志: cat $LOG_DIR/tauri_build.log"
        exit 1
    fi
else
    step_done $CURRENT_STEP $TOTAL_STEPS "Tauri 应用 (使用缓存)"
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
for pid in $(pgrep -f "cc-switch" 2>/dev/null); do
    # 跳过当前脚本进程
    if [ "$pid" = "$CURRENT_SCRIPT_PID" ]; then
        continue
    fi
    # 检查进程命令行，只杀死真正的 cc-switch 服务进程
    CMDLINE=$(cat /proc/$pid/cmdline 2>/dev/null | tr '\0' ' ')
    if echo "$CMDLINE" | grep -qE "(^|/)cc-switch( |$)|cc-switch server"; then
        kill "$pid" 2>/dev/null
    fi
done

# 等待进程完全退出
sleep 2

# 确认进程已停止，如果还在运行则强制终止
for pid in $(pgrep -f "cc-switch" 2>/dev/null); do
    if [ "$pid" = "$CURRENT_SCRIPT_PID" ]; then
        continue
    fi
    CMDLINE=$(cat /proc/$pid/cmdline 2>/dev/null | tr '\0' ' ')
    if echo "$CMDLINE" | grep -qE "(^|/)cc-switch( |$)|cc-switch server"; then
        echo ""
        echo -e "${YELLOW}强制终止残留进程 (PID: $pid)...${NC}"
        kill -9 "$pid" 2>/dev/null
    fi
done

sleep 1

# 重新启用 set -e
set -e

step_done $CURRENT_STEP $TOTAL_STEPS "停止旧服务"

# ============================================================================
# 步骤 8: 安装二进制到系统路径
# ============================================================================
CURRENT_STEP=8
step_running $CURRENT_STEP $TOTAL_STEPS "安装到系统路径"

mkdir -p "$INSTALL_DIR"

# 查找编译后的二进制文件
if [ -f "src-tauri/target/release/cc-switch" ]; then
    # 删除旧的二进制文件（直接覆盖，不备份）
    rm -f "$INSTALL_DIR/cc-switch" 2>/dev/null || true
    rm -f "$INSTALL_DIR/csc" 2>/dev/null || true

    # 复制新的二进制文件
    cp "src-tauri/target/release/cc-switch" "$INSTALL_DIR/"
    chmod +x "$INSTALL_DIR/cc-switch"

    # 创建 csc 软链接（简写命令）
    ln -sf "$INSTALL_DIR/cc-switch" "$INSTALL_DIR/csc"
fi

# 添加到 PATH
SHELL_CONFIG=""
if [ -n "$BASH_VERSION" ] && [ -f "$HOME/.bashrc" ]; then
    SHELL_CONFIG="$HOME/.bashrc"
elif [ -n "$ZSH_VERSION" ] && [ -f "$HOME/.zshrc" ]; then
    SHELL_CONFIG="$HOME/.zshrc"
elif [ -f "$HOME/.bash_profile" ]; then
    SHELL_CONFIG="$HOME/.bash_profile"
fi

if [ -n "$SHELL_CONFIG" ]; then
    if ! grep -q "$INSTALL_DIR" "$SHELL_CONFIG" 2>/dev/null; then
        echo "" >> "$SHELL_CONFIG"
        echo "# CC-Switch PATH" >> "$SHELL_CONFIG"
        echo 'export PATH="$HOME/.local/bin:$PATH"' >> "$SHELL_CONFIG"
    fi
fi

step_done $CURRENT_STEP $TOTAL_STEPS

# ============================================================================
# 步骤 9: 配置环境变量
# ============================================================================
CURRENT_STEP=9
step_running $CURRENT_STEP $TOTAL_STEPS "配置环境变量"

DEFAULT_PORT=15721
PROXY_HOST="127.0.0.1"
PROXY_PORT="${CC_SWITCH_PORT:-$DEFAULT_PORT}"

# 检查端口占用
if command -v lsof &> /dev/null; then
    if lsof -Pi :$PROXY_PORT -sTCP:LISTEN -t >/dev/null; then
        PROXY_PORT=$((PROXY_PORT + 1))
    fi
fi

PROXY_BASE="http://${PROXY_HOST}:${PROXY_PORT}"

# 配置 Claude CLI
if [ -n "$SHELL_CONFIG" ]; then
    if ! grep -q "ANTHROPIC_BASE_URL.*$PROXY_BASE" "$SHELL_CONFIG" 2>/dev/null; then
        echo "" >> "$SHELL_CONFIG"
        echo "# CC-Switch Claude CLI 配置" >> "$SHELL_CONFIG"
        echo "export ANTHROPIC_BASE_URL=\"${PROXY_BASE}\"" >> "$SHELL_CONFIG"
        echo 'export ANTHROPIC_API_KEY="sk-placeholder-managed-by-cc-switch"' >> "$SHELL_CONFIG"
    fi
fi

# 配置 Codex CLI
CODEX_CONFIG_DIR="$HOME/.codex"
CODEX_CONFIG_FILE="$CODEX_CONFIG_DIR/config.toml"
mkdir -p "$CODEX_CONFIG_DIR"

cat > "$CODEX_CONFIG_FILE" <<EOF
model = "gpt-4o"
model_provider = "cc-switch"

[model_providers.cc-switch]
name = "CC-Switch Proxy"
base_url = "$PROXY_BASE/v1"
wire_api = "responses"
EOF

# 配置 Gemini CLI
GEMINI_CONFIG_DIR="$HOME/.gemini"
GEMINI_ENV_FILE="$GEMINI_CONFIG_DIR/.env"
mkdir -p "$GEMINI_CONFIG_DIR"

cat > "$GEMINI_ENV_FILE" <<EOF
GEMINI_API_BASE_URL=$PROXY_BASE
GEMINI_API_KEY=sk-placeholder-managed-by-cc-switch
EOF

step_done $CURRENT_STEP $TOTAL_STEPS "配置环境变量"

# ============================================================================
# 步骤 10: 启动代理服务
# ============================================================================
CURRENT_STEP=10
step_running $CURRENT_STEP $TOTAL_STEPS "启动代理服务"

cd "$SCRIPT_DIR"

# 检查是否使用 CLI 模式
if [ "$CLI_MODE" = "true" ]; then
    # CLI 模式：自动启动无头服务器
    nohup "$INSTALL_DIR/cc-switch" server start --host 127.0.0.1 --port $PROXY_PORT > "$LOG_DIR/server.log" 2>&1 &

    sleep 3

    # 验证服务器是否启动
    if [ -f "$CC_SWITCH_DIR/server.pid" ]; then
        SERVER_PID=$(cat "$CC_SWITCH_DIR/server.pid")
        if ps -p $SERVER_PID > /dev/null 2>&1; then
            step_done $CURRENT_STEP $TOTAL_STEPS "启动代理服务 (CLI 模式, PID: $SERVER_PID)"
        else
            step_error $CURRENT_STEP $TOTAL_STEPS "代理服务启动失败"
            echo "查看日志: tail -f $LOG_DIR/server.log"
            exit 1
        fi
    else
        step_error $CURRENT_STEP $TOTAL_STEPS "代理服务启动失败 (未生成PID文件)"
        echo "查看日志: tail -f $LOG_DIR/server.log"
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
    if "$INSTALL_DIR/csc" config lb --app "$app" --enabled true > /dev/null 2>&1; then
        WEIGHT_RR_ENABLED=$((WEIGHT_RR_ENABLED + 1))
    fi
done

if [ "$WEIGHT_RR_ENABLED" -gt 0 ]; then
    step_done $CURRENT_STEP $TOTAL_STEPS "权重轮询 (已启用 $WEIGHT_RR_ENABLED 个应用)"
else
    step_done $CURRENT_STEP $TOTAL_STEPS "权重轮询 (配置中)"
fi

# 提示：如需禁用权重轮询，可使用以下命令：
# csc config lb --app claude --enabled false
# csc config lb --app codex --enabled false
# csc config lb --app gemini --enabled false

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
    echo -e "   地址: ${PROXY_HOST}:${PROXY_PORT}"
    echo -e "   查看日志:  tail -n 300 -F ~/.cc-switch/logs/server.log"
else
    echo -e "   状态: ${RED}✗ 未运行${NC}"
fi
echo ""

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

echo -e "${GREEN}部署完成！感谢使用 CC-Switch！${NC}"
