# 方案设计:负载均衡策略重构 · CLI 输出美化 · CLI 专属 Web 控制台

> 状态:设计已通过对抗式评审(本文件已折叠全部阻断项修正)。按"执行顺序"分阶段实施。
> 适用仓库:`cc-switch-cli`(Tauri 2 应用,Rust 后端 `src-tauri/`,React+Vite 前端 `src/`)。
> 所有路径均为仓库相对路径。

## 一、背景与目标

二次开发版在官方 `cc-switch` 基础上新增了代理负载均衡能力。本次工作包含三条相互独立、可分阶段交付的工作线:

1. **负载均衡策略重构(核心)**:现有"权重轮询"存在"始终命中某个最低权重供应商"的问题。引入可插拔的三策略机制(频率控制 / 加权随机 / 硬全轮询)。
2. **CLI 输出美化**:保留全部现有命令与脚本可解析输出,仅美化人类可读的显示与排版。
3. **CLI 专属 Web 控制台**:仅在无头 CLI 模式下提供基于浏览器的可视化控制台(GUI 已内嵌面板,故不重复),端口独立于代理服务端口。

### 已确定的产品决策

- 新策略采用**正向权重语义**(权重越大流量越多);现有反向频率算法保留为独立可选策略 `frequency` 以兼容老数据。
- Web 控制台**仅在无头 CLI 模式**提供;复用同一套 React 组件;聚焦"代理 / 负载均衡 / 用量"控制面,而非完整复制全部 250 条命令面板。
- Web 控制台端口独立可配,默认绑定 `127.0.0.1`。

## 二、现状诊断

### 2.1 整体架构

- 业务逻辑已与 Tauri 解耦:`store.rs` 的 `AppState { db: Arc<Database>, proxy_service: ProxyService }` 不依赖 Tauri;无头 CLI(`cli/server.rs`)以 `app_handle: None` 启动同一代理引擎,证明可纯无头运行。
- 代理为 axum + 手写 hyper accept loop,监听 `{listen_address}:{listen_port}`(默认 `127.0.0.1:15721`)。
- 前端为状态驱动 SPA(无路由);22 个 `src/lib/api/*.ts` 各自直接 `import { invoke } from "@tauri-apps/api/core"`,无统一收口点。
- 全仓库除转发代理外**无任何管理类 HTTP 接口**。

### 2.2 权重轮询 bug 根因(已确认)

`proxy/load_balancer.rs` 的 `FrequencyControlledRR` 采用**反向频率语义**:`weight=1` 每轮命中(频率最高)、`weight=N` 每 N 轮命中、`weight=0` 禁用。`select()` 每请求调用一次(`handler_context.rs:127`),转发器仅顺序遍历预选候选列表做故障转移,不重新选择。

数学机理:最小非零权重的供应商在最多轮次中 `global_round % weight == 0` 成立;且无 eligible 时 fallback 无条件 `find(weight==1)`。两者叠加,导致最低权重供应商被压倒性选中。该语义与"权重越大越优先"的普遍直觉相反,是问题的根源。

### 2.3 持久化关键事实

- `weight_round_robin_enabled` 实际持久化在键值表 `settings`(键 `proxy_weight_round_robin_enabled_{app_type}`),而非 `proxy_config` 列(列虽存在但运行时不写)。
- `AppProxyConfig` 标注 `#[serde(rename_all="camelCase")]`,Rust snake_case 与前端 camelCase 自动桥接。
- `provider.weight: u32`(默认 1,范围 0-100),双存储于 `providers.weight` 列与 `meta.routingWeight`。

### 2.4 CLI 现状

- 已存在集中输出层 `cli/output.rs`(171 行,88 处调用),已直接依赖 `comfy-table = "7.1"`。
- 唯一可移植性缺陷:`output.rs::hint()`(第 158 行)输出裸 `\x1b[90m`,管道时泄漏 ANSI。
- `clap` 锁定解析为 4.6.x,其 `anstream 1.0` / `anstyle 1.0` / `anstyle-wincon` 已在 `Cargo.lock` 中(经 clap 传递引入)。

---

## 三、工作线一:负载均衡策略重构

### 3.1 设计

将单一算法 `FrequencyControlledRR` 重构为**枚举派发**的 `LoadBalancer`,承载三策略。采用枚举派发而非 `Box<dyn Trait>`,使 `select()` 保持同步 `&mut self`,可在 `load_balancers` 的 `RwLock` 写锁内执行且无 `.await`。

