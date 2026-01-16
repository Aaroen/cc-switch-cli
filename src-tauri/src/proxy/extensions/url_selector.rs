//! URL智能选择器
//!
//! 负责URL延迟缓存、智能选择和失效检测

use crate::provider::Provider;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// URL延迟数据
#[derive(Debug, Clone)]
pub struct UrlLatency {
    pub url: String,
    pub latency_ms: u64,
    pub measured_at: Instant,
    pub is_suspect: bool,
    pub consecutive_fails: u32,
}

/// URL智能选择器
pub struct UrlSelector {
    /// URL延迟缓存 (key: url, value: UrlLatency)
    url_latencies: Arc<RwLock<HashMap<String, UrlLatency>>>,
    /// 最大缓存大小
    max_cache_size: usize,
}

impl UrlSelector {
    /// 创建新的URL选择器
    pub fn new() -> Self {
        Self {
            url_latencies: Arc::new(RwLock::new(HashMap::new())),
            max_cache_size: 1000,
        }
    }

    /// 优化Provider的URL列表（按延迟排序）
    pub async fn optimize_urls(&self, _provider: &mut Provider) {
        // TODO: 实现URL优化逻辑
        // 1. 从缓存中获取各URL的延迟
        // 2. 过滤掉suspect状态的URL
        // 3. 按延迟排序
    }

    /// 记录URL延迟
    pub async fn record_latency(&self, url: String, latency_ms: u64) {
        let mut cache = self.url_latencies.write().await;

        // LRU淘汰策略
        if cache.len() >= self.max_cache_size {
            let cutoff = Instant::now() - Duration::from_secs(3600);
            cache.retain(|_, v| v.measured_at > cutoff);
        }

        cache.insert(
            url.clone(),
            UrlLatency {
                url,
                latency_ms,
                measured_at: Instant::now(),
                is_suspect: false,
                consecutive_fails: 0,
            },
        );
    }

    /// 标记URL为疑似失效
    pub async fn mark_suspect(&self, url: &str) {
        let mut cache = self.url_latencies.write().await;
        if let Some(latency) = cache.get_mut(url) {
            latency.consecutive_fails += 1;
            if latency.consecutive_fails >= 3 {
                latency.is_suspect = true;
            }
        }
    }

    /// 重置URL状态（成功后调用）
    pub async fn reset_url(&self, url: &str) {
        let mut cache = self.url_latencies.write().await;
        if let Some(latency) = cache.get_mut(url) {
            latency.is_suspect = false;
            latency.consecutive_fails = 0;
        }
    }
}

impl Default for UrlSelector {
    fn default() -> Self {
        Self::new()
    }
}
