# CC-Switch CLI

**Claude/Codex/Gemini 统一本地代理**,支持供应商权重轮询、自动故障转移、无头/图形双模式。本项目 fork 自 [farion1231/cc-switch](https://github.com/farion1231/cc-switch),强化 CLI 控制与后台服务能力。

## 主要功能

- **统一代理入口** — 单一本地端点(默认 `127.0.0.1:15721`)供 Claude Desktop、Codex CLI、Gemini CLI、OpenCode 等客户端接入
- **供应商权重轮询** — 按权重分配请求至多个供应商(如 `weight=1` 每轮必用,`weight=3` 每 3 轮用 1 次),均衡负载与规避限流
- **自动故障转移** — 供应商故障时透明切换备用,含熔断器、重试与优雅降级
- **无头后台服务** — CLI 模式适配 SSH 远程/服务器/容器,可选启用 Web 控制台(浏览器管理供应商与用量统计)
- **图形界面(GUI)** — Tauri 桌面应用,可视化管理供应商、权重、故障转移队列与实时监控
- **超参数精细调控** — 直接读写供应商 `settings_config` 任意 JSON 路径(Agent 参数/Category 配置/自定义 headers)
- **导入导出** — 供应商配置跨设备同步/备份,支持密钥脱敏
- **用量统计** — 成本/token/请求明细,按供应商/模型/时间范围聚合

## 快速安装

### Linux

**一键部署**（自动下载最新 release 并部署）：

```bash
curl -fsSL https://raw.githubusercontent.com/Aaroen/cc-switch-cli/main/deploy.sh | bash
```

部署脚本会自动：
- 从 GitHub Release 下载最新的二进制文件（仅 Linux x86_64 未找到本地二进制时）
- 检测 GUI/无头环境，选择对应模式
- 安装到 `~/.local/bin` 并配置 PATH
- 配置 Claude Code CLI 和 Codex CLI（最小侵入）
- 无头模式自动启动后台服务 + Web 控制台（局域网可访问）
- 默认启用权重轮询

部署完成后重载环境变量：

```bash
source ~/.bashrc  # 或 source ~/.zshrc
```

**常用命令**：

```bash
ccs status           # 查看运行状态
ccs stop             # 停止服务
ccs restart          # 重启服务
```


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
ccs start --host 0.0.0.0 --port 15721            # 启动(允许局域网)
ccs start --web-port 8888 --web-bind 0.0.0.0    # 启动 + Web 控制台
ccs stop
ccs status
ccs restart
```

## 配置与日志

- **数据库**: `~/.cc-switch/cc-switch.db` 
- **日志**: `~/.cc-switch/logs/server.log`
- **崩溃日志**: `~/.cc-switch/crash.log` 
- **CLI 原生配置**: `~/.claude/` / `~/.codex/` / `~/.gemini/` 等

## 客户端配置

将你的 Claude Desktop / Codex CLI / OpenCode 等客户端的代理端点指向 `http://127.0.0.1:15721`:

- **Claude Desktop**: `~/.claude/config.json` 中设 `"customBaseUrl": "http://127.0.0.1:15721"`
- **Codex CLI**: 环境变量 `CODEX_PROXY=http://127.0.0.1:15721`
- **OpenCode**: `~/.opencode/settings.toml` 中设 `anthropic_base_url = "http://127.0.0.1:15721"`

详见各客户端官方文档。

## 上游与许可证

本项目基于 [farion1231/cc-switch](https://github.com/farion1231/cc-switch) fork,持续贴近上游并强化 CLI/无头运行能力。上游原始 README 见 [README_UPSTREAM.md](./README_UPSTREAM.md)。

许可证: MIT

## 问题反馈

遇到问题请提 [Issue](https://github.com/Aaroen/cc-switch-cli/issues),附上:
