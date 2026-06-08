# CC-Switch CLI

**Claude/Codex/Gemini 统一本地代理**,支持供应商权重轮询、自动故障转移、无头/图形双模式。本项目 fork 自 [farion1231/cc-switch](https://github.com/farion1231/cc-switch),强化 CLI 控制与后台服务能力。

## 主要功能

<<<<<<< HEAD
- **统一代理入口** — 单一本地端点(默认 `127.0.0.1:15721`)供 Claude Desktop、Codex CLI、Gemini CLI、OpenCode 等客户端接入
- **供应商权重轮询** — 按权重分配请求至多个供应商(如 `weight=1` 每轮必用,`weight=3` 每 3 轮用 1 次),均衡负载与规避限流
- **自动故障转移** — 供应商故障时透明切换备用,含熔断器、重试与优雅降级
- **无头后台服务** — CLI 模式适配 SSH 远程/服务器/容器,可选启用 Web 控制台(浏览器管理供应商与用量统计)
- **图形界面(GUI)** — Tauri 桌面应用,可视化管理供应商、权重、故障转移队列与实时监控
- **超参数精细调控** — 直接读写供应商 `settings_config` 任意 JSON 路径(Agent 参数/Category 配置/自定义 headers)
- **导入导出** — 供应商配置跨设备同步/备份,支持密钥脱敏
- **用量统计** — 成本/token/请求明细,按供应商/模型/时间范围聚合
=======
命令入口以 `ccs` 为主。
>>>>>>> origin/cc-switch-cli

## 快速安装

### Linux (无头 CLI)

**一键安装**(预构建二进制,无需编译,约 25MB):

```bash
curl -fsSL https://github.com/Aaroen/cc-switch-cli/releases/latest/download/cc-switch-cli-linux-x86_64.tar.gz | tar -xz && cd cc-switch-cli-linux-x86_64 && ./install-ccs.sh
```

安装脚本会自动:
- 检测图形/无头环境,选择对应模式
- 询问是否启用 Web 控制台(浏览器管理)
- 安装到 `~/.local/bin` 并配置环境变量
- 无头模式自动启动后台代理服务

<<<<<<< HEAD
安装完成后重载环境变量:
=======
- 该命令依赖 Releases 中存在 `cc-switch-cli-linux-x86_64.tar.gz` 或 `cc-switch-cli-linux-arm64.tar.gz` 资产，压缩包内包含 `install-ccs.sh`、`cc-switch`、`cc-switch-cli` 与 `ccs/csc` 兼容入口。
- Linux CLI 发布包中，`ccs`、`csc` 与 `cc-switch` 均指向专用 `cc-switch-cli` 二进制；GUI 主程序不再承担 CLI 分流。
- 如 `latest` 暂不可用，可指定版本：`TAG=v3.13.0`（示例）后再运行上面的一行命令。
- 默认会将下载的 Release 资产缓存到 `~/.cc-switch/.cache/prebuilt/<TAG>/`；如需强制重新下载：`FORCE=1`。
- 为避免历史脚本导致 `~/.bashrc` 重复写入等问题，本命令会额外拉取仓库分支 `cc-switch-cli` 的最新版 `install-ccs.sh` 覆盖执行（不需要重新编译）。
- 默认以 CLI 模式部署（无头 server），并自动处理端口占用（必要时自动换端口）。
- 如需 GUI 模式（会启动 Tauri 界面）：使用 Git 版本并执行 `bash "$HOME/cc-switch-cli/install-ccs.sh" --gui`。
- macOS/Windows 用户可直接从 Releases 下载对应 GUI 安装包或 CLI 终端包，无需本机编译。

清理缓存（下次会重新下载）：

```bash
rm -rf ~/.cc-switch/.cache/prebuilt
```

## 权重轮询（Weight Round Robin）

本仓库在 CLI 场景下默认启用“按供应商权重分配请求”的能力（部署脚本会为 `claude/codex/gemini` 默认打开权重轮询），同时 GUI 中也提供同风格的权重面板用于可视化调整。

规则（与 CLI 输出保持一致）：

- `weight=0`：禁用该供应商
- `weight=1`：每轮都使用（最高频）
- `weight=N`：每 N 轮使用一次

常用命令：

```bash
# 查看某个 app 的负载均衡/权重轮询状态与供应商权重表
ccs config lb --app claude

# 启用/禁用权重轮询
ccs config lb --app claude --enabled true
ccs config lb --app claude --enabled false

# 列出供应商并获取 ID
ccs provider list --app claude

# 设置供应商权重（0-100）
ccs provider weight --app claude --id <PROVIDER_ID> --weight 1
```

## 超参数（Hyperparams）

`ccs provider hyperparams`（别名：`ccs provider hp`）用于直接读写供应商 `settings_config` 中的任意 JSON 路径，适合 OMO/OpenCode 的 Agent/Category 高级参数，也适用于其他 app 的细粒度配置修正。

典型路径示例：

- `agents.sisyphus.temperature`
- `agents.sisyphus.permission`
- `categories.quick.prompt_append`
- `options.headers.Authorization`

常用命令：

```bash
# 查看整个 settings_config
ccs provider hp show --app opencode --id <PROVIDER_ID>

# 查看单一路径
ccs provider hp show --app opencode --id <PROVIDER_ID> --path agents.sisyphus

# 设置数值 / 布尔 / 对象等 JSON 值
ccs provider hp set --app opencode --id <PROVIDER_ID> --path agents.sisyphus.temperature --json '0.5'
ccs provider hp set --app opencode --id <PROVIDER_ID> --path agents.sisyphus.permission --json '{"edit":"allow","bash":"ask"}'

# 设置纯字符串
ccs provider hp set --app opencode --id <PROVIDER_ID> --path categories.quick.prompt_append --value "Always answer in Chinese"

# 删除路径
ccs provider hp remove --app opencode --id <PROVIDER_ID> --path categories.quick.prompt_append
```

说明：

- 路径分隔符使用 `.`。
- 数组索引可直接写数字段，例如 `tools.0.name`。
- 该命令只修改供应商的 `settings_config`，不会影响您保留的权重轮询、故障转移和无头运行机制。

GUI 配置入口：

- `设置 -> 代理 -> 自动故障转移 -> 权重轮询`
- 每个应用（`claude/codex/gemini`）均可独立开关，并按供应商逐项设置权重

## 供应商导入导出

现有 CLI 已提供正式可用的导入导出接口，可用于备份、迁移与跨设备同步。

```bash
# 导出某个 app 的全部供应商
ccs provider export --app codex --output codex-providers.json

# 仅导出单个供应商，并对密钥做脱敏
ccs provider export --app claude --id <PROVIDER_ID> --output claude-provider.json --redact

# 导入供应商；如 ID 冲突则覆盖
ccs provider import --app codex --input codex-providers.json --overwrite

# 导入时重新生成 ID，并将导出文件中的 current 一并设置回来
ccs provider import --app claude --input claude-provider.json --new-ids --set-current
```

## 代理与故障转移

CLI 无头模式的核心链路是 `server`、`config lb` 与 `failover` 三组命令。

```bash
# 启动/停止/查看无头代理服务
ccs server start --host 127.0.0.1 --port 15721
ccs server status
ccs server stop

# 查看故障转移队列
ccs failover queue --app claude

# 添加/移除备用供应商
ccs failover add --app claude --id <PROVIDER_ID>
ccs failover remove --app claude --id <PROVIDER_ID>

# 启用/禁用自动故障转移
ccs failover toggle --app claude --enabled true
ccs failover toggle --app claude --enabled false
```

对应 GUI 入口：

- `设置 -> 代理服务`
- `设置 -> 自动故障转移`
- 供应商卡片上的故障转移开关与状态标记

## 配置目录

默认数据目录为 `~/.cc-switch/`，其中常用文件如下：

```text
~/.cc-switch/
├── cc-switch.db
├── settings.json
├── logs/
└── skills/
```

不同 CLI 的 live 配置目录仍保持各自原生路径：

- Claude: `~/.claude/`
- Codex: `~/.codex/`
- Gemini: `~/.gemini/`
- OpenCode: `~/.opencode/`
- OpenClaw: `~/.openclaw/`

## 启动与验证

部署完成后通常需要重新加载 shell 环境变量：
>>>>>>> origin/cc-switch-cli

```bash
source ~/.bashrc  # 或 source ~/.zshrc
```

**启动服务**:

```bash
ccs server start            # 默认 127.0.0.1:15721
ccs server status           # 查看运行状态
```

**启用 Web 控制台**(可选,浏览器管理):

```bash
ccs server stop
ccs server start --web-port 8888 --web-bind 0.0.0.0  # 局域网可访问 http://<IP>:8888
```

### Windows / macOS (图形界面)

从 [Releases](https://github.com/Aaroen/cc-switch-cli/releases) 下载对应安装包:

- **Windows**: `CC-Switch-*-Windows.msi` (含 WebView2 离线运行时,中国网络友好)
- **macOS**: `CC-Switch-*-macOS-universal.dmg` (Intel + Apple Silicon 通用)

安装后启动图形界面,在 `设置 -> 供应商` 中添加你的 API Key 即可。

## 核心命令速查

### 供应商管理

```bash
ccs provider list --app claude                          # 列出供应商
ccs provider add --app claude --name "Claude-Main" \
  --api-key sk-ant-xxx --base-url https://api.anthropic.com
ccs provider delete --app claude --id <PROVIDER_ID>
ccs provider export --app claude -o backup.json        # 导出备份
ccs provider import --app claude -i backup.json        # 导入
```

### 权重轮询

```bash
ccs config lb --app claude                              # 查看权重轮询状态
ccs config lb --app claude --enabled true               # 启用
ccs provider weight --app claude --id <ID> --weight 1   # 设权重(1=每轮必用, 3=每3轮用1次, 0=禁用)
```

### 故障转移

```bash
ccs failover queue --app claude                         # 查看备用队列
ccs failover add --app claude --id <BACKUP_ID>          # 添加备用
ccs failover toggle --app claude --enabled true         # 启用自动切换
```

### 超参数(高级配置)

```bash
# 查看供应商完整配置树
ccs provider hp show --app opencode --id <ID>

# 设置 JSON 值(Agent 参数/Category 配置等)
ccs provider hp set --app opencode --id <ID> \
  --path agents.sisyphus.temperature --json '0.7'

# 设置字符串
ccs provider hp set --app opencode --id <ID> \
  --path categories.quick.prompt_append --value "Always in Chinese"

# 删除路径
ccs provider hp remove --app opencode --id <ID> --path <路径>
```

### 服务控制

```bash
ccs server start --host 0.0.0.0 --port 15721            # 启动(允许局域网)
ccs server start --web-port 8888 --web-bind 0.0.0.0    # 启动 + Web 控制台
ccs server stop
ccs server status
ccs server restart
```

## 配置与日志

- **数据库**: `~/.cc-switch/cc-switch.db` (SQLite,含供应商/权重/用量统计)
- **日志**: `~/.cc-switch/logs/server.log` (代理请求与故障转移日志,自动轮转,默认 50MB/保留 5 份)
- **崩溃日志**: `~/.cc-switch/crash.log` (Windows 闪退诊断用)
- **CLI 原生配置**: `~/.claude/` / `~/.codex/` / `~/.gemini/` 等(与官方 CLI 兼容)

## 客户端配置

将你的 Claude Desktop / Codex CLI / OpenCode 等客户端的代理端点指向 `http://127.0.0.1:15721`:

- **Claude Desktop**: `~/.claude/config.json` 中设 `"customBaseUrl": "http://127.0.0.1:15721"`
- **Codex CLI**: 环境变量 `CODEX_PROXY=http://127.0.0.1:15721`
- **OpenCode**: `~/.opencode/settings.toml` 中设 `anthropic_base_url = "http://127.0.0.1:15721"`

详见各客户端官方文档。

## 从源码构建

需 Rust 1.83+ / Node.js 20+ / pnpm 10+。

```bash
git clone https://github.com/Aaroen/cc-switch-cli.git
cd cc-switch-cli
./install-ccs.sh          # CLI 模式(自动安装依赖并编译)
./install-ccs.sh --gui    # GUI 模式(需 GTK3/WebKit2GTK)
```

## 上游与许可证

本项目基于 [farion1231/cc-switch](https://github.com/farion1231/cc-switch) fork,持续贴近上游并强化 CLI/无头运行能力。上游原始 README 见 [README_UPSTREAM.md](./README_UPSTREAM.md)。

许可证: MIT

## 问题反馈

遇到问题请提 [Issue](https://github.com/Aaroen/cc-switch-cli/issues),附上:
- 操作系统与版本
- `ccs server status` 输出
- `~/.cc-switch/logs/server.log` 最后 50 行
- Windows 闪退请附 `%USERPROFILE%\.cc-switch\crash.log`
