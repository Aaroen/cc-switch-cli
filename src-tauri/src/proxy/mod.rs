//! 代理服务器模块
//!
//! 提供本地HTTP代理服务，支持多Provider故障转移和请求透传

pub mod body_filter;
pub mod circuit_breaker;
pub mod error;
pub mod error_mapper;
pub mod extensions; // 【新增】扩展模块
pub(crate) mod failover_switch;
pub mod file_logger; // 【新增】文件日志模块
mod forwarder;
pub mod handler_config;
pub mod handler_context;
mod handlers;
mod health;
pub mod http_client;
pub mod layered_forwarder; // 【新增】分层转发器
pub mod load_balancer; // 【新增】负载均衡器
pub mod log_codes;
pub mod model_mapper;
pub mod provider_router;
pub mod providers;
pub mod response_handler;
pub mod response_processor;
pub(crate) mod server;
pub mod session;
pub mod thinking_budget_rectifier;
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
pub use layered_forwarder::LayeredForwarder; // 【新增】分层转发器导出
#[allow(unused_imports)]
pub use load_balancer::{FrequencyControlledRR, WeightedProvider}; // 【新增】负载均衡器导出（频率控制）
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