```text
enum LoadBalanceStrategy { Frequency(默认), WeightedRandom, HardRoundRobin }   // serde snake_case
struct LoadBalancer { strategy, providers: Vec<WeightedProvider>, global_round: u32, rng_state: u64 }
```

- **Frequency**:沿用现有反向频率算法,逐行保留(`weight=N` 每 N 轮)。
- **WeightedRandom**:正向权重,`p_i = weight_i / Σweight`(`u64` 累加器防溢出,全零守卫返回 `None`);使用自包含 **SplitMix64** 同步抽样(不新增 `rand` 依赖,可种子化、确定性、无 `.await`)。
- **HardRoundRobin**:`index = (global_round-1) % enabled.len()`,在 `weight>0` 集合上等概率轮转,忽略权重大小(仅 0/非0 决定启用)。

`select()` 统一在 `match` 之前自增 `global_round` 一次,各臂不再自增。

### 3.2 持久化与兼容(无 schema 迁移)

沿用键值表方案(键 `proxy_load_balance_strategy_{app_type}`),与 `weight_round_robin_enabled` 一致;键缺失即 `None` → 默认 `Frequency`,天然向后兼容,**无需 `SCHEMA_VERSION` 升级**。`dao/settings.rs` 新增 `get/set_load_balance_strategy`;`dao/proxy.rs::get_proxy_config_for_app` 在两个返回分支均填充该字段。

> 升级路径:老用户的 `weight_round_robin_enabled` 键原样保留;`proxy_load_balance_strategy_*` 不存在 → 默认 `Frequency` → `select()` 走逐行保留的反向频率臂 → 选择序列与 `global_round` 进程完全一致。不删除任何数据/行/文件。

> 备选(列迁移):若评审坚持用列,则 `SCHEMA_VERSION` 9→10,新增 `migrate_v9_to_v10` 调 `add_column_if_missing(proxy_config, load_balance_strategy, "TEXT NOT NULL DEFAULT 'frequency'")`,`CREATE TABLE` 同步加列,SELECT 用 `has_column` 做 NULL 回退。本方案默认不采用。

### 3.3 对抗评审修正(必须落实)

- **[阻断] 重导出**:`proxy/mod.rs:54` 必须由 `pub use load_balancer::{FrequencyControlledRR, WeightedProvider};` 改为 `{LoadBalancer, LoadBalanceStrategy, WeightedProvider}`;改名后全仓库 grep `FrequencyControlledRR` 确认零残留。
- **[阻断] Frequency 计数器单增**:`global_round += 1` 仅在 `select()` 内执行一次;`select_frequency(&self)` 不得包含自增(原第 66 行自增需移除,非"逐行粘贴"),否则破坏所有精确序列测试。
- **[阻断] 策略写入回写覆盖**:现有 GUI 调 `update_proxy_config_for_app(config)` 不带 `loadBalanceStrategy` → serde 默认 `Frequency` → 会覆盖 CLI 设置的策略。**修正:`update_proxy_config_for_app` 不写策略;策略仅经专用命令 `set_load_balance_strategy` 写入(读改写)。** `AppProxyConfig.load_balance_strategy` 仅用于读取展示。
- **[阻断] reset 异步/同步冲突**:`update_provider_weight` 是**同步 `fn`**,不能 `.await`。**修正:不在同步命令中调用 reset;依赖 `provider_router` 的 `needs_update`(现已纳入 strategy 比较,且本就检测权重/供应商变化)在下次请求自动重建。** 可选地仅在异步专用命令 `set_load_balance_strategy` 中 `.await reset_load_balancer`。
- **[重要] 权重语义反转告警**:策略切换在后端 WARN 日志显式提示"权重含义反转"(Frequency 反向 ↔ WeightedRandom 正向);CLI `config lb` 状态列必须按策略条件渲染:`frequency` 显示 `1/N`,`weighted_random` 显示 `N/Σ (=xx%)`,`hard_round_robin` 显示 `equal`(现有硬编码 `1/N` 必须改)。不自动迁移权重数值(遵循不删改数据原则),仅显式提示。
- **[重要] WeightedRandom 选择鲁棒性**:一次收集 `weight>0` 的原始下标,按累计权重命中后直接按下标返回,避免 `idx=0` 静默兜底与双重 filter;`debug_assert` 抽样值被正确消耗。模数偏置在权重和 ≤ 数百时可忽略(加注释说明)。
- **[重要] needs_update 纳入 strategy**:策略变更触发重建(`global_round` 归零,符合预期);测试断言重建后候选列表仍包含全部非熔断 `weight>0` 供应商(故障转移尾部不丢供应商)。
- **[次要]** HardRoundRobin 轮转顺序锁定为 `provider_router` 既有的权重升序(由测试固定);键值解析失败静默回退 `Frequency`(与现有 `weight_round_robin` 一致,加注释说明);`set_load_balance_strategy` 校验 `app_type ∈ {claude,codex,gemini}`。

