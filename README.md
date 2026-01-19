# CC-Switch CLI 版（cc-switch-cli）

本项目是上游 [farion1231/cc-switch](https://github.com/farion1231/cc-switch) 的 CLI/无头（Headless）变体：保留核心代理与配置能力，面向 Linux 终端与服务器环境，重点提供稳定的一键部署与命令行管理体验。

上游项目的原始 README 已保留为：`README_UPSTREAM.md`。

## 一键部署

### Git 版本（实时更新代码）

目录存在则自动 `git pull --rebase` 更新后再执行部署脚本；不存在则 `git clone` 后部署。该模式会在本机编译（首次耗时较久，但代码最新）。

```bash
bash -lc 'set -e; dir="$HOME/cc-switch-cli"; if [ -d "$dir/.git" ]; then git -C "$dir" pull --rebase; else git clone https://github.com/Aaroen/cc-switch-cli.git "$dir"; fi; bash "$dir/install-ccs.sh"'
```

### Release 版本（免编译，可能滞后）

从 GitHub Releases 下载预构建二进制并安装运行（无需本机编译）。当前默认资产名：

- `cc-switch-cli-linux-x86_64.tar.gz`

```bash
bash -lc 'set -e; repo="Aaroen/cc-switch-cli"; asset="cc-switch-cli-linux-x86_64.tar.gz"; tmp="$(mktemp -d)"; if command -v curl >/dev/null 2>&1; then curl -fsSL "https://github.com/$repo/releases/latest/download/$asset" -o "$tmp/$asset"; else wget -qO "$tmp/$asset" "https://github.com/$repo/releases/latest/download/$asset"; fi; tar -xzf "$tmp/$asset" -C "$tmp"; bash "$tmp/install-ccs.sh" --prebuilt "$tmp/cc-switch"'
```

说明：

- 该命令依赖 Releases 中存在 `cc-switch-cli-linux-x86_64.tar.gz` 资产（包含 `install-ccs.sh` 与 `cc-switch` 二进制）。
- 默认以 CLI 模式部署（无头 server），并自动处理端口占用（必要时自动换端口）。
- 如需 GUI 模式（会启动 Tauri 界面）：使用 Git 版本并执行 `bash "$HOME/cc-switch-cli/install-ccs.sh" --gui`。

## 权重轮询（Weight Round Robin）

本仓库在 CLI 场景下默认启用“按供应商权重分配请求”的能力（部署脚本会为 `claude/codex/gemini` 默认打开权重轮询）。

规则（与 CLI 输出保持一致）：

- `weight=0`：禁用该供应商
- `weight=1`：每轮都使用（最高频）
- `weight=N`：每 N 轮使用一次

常用命令：

```bash
# 查看某个 app 的负载均衡/权重轮询状态与供应商权重表
csc config lb --app claude

# 启用/禁用权重轮询
csc config lb --app claude --enabled true
csc config lb --app claude --enabled false

# 列出供应商并获取 ID
csc provider list --app claude

# 设置供应商权重（0-10）
csc provider weight --app claude --id <PROVIDER_ID> --weight 1
```

## 启动与验证

部署完成后通常需要重新加载 shell 环境变量：

```bash
source ~/.bashrc  # 或 source ~/.zshrc
```

然后可用以下命令确认服务状态：

```bash
csc server status
```
