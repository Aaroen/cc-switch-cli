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
bash -lc 'set -euo pipefail; repo="Aaroen/cc-switch-cli"; asset="cc-switch-cli-linux-x86_64.tar.gz"; tag="${TAG:-latest}"; cache_root="${CACHE_DIR:-$HOME/.cc-switch/.cache/prebuilt}"; cache="$cache_root/$tag"; mkdir -p "$cache"; if [ "$tag" = "latest" ]; then gh_url="https://github.com/$repo/releases/latest/download/$asset"; else gh_url="https://github.com/$repo/releases/download/$tag/$asset"; fi; urls=("$gh_url" "https://mirror.ghproxy.com/$gh_url" "https://ghproxy.com/$gh_url"); meta="$cache/meta.txt"; pkg="$cache/$asset"; get_meta(){ local url="$1"; local hdr="$2"; local eff; local etag; eff="$(curl -4fsSLI -D "$hdr" -o /dev/null -w "%{url_effective}" "$url")"; etag="$(grep -i "^etag:" "$hdr" | tail -n1 | sed -E "s/^etag:[[:space:]]*//I; s/\\r//g; s/\\\"//g")"; printf "url=%s\\netag=%s\\n" "$eff" "$etag"; }; need_dl=1; if [ -f "$pkg" ]; then if command -v curl >/dev/null 2>&1 && [ -f "$meta" ]; then tmp="$(mktemp -d)"; trap "rm -rf \\"$tmp\\"" EXIT; get_meta "$gh_url" "$tmp/hdr" > "$tmp/meta.new" 2>/dev/null || true; if [ -s "$tmp/meta.new" ] && cmp -s "$tmp/meta.new" "$meta"; then need_dl=0; fi; rm -rf "$tmp"; trap - EXIT; else need_dl=0; fi; fi; [ "${FORCE:-0}" = "1" ] && need_dl=1; if [ "$need_dl" = 1 ]; then tmp="$(mktemp -d)"; trap "rm -rf \\"$tmp\\"" EXIT; ok=0; for url in "${urls[@]}"; do echo "下载: $url"; if command -v curl >/dev/null 2>&1; then if curl -4 -fL --connect-timeout 10 --max-time 600 --speed-time 20 --speed-limit 1024 --retry 3 --retry-delay 1 --retry-all-errors "$url" -o "$tmp/$asset"; then tar -tzf "$tmp/$asset" >/dev/null; mv -f "$tmp/$asset" "$pkg"; get_meta "$gh_url" "$tmp/hdr2" > "$meta" 2>/dev/null || printf "url=%s\\netag=\\n" "$url" > "$meta"; ok=1; break; fi; else if wget -4 -nv --timeout=20 --tries=3 "$url" -O "$tmp/$asset"; then tar -tzf "$tmp/$asset" >/dev/null; mv -f "$tmp/$asset" "$pkg"; printf "url=%s\\netag=\\n" "$url" > "$meta"; ok=1; break; fi; fi; done; [ "$ok" = 1 ]; rm -rf "$tmp"; trap - EXIT; fi; tmp="$(mktemp -d)"; trap "rm -rf \\"$tmp\\"" EXIT; tar -xzf "$pkg" -C "$tmp"; bash "$tmp/install-ccs.sh" --prebuilt "$tmp/cc-switch"'
```

说明：

- 该命令依赖 Releases 中存在 `cc-switch-cli-linux-x86_64.tar.gz` 资产（包含 `install-ccs.sh` 与 `cc-switch` 二进制）。
- 如 `latest` 暂不可用，可指定版本：`TAG=v3.9.1-3`（示例）后再运行上面的一行命令。
- 默认会将下载的 Release 资产缓存到 `~/.cc-switch/.cache/prebuilt/<TAG>/`；如需强制重新下载：`FORCE=1`。
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

# 设置供应商权重（0-100）
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