### 3.4 涉及文件

- `src-tauri/src/proxy/load_balancer.rs`:枚举 + `LoadBalancer` + 三策略 + 测试。
- `src-tauri/src/proxy/mod.rs`:重导出更名。
- `src-tauri/src/proxy/types.rs`:`AppProxyConfig` 加 `load_balance_strategy`(`#[serde(default)]`,只读填充)。
- `src-tauri/src/proxy/provider_router.rs`:`load_balancers` 值类型改 `LoadBalancer`;读策略;`needs_update` 加 strategy 比较。
- `src-tauri/src/database/dao/settings.rs`:`get/set_load_balance_strategy`。
- `src-tauri/src/database/dao/proxy.rs`:读填充(两分支);**不写**策略。
- `src-tauri/src/commands/proxy.rs` + `lib.rs`:`get/set_load_balance_strategy` 命令并注册。
- `src-tauri/src/cli/commands.rs`:`config lb` 加 `strategy` 参数;状态按策略条件渲染。
- `src/types/proxy.ts` + `src/components/proxy/WeightRoundRobinConfigPanel.tsx`:加策略选择器(调专用命令);权重列文案按策略切换。
- `src/i18n/locales/{zh,en,ja}.json`:策略相关文案。

### 3.5 测试

- 现有 Frequency 测试改用 `LoadBalancer { strategy: Frequency, .., rng_state:0 }` 构造,断言全部不变通过。
- 新增:WeightedRandom 分布(10k 次,误差 ±0.03,显式结构体字面量保证确定性)、全零→`None`、`weight==0` 永不命中;HardRoundRobin 精确轮转 + 忽略权重大小;`provider_router` 策略切换重建(`global_round` 归零、候选列表保全);`dao` 往返 + 空库默认 `Frequency`;serde 默认值测试。

---

## 四、工作线二:CLI 输出美化

### 4.1 设计

对既有 `cli/output.rs` 做"加固 + 增强",而非重写:统一经单一 `anstream`/`anstyle` 门控自动适配 NO_COLOR / 非 TTY(管道)/ Windows-WSL 虚拟终端;补充分节标题、对齐键值块、状态字形、当前项高亮表、错误+提示格式。保持全部辅助函数签名不变,88 处调用零改动。

### 4.2 依赖(零新增传递依赖)

- **[阻断修正] 版本对齐**:`anstream`、`anstyle` 提升为直接依赖,版本必须写 `"1.0"`(匹配 `Cargo.lock` 经 clap 4.6 锁定的 1.0.x);**严禁写 `"0.6"`**(会引入第二个 major 副本)。编辑后 `cargo tree -d | grep -E 'anstream|anstyle'` 断言单一版本。
- `comfy-table` 保持 7.1;不引入 `indicatif`/`owo-colors`/`supports-color`。

### 4.3 对抗评审修正(必须落实)

