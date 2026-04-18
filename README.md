# CC-Switch CLI 版（cc-switch-cli）

本项目是上游 [farion1231/cc-switch](https://github.com/farion1231/cc-switch) 的 CLI/无头（Headless）变体：在持续贴近上游结构的同时，保留并强化以下核心能力：

- 供应商权重轮询
- 无头终端运行与后台代理服务
- CLI 全功能控制
- GUI 中对轮询/故障转移的可视化配置入口

命令入口以 `ccs` 为主；同时继续保留 `csc` 与 `cc-switch` 的兼容调用。

上游项目的原始 README 已保留为：`README_UPSTREAM.md`。

## 一键部署

Linux 终端环境可直接使用下述一行命令自动识别 `x86_64/arm64` 并安装 CLI 版本：

```bash
bash -lc 'set -euo pipefail; repo="Aaroen/cc-switch-cli"; arch="$(uname -m)"; case "$arch" in x86_64|amd64) asset="cc-switch-cli-linux-x86_64.tar.gz" ;; aarch64|arm64) asset="cc-switch-cli-linux-arm64.tar.gz" ;; *) echo "暂不支持的架构: $arch" >&2; exit 1 ;; esac; tag="${TAG:-latest}"; cache_root="${CACHE_DIR:-$HOME/.cc-switch/.cache/prebuilt}"; cache="$cache_root/$tag"; mkdir -p "$cache"; if [ "$tag" = "latest" ]; then gh_url="https://github.com/$repo/releases/latest/download/$asset"; else gh_url="https://github.com/$repo/releases/download/$tag/$asset"; fi; urls=("$gh_url" "https://mirror.ghproxy.com/$gh_url" "https://ghproxy.com/$gh_url"); meta="$cache/meta.txt"; pkg="$cache/$asset"; get_meta(){ local url="$1"; local hdr="$2"; local eff; local etag; eff="$(curl -4fsSLI -D "$hdr" -o /dev/null -w "%{url_effective}" "$url")"; etag="$(grep -i "^etag:" "$hdr" | tail -n1 | sed -E "s/^etag:[[:space:]]*//I; s/\\r//g; s/\\\"//g")"; printf "url=%s\\netag=%s\\n" "$eff" "$etag"; }; need_dl=1; if [ -f "$pkg" ]; then if command -v curl >/dev/null 2>&1 && [ -f "$meta" ]; then tmp="$(mktemp -d)"; trap "rm -rf \\"$tmp\\"" EXIT; get_meta "$gh_url" "$tmp/hdr" > "$tmp/meta.new" 2>/dev/null || true; if [ -s "$tmp/meta.new" ] && cmp -s "$tmp/meta.new" "$meta"; then need_dl=0; fi; rm -rf "$tmp"; trap - EXIT; else need_dl=0; fi; fi; [ "${FORCE:-0}" = "1" ] && need_dl=1; if [ "$need_dl" = 1 ]; then tmp="$(mktemp -d)"; trap "rm -rf \\"$tmp\\"" EXIT; ok=0; for url in "${urls[@]}"; do echo "下载: $url"; if command -v curl >/dev/null 2>&1; then if curl -4 -fL --connect-timeout 10 --max-time 600 --speed-time 20 --speed-limit 1024 --retry 3 --retry-delay 1 --retry-all-errors "$url" -o "$tmp/$asset"; then tar -tzf "$tmp/$asset" >/dev/null; mv -f "$tmp/$asset" "$pkg"; get_meta "$gh_url" "$tmp/hdr2" > "$meta" 2>/dev/null || printf "url=%s\\netag=\\n" "$url" > "$meta"; ok=1; break; fi; else if wget -4 -nv --timeout=20 --tries=3 "$url" -O "$tmp/$asset"; then tar -tzf "$tmp/$asset" >/dev/null; mv -f "$tmp/$asset" "$pkg"; printf "url=%s\\netag=\\n" "$url" > "$meta"; ok=1; break; fi; fi; done; [ "$ok" = 1 ]; rm -rf "$tmp"; trap - EXIT; fi; tmp="$(mktemp -d)"; trap "rm -rf \\"$tmp\\"" EXIT; tar -xzf "$pkg" -C "$tmp"; raw="https://raw.githubusercontent.com/$repo/cc-switch-cli/install-ccs.sh"; raw_urls=("$raw" "https://mirror.ghproxy.com/$raw" "https://ghproxy.com/$raw"); ok=0; for url in "${raw_urls[@]}"; do echo "获取最新 install-ccs.sh: $url"; if command -v curl >/dev/null 2>&1; then if curl -4 -fL --connect-timeout 10 --max-time 120 --retry 3 --retry-delay 1 --retry-all-errors "$url" -o "$tmp/install-ccs.sh"; then ok=1; break; fi; else if wget -4 -nv --timeout=20 --tries=3 "$url" -O "$tmp/install-ccs.sh"; then ok=1; break; fi; fi; done; [ "$ok" = 1 ]; chmod +x "$tmp/install-ccs.sh"; bash "$tmp/install-ccs.sh" --prebuilt "$tmp/cc-switch"'
```

说明：

- 该命令依赖 Releases 中存在 `cc-switch-cli-linux-x86_64.tar.gz` 或 `cc-switch-cli-linux-arm64.tar.gz` 资产，压缩包内包含 `install-ccs.sh`、`cc-switch`、`cc-switch-cli` 与 `ccs/csc` 兼容入口。
- 如 `latest` 暂不可用，可指定版本：`TAG=v3.11.4`（示例）后再运行上面的一行命令。
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

## 启动与验证

部署完成后通常需要重新加载 shell 环境变量：

```bash
source ~/.bashrc  # 或 source ~/.zshrc
```

然后可用以下命令确认服务状态：

```bash
ccs server status
```
