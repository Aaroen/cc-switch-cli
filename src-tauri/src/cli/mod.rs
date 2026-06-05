//! CLI模块 - 纯终端控制接口
//!
//! 提供完整的命令行接口，支持所有GUI功能的终端控制

pub mod commands;
mod entry;
pub mod headless_log;
pub mod output;
pub mod server;

use clap::{Parser, Subcommand, ValueEnum};

pub use entry::{has_cli_args, run_from_env};

/// 彩色输出控制
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default, ValueEnum)]
pub enum ColorWhen {
    /// 自动检测（终端着色，管道/NO_COLOR 时关闭）
    #[default]
    Auto,
    /// 始终着色
    Always,
    /// 从不着色
    Never,
}

#[derive(Parser)]
#[command(name = "ccs")]
#[command(bin_name = "ccs")]
#[command(about = "Claude Code / Codex / Gemini CLI 统一管理工具", long_about = None)]
#[command(version)]
pub struct Cli {
    /// 控制彩色输出：auto（默认）/ always / never
    #[arg(long, value_enum, default_value_t = ColorWhen::Auto, global = true)]
    pub color: ColorWhen,

    /// 关闭彩色输出（等价于 --color never）
    #[arg(long, global = true)]
    pub no_color: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// 服务器管理（启动/停止/状态）
    #[command(subcommand, alias = "srv")]
    Server(ServerCommands),

    /// 启动代理服务器（`server start` 的简写）
    Start {
        /// 监听端口（默认15721）
        #[arg(short, long, default_value = "15721")]
        port: u16,

        /// 监听地址（默认127.0.0.1）
        #[arg(short = 'H', long, default_value = "127.0.0.1")]
        host: String,

        /// 前台运行（默认后台启动；需要查看实时输出/调试时使用）
        #[arg(short = 'f', long)]
        foreground: bool,

        /// 后台运行（兼容旧参数；当前已默认后台启动）
        #[arg(short, long, hide = true, conflicts_with = "foreground")]
        daemon: bool,

        /// 同时启动 Web 控制台并监听该端口（不指定则读取已持久化端口，未配置则不启动）
        #[arg(long)]
        web_port: Option<u16>,

        /// Web 控制台监听地址（默认 0.0.0.0，允许局域网访问；首次访问需设置密码）
        #[arg(long, default_value = "0.0.0.0")]
        web_bind: String,
    },

    /// 停止代理服务器（`server stop` 的简写）
    Stop,

    /// 查看服务器状态（`server status` 的简写）
    Status,

    /// 重启服务器（`server restart` 的简写）
    Restart {
        /// 监听端口
        #[arg(short, long, default_value = "15721")]
        port: u16,
    },

    /// 供应商管理（增删改查/权重设置）
    #[command(subcommand, alias = "p")]
    Provider(ProviderCommands),

    /// 配置管理（查看/修改配置）
    #[command(subcommand, alias = "cfg")]
    Config(ConfigCommands),

    /// 故障转移管理（队列管理）
    #[command(subcommand, alias = "fo")]
    Failover(FailoverCommands),
}

#[derive(Subcommand)]
pub enum ServerCommands {
    /// 启动代理服务器（无头模式）
    Start {
        /// 监听端口（默认15721）
        #[arg(short, long, default_value = "15721")]
        port: u16,

        /// 监听地址（默认127.0.0.1）
        #[arg(short = 'H', long, default_value = "127.0.0.1")]
        host: String,

        /// 前台运行（默认后台启动；需要查看实时输出/调试时使用）
        #[arg(short = 'f', long)]
        foreground: bool,

        /// 后台运行（兼容旧参数；当前已默认后台启动）
        #[arg(short, long, hide = true, conflicts_with = "foreground")]
        daemon: bool,

        /// 同时启动 Web 控制台并监听该端口（不指定则读取已持久化端口，未配置则不启动）
        #[arg(long)]
        web_port: Option<u16>,

        /// Web 控制台监听地址（默认 0.0.0.0，允许局域网访问；首次访问需设置密码）
        #[arg(long, default_value = "0.0.0.0")]
        web_bind: String,
    },

    /// 停止代理服务器
    Stop,

    /// 查看服务器状态
    Status,

    /// 重启服务器
    Restart {
        /// 监听端口
        #[arg(short, long, default_value = "15721")]
        port: u16,
    },
}