- **[阻断] 内嵌换行**:`server.rs:108` 调 `info("\n正在停止服务器...")`,字形前缀会使字形落在上一行。修正:在调用点把前导 `\n` 移出参数(先空行后 `info(...)`);grep 全部 88 处审计前导 `\n/\t`。
- **[阻断] `--color` 与 GUI 路由冲突**:`entry.rs::has_cli_args()` 以 `argv[1]` 匹配子命令决定 CLI/GUI 分流;前导 `ccs --color=always provider list` 会因 `--color` 不在匹配集而误入 GUI。修正:`has_cli_args()` 增加对全局标志(`--color`/`--no-color`/`--help`/`--version`)的识别,或在任意位置检测已知子命令。
- **[重要] 机器可读范围收窄**:仅 `provider export -o -` 真正字节稳定可管道。`provider show` / `hyperparams show` 因夹带分节标题,**本就不可** `jq`,不得标注为冻结路径,相应 `| jq` 冒烟测试删除。`export` 路径走新增 `output::raw_stdout()`(绕过 AutoStream,永不着色)。
- **[重要] 表格门控一致**:`comfy-table` 经 `println!` 输出、绕过 AutoStream;必须用同一 `ColorChoice` 驱动其 `force_no_tty()`+`ASCII_FULL`(门控为 Never 或非 TTY 时),单元格内只放纯文本/Unicode 标记、绝不放裸 ANSI。承认 crossterm 与 anstyle-query 两套检测并存,由 `output.rs` 统一裁决。
- **[重要] NO_COLOR 优先级**:显式 `--color=always` 覆盖 `NO_COLOR`(用户显式意图优先),`Auto` 经 anstream 遵循 NO_COLOR/CLICOLOR/TTY;文档明示该约定。
- **[次要]** 字形 ASCII 回退建议直接复用现有 `✓/✗`(已发布无问题);别名清单以实际 clap 定义为准(`srv/p/cfg/fo/ls/...`,删除不存在的 `st/mcp/m/pr/sk`);守护子进程日志重定向到文件,验证 `server.log` 无 `\x1b`;CJK 对齐改用 `unicode-width`(已在锁中)。

### 4.4 涉及文件与测试

- 文件:`Cargo.toml`、`cli/output.rs`、`cli/mod.rs`、`cli/entry.rs`、`cli/commands.rs`、`cli/server.rs`。
- 测试:`cargo build`/`clippy` 干净;管道无 `\x1b`、表格降级 ASCII;`NO_COLOR=1` 与 `--color=never/always` 行为;`export -o - | jq .` 通过;现有 `cli::commands` 单测通过;退出码 success=0 / Err=1;`server.log` 无 ANSI;`--color` 前导/后置均正确入 CLI。

---

## 五、工作线三:CLI 专属 Web 控制台

> 本工作线规模最大、安全敏感度最高,且依赖工作线一提供的策略命令。建议最后实施。

### 5.1 设计

新增 crate 内模块 `src-tauri/src/web_panel/`,定义 `WebPanelServer`,持有 **`Arc<AppState>`**(复用 `db` + `proxy_service`,**不**持 `AppHandle`),`axum::serve(listener, app).with_graceful_shutdown(rx)`,独立 `oneshot` 关停 + `JoinHandle`。**绝不调用 `set_proxy_port`**。

- **网关**:单一 `POST /api/invoke/:command`,JSON body 即 invoke 入参;`dispatch.rs` 每命令一臂,**调用与对应 `#[tauri::command]` 完全相同的后端逻辑**(对含 AppHandle 副作用者,抽取共享自由函数,Web 侧传 `None`)。
- **前端复用**:新增 **Web 专属精简入口 `src/main.web.tsx`**,仅挂载代理/故障转移/权重轮询/用量面板(不引导完整 `App.tsx`,从而避开侧栏/导入导出/通用供应商/窗口控制等庞大命令面与 Tauri 插件)。复用 `ProxyTabContent`、`UsageDashboard` 及其子组件。
- **实时性**:**仅轮询**(v1 不做 SSE)。Web 构建下为 `useProxyStatus`(5s)与 `useProvidersQuery`(新增 ~5s)配置刷新间隔;省去 SSE 鉴权与"运行期故障转移事件无法到达面板"的难题。SSE 列为后续增强。
- **传输切换**:新增 `src/lib/api/transport.ts`,`invoke(cmd,args)` 检测 `window.__TAURI_INTERNALS__`,Tauri 走原生 invoke,否则 `fetch('/api/invoke/'+cmd)`;经 `vite.config.ts` 的 `resolve.alias`(仅 `VITE_TARGET=web` 时)将 `@tauri-apps/api/core` 重定向至该 shim,Tauri 构建不受影响,22 个 api 文件零改动。`providersApi.updateTrayMenu` 在 api 层 `!isTauri()` 直接返回,避免浏览器抛错。
- **静态资源**:`build:web` 输出到**独立目录 `dist-web/`**(与 Tauri 的 `dist/` 分离),`rust-embed` 仅嵌入 `dist-web`,杜绝误嵌未走 shim 的 Tauri 包;构建标记校验。

### 5.2 安全模型(对抗评审,必须落实)

