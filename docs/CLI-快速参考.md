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

## 日志位置
```bash
tail -f ~/.cc-switch/logs/server.log   # 服务器日志
```

## 详细文档
查看完整教程: `~/.cc-switch/docs/CLI-使用教程.md`