#[derive(Subcommand)]
pub enum ProviderCommands {
    /// 列出所有供应商
    #[command(alias = "ls")]
    List {
        /// 应用类型（claude/codex/gemini）
        #[arg(short, long)]
        app: String,

        /// 显示详细信息
        #[arg(short, long)]
        verbose: bool,
    },

    /// 添加供应商
    Add {
        /// 应用类型
        #[arg(short, long)]
        app: String,

        /// 供应商名称
        #[arg(short, long)]
        name: String,

        /// API Key
        #[arg(short, long)]
        key: Option<String>,

        /// Base URL
        #[arg(short, long)]
        url: Option<String>,

        /// JSON配置文件路径
        #[arg(short, long)]
        file: Option<String>,
    },

    /// 删除供应商
    #[command(alias = "rm")]
    Remove {
        /// 应用类型
        #[arg(short, long)]
        app: String,

        /// 供应商ID
        #[arg(short, long)]
        id: String,
    },

    /// 切换当前供应商
    #[command(alias = "sw")]
    Switch {
        /// 应用类型
        #[arg(short, long)]
        app: String,

        /// 供应商ID
        #[arg(short, long)]
        id: String,
    },

    /// 设置供应商权重
    #[command(alias = "wt")]
    Weight {
        /// 应用类型
        #[arg(short, long)]
        app: String,

        /// 供应商ID
        #[arg(short, long)]
        id: String,

        /// 权重值（0-100，0=禁用，N=按 1/N 参与轮询槽位）
        #[arg(short, long)]
        weight: u32,
    },

    /// 设置供应商模型映射（单条）
    #[command(alias = "map")]
    ModelMap {
        /// 应用类型
        #[arg(short, long)]
        app: String,

        /// 供应商ID
        #[arg(short, long)]
        id: String,

        /// 原始模型名
        #[arg(long)]
        from: String,

        /// 映射模型名
        #[arg(long)]
        to: String,
    },

    /// 设置供应商 env 配置
    #[command(alias = "env")]
    EnvSet {
        /// 应用类型
        #[arg(short, long)]
        app: String,

        /// 供应商ID
        #[arg(short, long)]
        id: String,

        /// 配置键（例如 ANTHROPIC_DEFAULT_SONNET_MODEL）
        #[arg(long)]
        key: String,

        /// 配置值
        #[arg(long)]
        value: String,
    },

    /// 管理供应商超参数（settings_config 任意 JSON 路径）
    #[command(subcommand, alias = "hp", alias = "param", alias = "params")]
    Hyperparams(HyperparamsCommands),

    /// 查看供应商详情
    #[command(alias = "info")]
    Show {
        /// 应用类型
        #[arg(short, long)]
        app: String,

        /// 供应商ID
        #[arg(short, long)]
        id: String,
    },

    /// 导出供应商（用于备份/迁移）
    #[command(alias = "exp")]
    Export {
        /// 应用类型（claude/codex/gemini）
        #[arg(short, long)]
        app: String,

        /// 输出文件路径（可用 "-" 输出到 stdout）
        #[arg(short, long)]
        output: String,

        /// 仅导出指定供应商ID（不指定则导出全部）
        #[arg(short, long)]
        id: Option<String>,

        /// 脱敏导出（将疑似密钥字段替换为 "***"；导入后不可直接使用）
        #[arg(long, default_value_t = false)]
        redact: bool,
    },

    /// 导入供应商（从 Export 的 JSON 文件）
    #[command(alias = "imp")]
    Import {
        /// 应用类型（claude/codex/gemini）
        #[arg(short, long)]
        app: String,

        /// 输入文件路径（可用 "-" 从 stdin 读取）
        #[arg(short, long)]
        input: String,

        /// 覆盖同 ID 供应商（默认跳过同 ID）
        #[arg(long, default_value_t = false)]
        overwrite: bool,

        /// 导入时为每个供应商生成新 ID（避免与现有冲突）
        #[arg(long, default_value_t = false)]
        new_ids: bool,

        /// 导入后切换到导出文件记录的 current 供应商（若存在）
        #[arg(long, default_value_t = true)]
        set_current: bool,
    },