- **[阻断] 强制令牌 + 自定义头**:所有绑定(含回环)**强制**随机 Bearer 令牌;`/api/invoke` 额外要求非安全列表自定义头 `X-CC-Switch-Panel: 1`(强制 CORS 预检,阻断表单型 CSRF);`Origin/Host` 白名单作纵深防御。SPA 从同源注入的 meta 标签读取令牌,不走 URL query。
- **[阻断] 代理端口变更脑裂**:面板经 `update_global_proxy_config` 改 `listen_port/address` 仅写 DB、不重绑运行中代理。修正:该臂在持久化后,若端口/地址变化则经 `ProxyService::update_config`(stop+rebind)实际重绑;或在无头面板禁止端口/地址编辑并提示"需 `ccs server restart`"。
- **[阻断] 代理 0.0.0.0 暴露**:面板可将**代理**设为 `0.0.0.0`(无鉴权的用密钥中继)。修正:dispatch 层对非回环代理地址拒绝或强警告,除非 `server start` 显式传入覆盖标志。
- **[阻断] 面板端口占用使代理孤儿化**:`panel.start().await?` 在代理已启动且 PID 已写之后,失败会孤儿化代理。修正:面板绑定失败**非致命**(记日志、继续运行代理);启动前强制 `web_port != proxy listen_port`。
- **[阻断] 守护进程丢面板**:默认 `server start` 为后台;守护重启仅透传 `--host/--port`。修正:`--web-port/--web-bind` 贯穿 `start_headless_server` → `start_headless_server_daemon` → `cmd.args`;令牌在**子进程**生成并持久化到 `~/.cc-switch/panel.token`(0600),**不**打印到 `server.log`;就绪探测覆盖面板端口。`restart`/持久化 `web_panel_port` 同步处理。

### 5.3 其他评审修正

- **[阻断] 网关命令覆盖**:必须按实际复用组件逐一补齐,至少包括:全局出站代理 `get/set_global_proxy_url`、`test_proxy_url`、`get_upstream_proxy_status`、`scan_local_proxies`;整流器 `get/set_rectifier_config`、`get/set_optimizer_config`;用量计价 `get/set_default_cost_multiplier`、`get/set_pricing_model_source`、`sync_session_usage`、`get_usage_data_sources`、`get_request_detail`;以及代理/状态/配置/供应商/故障转移/用量基础集与新策略命令 `get/set_load_balance_strategy`。未覆盖即面板抛错。
- **[阻断] 浏览器启动**:精简入口 `main.web.tsx` 不引入 `@tauri-apps/plugin-dialog`/`plugin-process`/`api/event` 等模块级 Tauri 导入,从根本规避 `main.tsx` 启动崩溃。
- **[重要] 并发与锁**:每个 dispatch 臂在 `.await` 前释放 `Mutex<Connection>` 守卫(复用既有命令体即满足);重型同步 DB 臂用 `spawn_blocking`;为 `/api/invoke` 设置 body 上限;建议开启 SQLite WAL + busy_timeout 缓解面板轮询与代理流量的锁竞争。
- **[重要] 参数键大小写**:SPA 经 Tauri 习惯发送 camelCase(`providerId`/`appType`/`pageSize`),但部分命令用 `app`(如 `get_providers`/`update_provider_weight` 用 `{app,id,weight}`);dispatch 反序列化必须按各命令实际键名。
- **[重要] get_providers 形状一致**:经真实 `AppState` 调与 `#[tauri::command]` 完全相同的 `ProviderService::list`,保证 `providers` 映射键与解析后权重逐字节一致。

### 5.4 涉及文件与依赖

- 新增:`src-tauri/src/web_panel/{mod,server,dispatch,assets}.rs`、`src/main.web.tsx`、`src/lib/api/transport.ts`、`src/lib/platform/isTauri.ts`。
- 改动:`src-tauri/src/lib.rs`(仅声明 `pub mod web_panel;`,不在 GUI 启动)、`cli/{mod,entry,server}.rs`、`database/dao/settings.rs`(`web_panel_port`)、`vite.config.ts`、`package.json`(`build:web`、`cross-env`)、`src/lib/api/providers.ts`(`updateTrayMenu`/`onSwitched` 守卫)、`src/lib/query/{proxy,queries}.ts`(Web 轮询间隔)。
- 依赖:`rust-embed = "8"`、`mime_guess = "2"`;`uuid`(已有)生成令牌。
- 测试:dispatch 代表性臂(含 `get_proxy_config_for_app` 经 settings 往返保 `weightRoundRobinEnabled`、`switch_proxy_provider` 拒官方供应商、`update_provider_weight` 写 `meta.routingWeight`+列);集成(临时端口起服 + `/api/invoke` 往返断言信封);安全(异源 403、缺/错令牌 401、非回环无令牌拒启);非致命绑定;关停可重绑;前端 `transport.ts` 单测;手动冒烟。

