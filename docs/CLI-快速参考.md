# CC-Switch CLI 快速参考

## 核心命令

### 服务器管理
```bash
# 推荐：简写命令
csc start                           # 启动服务器（默认后台运行，可直接关闭终端）
csc status                          # 查看状态
csc stop                            # 停止服务器

# 完整命令（等价）
csc server start                    # 启动服务器（默认后台运行，可直接关闭终端）
csc server start --foreground        # 前台运行（调试/查看实时输出）
csc server stop                     # 停止服务器
csc server status                   # 查看状态
csc server start --port 15721       # 指定端口启动
```

### 供应商管理
```bash
# 列表/详情
csc provider list --app claude      # 列出 Claude 供应商
csc provider list --app codex       # 列出 Codex 供应商
csc provider list --app gemini      # 列出 Gemini 供应商
csc provider list --app codex --verbose        # 显示更详细信息（含配置摘要）
csc provider show --app codex --id <ID>        # 查看供应商详情（含 settings_config）

# 添加/删除/切换
csc provider add --app codex --name <NAME> --key <OPENAI_API_KEY> --url <BASE_URL>
csc provider add --app claude --name <NAME> --key <ANTHROPIC_API_KEY> --url <ANTHROPIC_BASE_URL>
csc provider add --app gemini --name <NAME> --key <GEMINI_API_KEY> --url <GEMINI_API_BASE_URL>

# 从 JSON 文件添加（推荐：便于复用/迁移）
csc provider add --app codex --name <NAME> --file ./provider.codex.json

csc provider remove --app codex --id <ID>      # 删除供应商（会二次确认）
csc provider switch --app codex --id <ID>  # 切换供应商

# 修改配置（最常用的“编辑”能力）
csc provider weight --app codex --id <ID> --weight 3  # 设置权重
csc provider map --app codex --id <ID> --from gpt-5.2 --to gpt-5.2-2cx  # 设置模型映射
csc provider env --app claude --id <ID> --key ANTHROPIC_DEFAULT_SONNET_MODEL --value <MODEL>  # 设置 Claude 模型映射

# 更新供应商 settings_config（merge 合并，默认不破坏已有字段）
csc provider update --app codex --id <ID> --file ./patch.codex.json

# 替换整个 settings_config（会覆盖原有配置）
csc provider update --app codex --id <ID> --file ./full.codex.json --replace

# 直接修改 key/base_url（无需文件；会写入 settings_config）
csc provider update --app codex --id <ID> --key <OPENAI_API_KEY>
csc provider update --app codex --id <ID> --url <BASE_URL>

# 同时改名/备注
csc provider update --app codex --id <ID> --name <NEW_NAME> --notes "..."

# 导入/导出（用于备份/迁移）
# 注意：默认导出会包含密钥/令牌等敏感信息，请妥善保管
csc provider export --app codex --output ./codex.providers.json
csc provider import --app codex --input ./codex.providers.json

# 仅导出一个供应商
csc provider export --app codex --id <ID> --output ./one.provider.json

# 脱敏导出（用于分享模板，导入后不可直接使用）
csc provider export --app codex --output ./codex.providers.redacted.json --redact

# 导入时生成新 ID（避免与现有冲突）
csc provider import --app codex --input ./codex.providers.json --new-ids

# 覆盖同 ID 供应商（默认同 ID 会跳过）
csc provider import --app codex --input ./codex.providers.json --overwrite
```

### 供应商 JSON 配置示例（配合 `csc provider add --file`）

Codex（OpenAI 兼容）示例 `provider.codex.json`：
```json
{
  "base_url": "https://example.com",
  "env": {
    "OPENAI_API_KEY": "sk-..."
  }
}
```

Claude 示例 `provider.claude.json`：
```json
{
  "env": {
    "ANTHROPIC_API_KEY": "sk-ant-...",
    "ANTHROPIC_BASE_URL": "https://api.anthropic.com"
  }
}
```

Gemini 示例 `provider.gemini.json`：
```json
{
  "env": {
    "GEMINI_API_KEY": "AIza...",
    "GEMINI_API_BASE_URL": "https://generativelanguage.googleapis.com"
  }
}
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
