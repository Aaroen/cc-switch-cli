# CC-Switch CLI 快速参考

## 核心命令

### 服务器管理
```bash
csc server start                    # 启动服务器
csc server stop                     # 停止服务器
csc server status                   # 查看状态
csc server start --port 15721       # 指定端口启动
```

### 供应商管理
```bash
csc provider list --app claude      # 列出 Claude 供应商
csc provider list --app codex       # 列出 Codex 供应商
csc provider list --app gemini      # 列出 Gemini 供应商
csc provider switch --app codex --id <ID>  # 切换供应商
csc provider weight --app codex --id <ID> --weight 3  # 设置权重
csc provider map --app codex --id <ID> --from gpt-5.2 --to gpt-5.2-2cx  # 设置模型映射
csc provider env --app claude --id <ID> --key ANTHROPIC_DEFAULT_SONNET_MODEL --value <MODEL>  # 设置 Claude 模型映射
```

### 权重轮询（负载均衡）
```bash
csc config lb --app codex                    # 查看状态
csc config lb --app codex --enabled true     # 启用
csc config lb --app codex --enabled false    # 禁用
```

### 故障转移
```bash
csc failover queue --app claude              # 查看队列
csc failover add --app claude --id <ID>      # 添加到队列
csc failover toggle --app claude --enabled true  # 启用故障转移
```

### 配置查看
```bash
csc config show                     # 查看全局配置
csc config show --app claude        # 查看应用配置
csc config proxy                    # 查看代理配置
```

### 帮助
```bash
csc --help                          # 主帮助
csc provider --help                 # 供应商命令帮助
csc config --help                   # 配置命令帮助
csc failover --help                 # 故障转移帮助
```

## 命令别名

| 完整命令 | 别名 |
|----------|------|
| `server` | `srv` |
| `provider` | `p` |
| `config` | `cfg` |
| `failover` | `fo` |
| `stats` | `st` |

## 权重说明

| 权重 | 说明 |
|------|------|
| 0 | 禁用 |
| 1 | 每轮使用（默认） |
| N | 每N轮使用一次 |

## 模型映射说明

- 支持在供应商配置中设置 `model_mapping`（JSON 对象）
- CLI: `csc provider map --app codex --id <ID> --from <旧模型> --to <新模型>`
- 映射示例：
  - `gpt-5.2` → `gpt-5.2-2cx`
  - `gpt-5.2-codex` → `gpt-5.2-codex-2cx`

## Codex 供应商配置（示例：hyb）

> hyb 在 Codex 下使用 **OPENAI_API_KEY + base_url**，不要使用 Claude 的 `env` 配置字段。

**推荐配置文件（JSON）**：
```json
{
  "base_url": "https://ai.hybgzs.com",
  "env": {
    "OPENAI_API_KEY": "<YOUR_KEY>"
  }
}
```

**CLI 创建并切换**：
```bash
csc provider add --app codex --name hyb --file ./hyb.codex.json
csc provider switch --app codex --id <ID>
```

## Claude 模型映射（Haiku→Sonnet）

- Claude 模型映射使用 `env` 字段（四键模型配置）
- CLI: `csc provider env --app claude --id <ID> --key ANTHROPIC_DEFAULT_HAIKU_MODEL --value <SONNET_MODEL>`
- 常用键：
  - `ANTHROPIC_DEFAULT_HAIKU_MODEL`
  - `ANTHROPIC_DEFAULT_SONNET_MODEL`
  - `ANTHROPIC_DEFAULT_OPUS_MODEL`
  - `ANTHROPIC_MODEL`
- 通过为 Haiku 配置 Sonnet 目标，实现“Haiku 全部映射为 Sonnet”

## 日志位置
```bash
tail -f ~/.cc-switch/logs/server.log   # 服务器日志
```

## 详细文档
查看完整教程: `~/.cc-switch/docs/CLI-使用教程.md`