---

## 六、执行顺序与里程碑

1. **阶段一 — 负载均衡策略(核心,最高优先)**:自包含、可 `cargo test` 验证,直接修复用户反馈的 bug。
2. **阶段二 — CLI 输出美化**:纯展示、低风险、独立。
3. **阶段三 — CLI 专属 Web 控制台**:规模最大、安全敏感,依赖阶段一的策略命令,最后实施。

每阶段独立成 PR;阶段一/二可即时合并,阶段三需安全评审。

## 七、上游合并差距处理建议

本地 upstream 落后 GitHub `origin/main` 约 357 commit,且集中改动 `forwarder.rs`/`handlers.rs`(fork 分叉最重区域)。**建议本三阶段完成后再单独安排合并**(择优 cherry-pick 而非整体合并),以免在代理热路径产生大量冲突。`cc-switch-upstream` 两处未提交的 rustls(`ring`)修复须先提交或 stash,不得丢弃。

## 八、风险与回滚

- 阶段一:重命名 fan-out(grep 校验)、Frequency 计数器单增(精确序列测试守护)、权重语义反转(WARN + UI 文案);回滚=还原 `load_balancer.rs`/`provider_router.rs`/`types.rs` 及键值键(键缺失即默认 Frequency,无副作用)。
- 阶段二:依赖版本对齐(`cargo tree -d` 断言)、`has_cli_args` 路由(冒烟覆盖);回滚=还原 `output.rs` 及 `Cargo.toml` 两依赖。
- 阶段三:安全(强制令牌+自定义头)、生命周期(非致命绑定+守护透传)、脑裂(端口变更重绑);回滚=移除 `web_panel` 模块、CLI 标志与 `vite` web 目标,主程序不受影响。
- 通用:全程不删除/不间接丢失任何文件;所有配置使用相对路径。

---

## 九、实施记录(as-built)

> 本节记录实际落地状态与对设计的偏差,作为评审与回归依据。三阶段均通过 `cargo build`(lib+bins)、`cargo test --lib`(907 项全通过)与端到端冒烟。

### 9.1 阶段一(负载均衡策略)

- `proxy/load_balancer.rs`:`FrequencyControlledRR` → 枚举派发 `LoadBalancer` + `LoadBalanceStrategy{Frequency,WeightedRandom,HardRoundRobin}`;`select()` 单点自增 + 分派;WeightedRandom 用自包含 SplitMix64(无新依赖);15 项单测(含分布、确定性、零权重、轮转)。
- `proxy/mod.rs` 重导出更名;`provider_router.rs` 接入策略并入 `needs_update`(策略切换重建,新增 `test_strategy_switch_rebuilds_load_balancer`);`types.rs` `AppProxyConfig` 加 `load_balance_strategy`(只读);`dao/settings.rs` 键值持久化(无迁移);`dao/proxy.rs` 两分支只读填充、**不经通用 update 写入**(避免回写覆盖)。
- 命令 `get/set_load_balance_strategy`(commands/proxy.rs + lib.rs 注册);CLI `config lb --strategy` 并按策略条件渲染末列(频率/占比/轮转)+ 策略反转 WARN。
- 前端:`types/proxy.ts` + `proxy.ts` API + `WeightRoundRobinConfigPanel.tsx` 策略选择器(经专用命令写入)+ 指标列随策略切换;中/英/日 i18n。

### 9.2 阶段二(CLI 输出美化)

