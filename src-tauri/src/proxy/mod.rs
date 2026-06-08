//! 代理服务器模块
//!
//! 提供本地HTTP代理服务，支持多Provider故障转移和请求透传

pub mod body_filter;
pub mod cache_injector;
pub mod circuit_breaker;
pub mod copilot_optimizer;
pub mod error;
pub mod error_mapper;
#[allow(dead_code)] // 分层转发器扩展：合并官方 forwarder 后暂未接线，保留待后续集成
pub mod extensions; // 【新增】扩展模块
pub(crate) mod failover_switch;
#[allow(dead_code)] // 转发路径文件日志：合并官方 forwarder 后暂未接线，保留待后续集成
pub mod file_logger; // 【新增】文件日志模块
mod forwarder;
pub mod gemini_url;
pub mod handler_config;
pub mod handler_context;
mod handlers;
mod health;
#[allow(dead_code)] // 含 fork 出站代理兜底辅助：合并官方 forwarder 后暂未接线，保留待后续集成
pub mod http_client;
pub mod hyper_client;
<<<<<<< HEAD
pub(crate) mod json_canonical;
=======
pub mod layered_forwarder; // 【新增】分层转发器
>>>>>>> origin/cc-switch-cli
pub mod load_balancer; // 【新增】负载均衡器
pub mod log_codes;
pub mod media_sanitizer;
pub mod model_mapper;
pub mod provider_router;
pub mod providers;
pub mod response_handler;
pub mod response_processor;
pub(crate) mod server;
pub mod session;
pub(crate) mod sse;
pub(crate) mod switch_lock;
pub mod thinking_budget_rectifier;
pub mod thinking_optimizer;
pub mod thinking_rectifier;
pub(crate) mod types;
pub mod usage;

// 公开导出给外部使用（commands, services等模块需要）
#[allow(unused_imports)]
pub use circuit_breaker::{
    CircuitBreaker, CircuitBreakerConfig, CircuitBreakerStats, CircuitState,
};
#[allow(unused_imports)]
pub use error::ProxyError;
#[allow(unused_imports)]
pub use extensions::{BenchmarkManager, TrafficController, UrlSelector}; // 【新增】扩展模块导出
#[allow(unused_imports)]
pub use file_logger::{get_file_logger, FileLogger}; // 【新增】文件日志器导出
#[allow(unused_imports)]
pub use load_balancer::{LoadBalanceStrategy, LoadBalancer, WeightedProvider}; // 【新增】负载均衡器导出（多策略）
#[allow(unused_imports)]
pub use provider_router::ProviderRouter;
#[allow(unused_imports)]
pub use response_handler::{NonStreamHandler, ResponseType, StreamHandler};
#[allow(unused_imports)]
pub use session::{
    extract_session_id, ClientFormat, ProxySession, SessionIdResult, SessionIdSource,
};
#[allow(unused_imports)]
pub use types::{ProxyConfig, ProxyServerInfo, ProxyStatus};

// 内部模块间共享（供子模块使用）
// 注意：这个导出用于模块内部，编译器可能警告未使用但实际被子模块使用
#[allow(unused_imports)]
pub(crate) use types::*;
