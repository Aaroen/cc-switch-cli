//! CLI模块 - 纯终端控制接口
//!
//! 提供完整的命令行接口，支持所有GUI功能的终端控制

pub mod commands;
pub mod output;
pub mod server;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "cc-switch")]
#[command(about = "Claude Code / Codex / Gemini CLI 统一管理工具", long_about = None)]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// 服务器管理（启动/停止/状态）
    #[command(subcommand, alias = "srv")]
    Server(ServerCommands),

    /// 供应商管理（增删改查/权重设置）
    #[command(subcommand, alias = "p")]
    Provider(ProviderCommands),

    /// 配置管理（查看/修改配置）
    #[command(subcommand, alias = "cfg")]
    Config(ConfigCommands),

    /// 故障转移管理（队列管理/熔断器）
    #[command(subcommand, alias = "fo")]
    Failover(FailoverCommands),

    /// 统计信息（用量/请求日志）
    #[command(subcommand, alias = "st")]
    Stats(StatsCommands),

    /// MCP服务器管理
    #[command(subcommand, alias = "m")]
    Mcp(McpCommands),

    /// 提示词管理
    #[command(subcommand, alias = "pr")]
    Prompt(PromptCommands),

    /// 技能管理
    #[command(subcommand, alias = "sk")]
    Skill(SkillCommands),
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

        /// 后台运行
        #[arg(short, long)]
        daemon: bool,
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
    #[command(alias = "add")]
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

        /// 权重值（0-10，0=禁用，1=每轮使用）
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

    /// 测试供应商连接
    #[command(alias = "test")]
    Test {
        /// 应用类型
        #[arg(short, long)]
        app: String,

        /// 供应商ID
        #[arg(short, long)]
        id: String,
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

    /// 导出配置到文件
    Export {
        /// 输出文件路径
        #[arg(short, long)]
        output: String,
    },

    /// 从文件导入配置
    Import {
        /// 输入文件路径
        #[arg(short, long)]
        input: String,
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

    /// 查看熔断器状态
    CircuitBreaker {
        /// 应用类型
        #[arg(short, long)]
        app: String,

        /// 供应商ID（可选）
        #[arg(short, long)]
        id: Option<String>,
    },

    /// 重置熔断器
    Reset {
        /// 应用类型
        #[arg(short, long)]
        app: String,

        /// 供应商ID
        #[arg(short, long)]
        id: String,
    },
}

#[derive(Subcommand)]
pub enum StatsCommands {
    /// 查看用量摘要
    Summary {
        /// 时间范围（天数）
        #[arg(short, long, default_value = "7")]
        days: u32,

        /// 应用类型（可选）
        #[arg(short, long)]
        app: Option<String>,
    },

    /// 查看供应商统计
    Provider {
        /// 应用类型
        #[arg(short, long)]
        app: String,

        /// 供应商ID（可选）
        #[arg(short, long)]
        id: Option<String>,

        /// 时间范围（天数）
        #[arg(short, long, default_value = "7")]
        days: u32,
    },

    /// 查看模型统计
    Model {
        /// 时间范围（天数）
        #[arg(short, long, default_value = "7")]
        days: u32,
    },

    /// 查看请求日志
    Logs {
        /// 限制条数
        #[arg(short, long, default_value = "50")]
        limit: u32,

        /// 应用类型（可选）
        #[arg(short, long)]
        app: Option<String>,

        /// 供应商ID（可选）
        #[arg(short = 'p', long)]
        provider: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum McpCommands {
    /// 列出所有MCP服务器
    List {
        /// 应用类型（可选）
        #[arg(short, long)]
        app: Option<String>,
    },

    /// 添加MCP服务器
    Add {
        /// 服务器名称
        #[arg(short, long)]
        name: String,

        /// 命令
        #[arg(short, long)]
        command: String,

        /// 参数（多个）
        #[arg(short = 'r', long)]
        args: Vec<String>,

        /// 启用的应用（多个，如：claude,codex）
        #[arg(short = 'e', long)]
        enabled: Vec<String>,
    },

    /// 删除MCP服务器
    Remove {
        /// 服务器名称
        #[arg(short, long)]
        name: String,
    },

    /// 启用/禁用MCP服务器
    Toggle {
        /// 服务器名称
        #[arg(short, long)]
        name: String,

        /// 应用类型
        #[arg(short, long)]
        app: String,

        /// 启用状态
        #[arg(short, long)]
        enabled: bool,
    },
}

#[derive(Subcommand)]
pub enum PromptCommands {
    /// 列出所有提示词
    List {
        /// 应用类型（可选）
        #[arg(short, long)]
        app: Option<String>,
    },

    /// 添加提示词
    Add {
        /// 提示词名称
        #[arg(short, long)]
        name: String,

        /// 提示词内容
        #[arg(short, long)]
        content: String,

        /// 应用类型
        #[arg(short, long)]
        app: String,
    },

    /// 删除提示词
    Remove {
        /// 提示词名称
        #[arg(short, long)]
        name: String,

        /// 应用类型
        #[arg(short, long)]
        app: String,
    },

    /// 查看提示词内容
    Show {
        /// 提示词名称
        #[arg(short, long)]
        name: String,

        /// 应用类型
        #[arg(short, long)]
        app: String,
    },
}

#[derive(Subcommand)]
pub enum SkillCommands {
    /// 列出已安装的技能
    List {
        /// 应用类型（可选）
        #[arg(short, long)]
        app: Option<String>,
    },

    /// 安装技能
    Install {
        /// 技能ID
        #[arg(short, long)]
        id: String,

        /// 应用类型（多个）
        #[arg(short, long)]
        apps: Vec<String>,
    },

    /// 卸载技能
    Uninstall {
        /// 技能ID
        #[arg(short, long)]
        id: String,

        /// 应用类型（可选，不指定则从所有应用卸载）
        #[arg(short, long)]
        app: Option<String>,
    },

    /// 发现可用技能
    Discover,
}