- `Cargo.toml` 提升 `anstream/anstyle = "1.0"`(对齐锁定版本,`cargo tree -d` 无重复)。
- `cli/output.rs` 经单一 `ColorMode` 门控(`anstream::AutoStream`),修复 `hint()` 裸 ANSI;`comfy-table` 同步 `force_no_tty`+ASCII 降级;CJK 显示宽度对齐;字形前导换行修正;新增 `raw_stdout`(export 纯净)、`warning_stderr`(导出前提示走 stderr)。
- 全局 `--color/--no-color`(`cli/mod.rs`),`entry.rs::has_cli_args` 跳过前导全局标志(修复路由冲突);`provider export -o -` 改 `raw_stdout`。
- 顺带修复 clap 4.6 暴露的 `provider add` 重复别名(debug panic)。
- 冒烟:管道 0 ANSI、`--color=always` 有 ANSI、表格降级 ASCII、`export -o - | jq` 通过。

### 9.3 阶段三(CLI 专属 Web 控制台)—— 架构偏差说明

实现时按"100% 复用官方前端"的要求,采用比原 5.1 更稳健的方案:

- **不新建 `main.web.tsx` 精简壳、不使用 Vite 别名替换整模块**(后者会破坏 `@tauri-apps/api/event` 对 core 的内部依赖)。改为**运行时传输切换**:新增 `src/lib/api/transport.ts`(`isTauri()` → 原生 IPC 或 `/api/invoke` fetch),并把 28 处 `import { invoke } from "@tauri-apps/api/core"` 统一重定向到该 shim(逐行替换,零界面改动)。同一份 `dist` 桌面/浏览器通用,`rust-embed` 直接嵌入 `../dist`,**无需单独 Web 构建**。
- 后端模块 `src-tauri/src/web_panel/{mod,server,dispatch,assets}.rs`:axum + 优雅关停;`POST /api/invoke/:command` 网关(与内容类型无关地解析 JSON body);SPA 回退。
- **安全**(已端到端验证 403/401/403):回环绑定 + 强制 Bearer 令牌(`~/.cc-switch/panel.token` 0600,经 `?token=` 一次性下发后清除地址栏)+ 自定义头 `X-CC-Switch-Panel` 强制预检 + Origin 白名单;绝不调用 `set_proxy_port`。
- **CLI 集成**:`ccs server start --web-port <P> [--web-bind]`;后台守护透传参数、令牌子进程生成、父进程读取后打印完整链接;端口持久化(`dao/settings.rs::get/set_web_panel_port`)供 `restart` 继承;绑定失败非致命;强制 `web_port ≠ proxy port`。
- **网关命令覆盖**(均复用同一后端逻辑,AppHandle 副作用以轮询替代):
  - 代理:status/running/takeover/start/stop/config(全局+应用)/switch;负载均衡策略 get/set。
  - 供应商:get/current/weight/add/update/delete/switch/sort/remove-from-live/universal CRUD+sync/opencode+openclaw live 导入。
  - 故障转移:queue 增删查/auto 开关/熔断器重置/health。
  - 用量仪表盘:summary/trends/provider-stats/model-stats/logs/detail/pricing CRUD/limits/sync/data-sources。
  - 全局出站代理:url get/set/test/upstream-status/scan;计费 cost/pricing-source;公共配置片段 get/set/extract;`update_global_proxy_config` 端口变更经 `ProxyService::update_config` 重绑定(避免脑裂)。
  - WebDAV 云备份:test/save-settings/fetch-remote/upload/download/sync-live;本地 DB 备份 create/list/restore/rename/delete;配置 export/import(服务端路径)。
  - 应用设置:get/save(空密码保持现有)。
  - 启动降级:`get_init_error`→null、`update_tray_menu`→no-op。
- **浏览器不支持(已记录)**:原生文件对话框 `pick_directory/save_file_dialog/open_file_dialog`、`open_external`/`open_config_folder`、托盘/开机自启/更新器、`app_config_dir` 覆盖等桌面专属命令。后果:对应按钮在浏览器报错但不致崩溃。
- **遗留增强(后续)**:① 整库导入/导出当前接收**服务端文件路径**,浏览器侧理想形态为 HTTP 上传/下载端点(需少量前端文件 I/O 适配);② 实时性当前依赖 react-query 轮询(5s),可后续加 SSE 加速 provider-switched;③ 上游 357 commit 合并仍建议本功能合并后单独处理。

### 9.4 运行方式

```
pnpm build:renderer            # 构建前端到 dist/(桌面与 Web 控制台共用)
cargo build -p cc-switch --bin cc-switch-cli
ccs server start --web-port 18080 --foreground   # 前台:直接打印带令牌访问链接
# 浏览器打开 http://127.0.0.1:18080/?token=<令牌>
```

