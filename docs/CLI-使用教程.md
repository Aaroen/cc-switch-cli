# CC-Switch CLI 使用教程

> CC-Switch 是一个 Claude Code / Codex / Gemini CLI 统一管理工具，提供完整的命令行接口来管理多个 AI 服务供应商。

## 目录

- [安装与部署](#安装与部署)
- [快速开始](#快速开始)
- [命令概览](#命令概览)
- [服务器管理 (server)](#服务器管理-server)
- [供应商管理 (provider)](#供应商管理-provider)
- [配置管理 (config)](#配置管理-config)
- [故障转移 (failover)](#故障转移-failover)
- [统计信息 (stats)](#统计信息-stats)
- [MCP服务器管理 (mcp)](#mcp服务器管理-mcp)
- [提示词管理 (prompt)](#提示词管理-prompt)
- [技能管理 (skill)](#技能管理-skill)
- [常见问题](#常见问题)

---

## 安装与部署

### 一键部署

使用官方部署脚本进行安装：

```bash
# 克隆仓库后执行
./install-ccs.sh
```

### 部署模式

CC-Switch 支持两种部署模式：

| 模式 | 命令 | 说明 |
|------|------|------|
| CLI 模式（默认） | `./install-ccs.sh` | 无头服务器模式，适合服务器部署 |
| GUI 模式 | `./install-ccs.sh --gui` | 启动图形界面 |

### 更新

```bash
# 拉取官方更新
./install-ccs.sh --update
# 或
./install-ccs.sh -u
```

### 安装目录

| 路径 | 说明 |
|------|------|
| `~/.local/bin/cc-switch` | 主程序 |
| `~/.local/bin/csc` | 简写命令（软链接） |
| `~/.cc-switch/` | 配置目录 |
| `~/.cc-switch/cc-switch.db` | SQLite 数据库 |
| `~/.cc-switch/logs/` | 日志目录 |
| `~/.cc-switch/server.pid` | 服务器 PID 文件 |

### 环境变量配置

部署脚本会自动配置以下环境变量：

```bash
# Claude CLI
export ANTHROPIC_BASE_URL="http://127.0.0.1:15721"
export ANTHROPIC_API_KEY="sk-placeholder-managed-by-cc-switch"

# Codex CLI (配置文件: ~/.codex/config.toml)
# Gemini CLI (配置文件: ~/.gemini/.env)
```

---

## 快速开始

### 1. 启动服务器

```bash
# 启动代理服务器
csc server start

# 指定端口启动
csc server start --port 15721 --host 127.0.0.1
```

### 2. 查看供应商

```bash
# 列出 Claude 供应商
csc provider list --app claude

# 列出 Codex 供应商
csc provider list --app codex

# 列出 Gemini 供应商
csc provider list --app gemini
```

### 3. 切换供应商

```bash
# 切换到指定供应商
csc provider switch --app codex --id <供应商ID>
```

### 4. 查看帮助

```bash
# 查看主帮助
csc --help

# 查看子命令帮助
csc provider --help
csc config --help
```

---

## 命令概览

CC-Switch CLI 提供以下主要命令组：

| 命令 | 别名 | 说明 |
|------|------|------|
| `server` | `srv` | 服务器管理（启动/停止/状态） |
| `provider` | `p` | 供应商管理（增删改查/权重设置） |
| `config` | `cfg` | 配置管理（查看/修改配置） |
| `failover` | `fo` | 故障转移管理（队列管理/熔断器） |
| `stats` | `st` | 统计信息（用量/请求日志） |
| `mcp` | `m` | MCP服务器管理 |
| `prompt` | `pr` | 提示词管理 |
| `skill` | `sk` | 技能管理 |

---

## 服务器管理 (server)

### 启动服务器

```bash
# 基本启动
csc server start

# 指定端口和地址
csc server start --port 15721 --host 127.0.0.1

# 后台运行
csc server start --daemon
```

**参数说明：**

| 参数 | 短选项 | 默认值 | 说明 |
|------|--------|--------|------|
| `--port` | `-p` | 15721 | 监听端口 |
| `--host` | `-H` | 127.0.0.1 | 监听地址 |
| `--daemon` | `-d` | false | 后台运行 |

### 停止服务器

```bash
csc server stop
```

### 查看状态

```bash
csc server status
```

输出示例：
```
● 代理服务器 运行中 (PID: 12345)
监听地址: 127.0.0.1:15721
启用应用: ["claude", "codex", "gemini"]
```

### 重启服务器

```bash
# 使用默认端口重启
csc server restart

# 指定端口重启
csc server restart --port 15721
```

---

## 供应商管理 (provider)

### 列出供应商

```bash
# 基本列表
csc provider list --app claude

# 详细信息
csc provider list --app claude --verbose
# 或
csc p ls --app claude -v
```

**输出示例（表格模式）：**

```
┌──────────────────────────────────────┬──────────┬──────┬──────┬──────────┐
│ ID                                   │ 名称     │ 权重 │ 当前 │ 故障转移 │
├──────────────────────────────────────┼──────────┼──────┼──────┼──────────┤
│ abc123-def456                        │ OpenAI   │ 1    │ ✓    │          │
│ xyz789-uvw012                        │ Azure    │ 2    │      │ ✓        │
└──────────────────────────────────────┴──────────┴──────┴──────┴──────────┘
```

### 添加供应商

```bash
# 使用参数添加
csc provider add --app claude --name "My Provider" --key "sk-xxx" --url "https://api.example.com"

# 从配置文件添加
csc provider add --app claude --name "My Provider" --file ./provider-config.json
```

**参数说明：**

| 参数 | 短选项 | 必填 | 说明 |
|------|--------|------|------|
| `--app` | `-a` | 是 | 应用类型（claude/codex/gemini） |
| `--name` | `-n` | 是 | 供应商名称 |
| `--key` | `-k` | 否 | API Key |
| `--url` | `-u` | 否 | Base URL |
| `--file` | `-f` | 否 | JSON 配置文件路径 |

**配置文件示例 (provider-config.json)：**

```json
{
  "env": {
    "ANTHROPIC_API_KEY": "sk-xxx",
    "ANTHROPIC_BASE_URL": "https://api.example.com"
  }
}
```

### 删除供应商

```bash
csc provider remove --app claude --id <供应商ID>
# 或
csc p rm --app claude --id <供应商ID>
```

### 切换供应商

```bash
csc provider switch --app codex --id <供应商ID>
# 或
csc p sw --app codex --id <供应商ID>
```

### 设置权重

```bash
csc provider weight --app codex --id <供应商ID> --weight 3
# 或
csc p wt --app codex --id <供应商ID> --weight 3
```

**权重说明：**

| 权重值 | 说明 |
|--------|------|
| 0 | 禁用供应商 |
| 1 | 每轮都使用（默认） |
| 2 | 每2轮使用一次 |
| N | 每N轮使用一次（频率=1/N） |

### 查看供应商详情

```bash
csc provider show --app claude --id <供应商ID>
# 或
csc p info --app claude --id <供应商ID>
```

### 测试供应商连接

```bash
csc provider test --app claude --id <供应商ID>
```

---

## 配置管理 (config)

### 查看配置

```bash
# 查看全局配置
csc config show

# 查看特定应用配置
csc config show --app claude
```

### 设置配置

```bash
# 设置全局代理
csc config set --key global_proxy --value "http://proxy.example.com:8080"

# 设置应用端口
csc config set --key port --value 15721 --app claude

# 启用/禁用应用
csc config set --key enabled --value true --app codex
```

### 查看代理配置

```bash
# 查看所有代理配置
csc config proxy

# 查看特定应用代理配置
csc config proxy --app claude
```

**输出示例：**

```
CLAUDE 代理配置
────────────────────────────────────────────────────────────────────────────────
启用    : 是
监听地址: 127.0.0.1:15721
非流式超时: 120秒
流式首字节超时: 30秒
流式空闲超时: 60秒
```

### 权重轮询配置（负载均衡）

```bash
# 查看权重轮询状态
csc config lb --app codex

# 启用权重轮询
csc config lb --app codex --enabled true

# 禁用权重轮询
csc config lb --app codex --enabled false
```

**输出示例：**

```
CODEX 权重轮询配置
────────────────────────────────────────────────────────────────────────────────
状态        : 启用
自动故障转移: 启用

供应商权重:
┌──────────────────────────────────────┬──────────┬──────┬──────┐
│ ID                                   │ 名称     │ 权重 │ 频率 │
├──────────────────────────────────────┼──────────┼──────┼──────┤
│ abc123                               │ Provider1│ 1    │ 1/1  │
│ def456                               │ Provider2│ 3    │ 1/3  │
└──────────────────────────────────────┴──────────┴──────┴──────┘
```

### 导出/导入配置

```bash
# 导出配置
csc config export --output ./backup-config.json

# 导入配置
csc config import --input ./backup-config.json
```

---

## 故障转移 (failover)

### 查看故障转移队列

```bash
csc failover queue --app claude
```

**输出示例：**

```
CLAUDE 故障转移队列
────────────────────────────────────────────────────────────────────────────────
┌──────┬──────────────────────────────────────┬──────────┬──────┐
│ 顺序 │ ID                                   │ 名称     │ 权重 │
├──────┼──────────────────────────────────────┼──────────┼──────┤
│ 1    │ abc123                               │ Primary  │ 1    │
│ 2    │ def456                               │ Backup   │ 2    │
└──────┴──────────────────────────────────────┴──────────┴──────┘

自动故障转移: 启用
```

### 添加到故障转移队列

```bash
csc failover add --app claude --id <供应商ID>
```

### 从队列移除

```bash
csc failover remove --app claude --id <供应商ID>
```

### 启用/禁用自动故障转移

```bash
# 启用
csc failover toggle --app claude --enabled true

# 禁用
csc failover toggle --app claude --enabled false
```

### 查看熔断器状态

```bash
# 查看所有供应商的熔断器状态
csc failover circuit-breaker --app claude

# 查看特定供应商
csc failover circuit-breaker --app claude --id <供应商ID>
```

### 重置熔断器

```bash
csc failover reset --app claude --id <供应商ID>
```

---

## 统计信息 (stats)

### 查看用量摘要

```bash
# 最近7天摘要
csc stats summary

# 指定天数
csc stats summary --days 30

# 特定应用
csc stats summary --days 7 --app claude
```

### 查看供应商统计

```bash
# 所有供应商
csc stats provider --app claude

# 特定供应商
csc stats provider --app claude --id <供应商ID> --days 7
```

### 查看模型统计

```bash
csc stats model --days 7
```

### 查看请求日志

```bash
# 最近50条
csc stats logs

# 指定条数和过滤
csc stats logs --limit 100 --app claude --provider <供应商ID>
```

---

## MCP服务器管理 (mcp)

### 列出MCP服务器

```bash
# 列出所有
csc mcp list

# 按应用过滤
csc mcp list --app claude
```

### 添加MCP服务器

```bash
csc mcp add \
  --name "my-mcp-server" \
  --command "npx" \
  --args "-y" \
  --args "@modelcontextprotocol/server-filesystem" \
  --enabled claude \
  --enabled codex
```

### 删除MCP服务器

```bash
csc mcp remove --name "my-mcp-server"
```

### 启用/禁用MCP服务器

```bash
# 启用
csc mcp toggle --name "my-mcp-server" --app claude --enabled true

# 禁用
csc mcp toggle --name "my-mcp-server" --app claude --enabled false
```

---

## 提示词管理 (prompt)

### 列出提示词

```bash
# 列出所有
csc prompt list

# 按应用过滤
csc prompt list --app claude
```

### 添加提示词

```bash
csc prompt add \
  --name "code-review" \
  --content "请帮我审查以下代码..." \
  --app claude
```

### 删除提示词

```bash
csc prompt remove --name "code-review" --app claude
```

### 查看提示词内容

```bash
csc prompt show --name "code-review" --app claude
```

---

## 技能管理 (skill)

### 列出已安装技能

```bash
# 列出所有
csc skill list

# 按应用过滤
csc skill list --app claude
```

### 安装技能

```bash
csc skill install --id "skill-id" --apps claude --apps codex
```

### 卸载技能

```bash
# 从所有应用卸载
csc skill uninstall --id "skill-id"

# 从特定应用卸载
csc skill uninstall --id "skill-id" --app claude
```

### 发现可用技能

```bash
csc skill discover
```

---

## 常见问题

### Q: 如何查看服务器日志？

```bash
# 查看服务器日志
tail -f ~/.cc-switch/logs/server.log

# 查看代理日志
tail -f ~/.cc-switch/logs/proxy.log
```

### Q: 端口被占用怎么办？

```bash
# 检查端口占用
lsof -i :15721

# 使用其他端口启动
csc server start --port 15722
```

### Q: 如何完全重置配置？

```bash
# 停止服务器
csc server stop

# 删除配置目录
rm -rf ~/.cc-switch

# 重新部署
./install-ccs.sh
```

### Q: 如何在多台机器间同步配置？

```bash
# 导出配置
csc config export --output ./my-config.json

# 在另一台机器导入
csc config import --input ./my-config.json
```

### Q: 权重轮询和故障转移有什么区别？

| 特性 | 权重轮询 | 故障转移 |
|------|----------|----------|
| 目的 | 负载均衡 | 高可用 |
| 触发条件 | 每次请求 | 供应商失败时 |
| 供应商选择 | 按权重分配 | 按队列顺序 |
| 适用场景 | 多供应商分流 | 备份切换 |

### Q: 如何设置上游代理？

```bash
# 设置全局代理
csc config set --key global_proxy --value "http://proxy.example.com:8080"

# 清除代理
csc config set --key global_proxy --value ""
```

---

## 命令速查表

```bash
# 服务器管理
csc server start                          # 启动服务器
csc server stop                           # 停止服务器
csc server status                         # 查看状态
csc server restart                        # 重启服务器

# 供应商管理
csc provider list --app claude            # 列出供应商
csc provider add --app claude --name xxx  # 添加供应商
csc provider remove --app claude --id xxx # 删除供应商
csc provider switch --app claude --id xxx # 切换供应商
csc provider weight --app claude --id xxx --weight 2  # 设置权重

# 配置管理
csc config show                           # 查看配置
csc config proxy                          # 查看代理配置
csc config lb --app claude                # 查看负载均衡
csc config lb --app claude --enabled true # 启用负载均衡

# 故障转移
csc failover queue --app claude           # 查看队列
csc failover add --app claude --id xxx    # 添加到队列
csc failover toggle --app claude --enabled true  # 启用故障转移

# 帮助
csc --help                                # 主帮助
csc <command> --help                      # 子命令帮助
```

---

## 版本信息

```bash
csc --version
```

---

*文档版本: v3.9.1+*
*最后更新: 2026-01-16*
