//! 扩展模块
//!
//! 包含分层轮询、负载均衡等增强功能的扩展模块

pub mod benchmark;
pub mod traffic_control;
pub mod url_selector;

pub use benchmark::BenchmarkManager;
pub use traffic_control::TrafficController;
pub use url_selector::UrlSelector;