    /// 更新供应商配置（常用：修改 key/base_url/完整 JSON 配置）
    #[command(alias = "upd")]
    Update {
        /// 应用类型（claude/codex/gemini）
        #[arg(short, long)]
        app: String,

        /// 供应商ID
        #[arg(short, long)]
        id: String,

        /// JSON配置文件路径（同 add --file；可用 "-" 从 stdin 读取）
        #[arg(short, long)]
        file: Option<String>,

        /// 直接设置 API Key（会写入 settings_config）
        #[arg(short, long)]
        key: Option<String>,

        /// 直接设置 Base URL（会写入 settings_config）
        #[arg(short, long)]
        url: Option<String>,

        /// 替换整个 settings_config（默认：merge 合并）
        #[arg(long, default_value_t = false)]
        replace: bool,

        /// 同时更新供应商名称（可选）
        #[arg(long)]
        name: Option<String>,

        /// 同时更新备注（可选）
        #[arg(long)]
        notes: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum HyperparamsCommands {
    /// 查看超参数（可指定单一路径）
    #[command(alias = "get")]
    Show {
        /// 应用类型（claude/codex/gemini/opencode/openclaw）
        #[arg(short, long)]
        app: String,

        /// 供应商ID
        #[arg(short, long)]
        id: String,

        /// JSON 路径（如 agents.sisyphus.temperature）
        #[arg(long)]
        path: Option<String>,
    },

    /// 设置超参数（支持 JSON 值或纯字符串）
    #[command(alias = "set")]
    Set {
        /// 应用类型（claude/codex/gemini/opencode/openclaw）
        #[arg(short, long)]
        app: String,

        /// 供应商ID
        #[arg(short, long)]
        id: String,

        /// JSON 路径（如 agents.sisyphus.temperature）
        #[arg(long)]
        path: String,

        /// JSON 值（如 0.5、true、{"edit":"allow"}）
        #[arg(long, required_unless_present = "value", conflicts_with = "value")]
        json: Option<String>,

        /// 纯字符串值（等价于 JSON 字符串）
        #[arg(long, required_unless_present = "json", conflicts_with = "json")]
        value: Option<String>,
    },

    /// 删除超参数
    #[command(alias = "rm", alias = "del", alias = "unset")]
    Remove {
        /// 应用类型（claude/codex/gemini/opencode/openclaw）
        #[arg(short, long)]
        app: String,

        /// 供应商ID
        #[arg(short, long)]
        id: String,

        /// JSON 路径（如 agents.sisyphus.temperature）
        #[arg(long)]
        path: String,
    },
}

#[derive(Subcommand)]
pub enum ConfigCommands {
    /// 查看配置
    Show {
        /// 应用类型（可选，不指定则显示全局配置）
        #[arg(short, long)]
        app: Option<String>,
    },

    /// 设置配置项
    Set {
        /// 配置键
        #[arg(short, long)]
        key: String,

        /// 配置值
        #[arg(short, long)]
        value: String,

        /// 应用类型（可选）
        #[arg(short, long)]
        app: Option<String>,
    },

    /// 查看代理配置
    Proxy {
        /// 应用类型（可选）
        #[arg(short, long)]
        app: Option<String>,
    },

    /// 权重轮询配置（负载均衡）
    #[command(alias = "lb")]
    Loadbalance {
        /// 应用类型
        #[arg(short, long)]
        app: String,

        /// 启用/禁用权重轮询（不指定则显示当前状态）
        #[arg(short, long)]
        enabled: Option<bool>,

        /// 负载均衡策略：frequency / weighted_random / hard_round_robin
        #[arg(short, long)]
        strategy: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum FailoverCommands {
    /// 查看故障转移队列
    Queue {
        /// 应用类型
        #[arg(short, long)]
        app: String,
    },

    /// 添加供应商到队列
    Add {
        /// 应用类型
        #[arg(short, long)]
        app: String,

        /// 供应商ID
        #[arg(short, long)]
        id: String,
    },

    /// 从队列移除供应商
    Remove {
        /// 应用类型
        #[arg(short, long)]
        app: String,

        /// 供应商ID
        #[arg(short, long)]
        id: String,
    },

    /// 启用/禁用自动故障转移
    Toggle {
        /// 应用类型
        #[arg(short, long)]
        app: String,

        /// 启用状态
        #[arg(short, long)]
        enabled: bool,
    },
}
