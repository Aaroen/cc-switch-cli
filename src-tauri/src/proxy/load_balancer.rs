//! 负载均衡器
//!
//! 实现频率控制轮询算法（Frequency-Controlled Round-Robin）
//!
//! 权重语义（反向频率控制）：
//! - weight=0: 禁用，永不使用
//! - weight=1: 每1轮使用1次 (100%频率)
//! - weight=2: 每2轮使用1次 (50%频率)
//! - weight=10: 每10轮使用1次 (10%频率)

use crate::provider::Provider;

/// 加权Provider
#[derive(Debug, Clone)]
pub struct WeightedProvider {
    pub provider: Provider,
    pub weight: u32, // 0-10, 0表示禁用
}

/// 频率控制轮询负载均衡器
///
/// 通过全局轮询计数器控制各Provider的使用频率
pub struct FrequencyControlledRR {
    providers: Vec<WeightedProvider>,
    global_round: u32, // 全局轮询计数器
}

impl FrequencyControlledRR {
    /// 创建新的负载均衡器
    pub fn new(providers: Vec<Provider>) -> Self {
        let weighted_providers = providers
            .into_iter()
            .map(|p| {
                // 从Provider对象读取weight字段（已从数据库加载）
                let weight = p.weight;

                WeightedProvider {
                    provider: p,
                    weight,
                }
            })
            .collect();

        Self {
            providers: weighted_providers,
            global_round: 0,
        }
    }

    /// 选择下一个Provider
    ///
    /// 算法逻辑：
    /// 1. global_round递增
    /// 2. 找到所有"到轮次"的Provider (global_round % weight == 0)
    /// 3. 优先选择weight最大的（低频优先，确保不会被weight=1长期压制）
    /// 4. 当多个Provider的weight相同且为最大值时，按轮次在同权重中轮转
    /// 5. 如果没有到轮次的，回退到weight=1的Provider
    ///
    /// 时间复杂度: O(n)
    pub fn select(&mut self) -> Option<&Provider> {
        if self.providers.is_empty() {
            return None;
        }

        // 递增全局轮询计数器
        self.global_round += 1;

        // 找到所有"到轮次"的Provider
        let eligible: Vec<&WeightedProvider> = self
            .providers
            .iter()
            .filter(|p| p.weight > 0 && self.global_round % p.weight == 0)
            .collect();

        if !eligible.is_empty() {
            // 有到轮次的，优先选择weight最大的（低频优先）
            let max_weight = eligible.iter().map(|p| p.weight).max().unwrap_or(1);

            // 同权重轮转（避免同权重供应商长期被固定顺序压制）
            let same_weight: Vec<&WeightedProvider> = eligible
                .into_iter()
                .filter(|p| p.weight == max_weight)
                .collect();

            let round_slot = (self.global_round / max_weight).max(1);
            let index = ((round_slot - 1) as usize) % same_weight.len();
            return Some(&same_weight[index].provider);
        }

        // 没有到轮次的，回退到weight=1的Provider（如果有）
        self.providers
            .iter()
            .find(|p| p.weight == 1)
            .map(|p| &p.provider)
    }

    /// 重置全局计数器
    pub fn reset_counter(&mut self) {
        self.global_round = 0;
    }

    /// 获取当前全局轮询计数
    pub fn current_round(&self) -> u32 {
        self.global_round
    }

    /// 获取Provider列表
    pub fn providers(&self) -> &[WeightedProvider] {
        &self.providers
    }

    /// 获取Provider数量
    pub fn len(&self) -> usize {
        self.providers.len()
    }

    /// 检查是否为空
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    /// 更新单个Provider的权重
    pub fn update_weight(&mut self, provider_id: &str, weight: u32) -> bool {
        if let Some(p) = self
            .providers
            .iter_mut()
            .find(|p| p.provider.id == provider_id)
        {
            p.weight = weight;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_provider(id: &str, weight: u32) -> Provider {
        Provider {
            id: id.to_string(),
            name: id.to_string(),
            settings_config: serde_json::json!({}),
            website_url: None,
            category: None,
            created_at: None,
            sort_index: Some(0),
            notes: None,
            meta: None,
            icon: None,
            icon_color: None,
            in_failover_queue: true,
            weight,
        }
    }

    fn create_weighted_provider(id: &str, weight: u32) -> WeightedProvider {
        WeightedProvider {
            provider: create_test_provider(id, weight),
            weight,
        }
    }

    #[test]
    fn test_frequency_controlled_rr_basic() {
        let providers = vec![
            create_weighted_provider("A", 1),
            create_weighted_provider("B", 2),
            create_weighted_provider("C", 3),
        ];

        let mut lb = FrequencyControlledRR {
            providers,
            global_round: 0,
        };

        // Round 1: A(1%1=0✓), B(1%2=1), C(1%3=1) -> 选A
        assert_eq!(lb.select().unwrap().id, "A");
        assert_eq!(lb.current_round(), 1);

        // Round 2: A(2%1=0✓), B(2%2=0✓), C(2%3=2) -> 选B (weight最大)
        assert_eq!(lb.select().unwrap().id, "B");
        assert_eq!(lb.current_round(), 2);

        // Round 3: A(3%1=0✓), B(3%2=1), C(3%3=0✓) -> 选C (weight最大)
        assert_eq!(lb.select().unwrap().id, "C");
        assert_eq!(lb.current_round(), 3);

        // Round 4: A(4%1=0✓), B(4%2=0✓), C(4%3=1) -> 选B
        assert_eq!(lb.select().unwrap().id, "B");

        // Round 5: A(5%1=0✓), B(5%2=1), C(5%3=2) -> 选A
        assert_eq!(lb.select().unwrap().id, "A");

        // Round 6: A(6%1=0✓), B(6%2=0✓), C(6%3=0✓) -> 选C (weight最大)
        assert_eq!(lb.select().unwrap().id, "C");
    }

    #[test]
    fn test_frequency_controlled_rr_no_weight_1() {
        // 测试没有weight=1的情况
        let providers = vec![
            create_weighted_provider("B", 2),
            create_weighted_provider("C", 3),
        ];

        let mut lb = FrequencyControlledRR {
            providers,
            global_round: 0,
        };

        // Round 1: 都不到轮次，且没有weight=1 -> None
        assert!(lb.select().is_none());

        // Round 2: B到轮次
        assert_eq!(lb.select().unwrap().id, "B");

        // Round 3: C到轮次
        assert_eq!(lb.select().unwrap().id, "C");

        // Round 4: B到轮次
        assert_eq!(lb.select().unwrap().id, "B");
    }

    #[test]
    fn test_frequency_controlled_rr_weight_0() {
        // 测试weight=0（禁用）
        let providers = vec![
            create_weighted_provider("A", 1),
            create_weighted_provider("B", 0), // 禁用
        ];

        let mut lb = FrequencyControlledRR {
            providers,
            global_round: 0,
        };

        // 所有轮次都应该选A，B被禁用
        for _ in 0..10 {
            assert_eq!(lb.select().unwrap().id, "A");
        }
    }

    #[test]
    fn test_reset_counter() {
        let providers = vec![create_weighted_provider("A", 1)];

        let mut lb = FrequencyControlledRR {
            providers,
            global_round: 0,
        };

        lb.select(); // Round 1
        lb.select(); // Round 2
        assert_eq!(lb.current_round(), 2);

        lb.reset_counter();
        assert_eq!(lb.current_round(), 0);
    }

    #[test]
    fn test_update_weight() {
        let providers = vec![create_weighted_provider("A", 1)];

        let mut lb = FrequencyControlledRR {
            providers,
            global_round: 0,
        };

        assert_eq!(lb.providers[0].weight, 1);

        // 更新权重
        assert!(lb.update_weight("A", 5));
        assert_eq!(lb.providers[0].weight, 5);

        // 更新不存在的Provider
        assert!(!lb.update_weight("B", 5));
    }

    #[test]
    fn test_frequency_distribution() {
        // 验证实际频率分布
        let providers = vec![
            create_weighted_provider("Fast", 1),   // 高频
            create_weighted_provider("Medium", 2), // 中频
            create_weighted_provider("Slow", 5),   // 低频
        ];

        let mut lb = FrequencyControlledRR {
            providers,
            global_round: 0,
        };

        let mut counts = std::collections::HashMap::new();

        // 运行10轮
        for _ in 0..10 {
            if let Some(p) = lb.select() {
                *counts.entry(p.id.clone()).or_insert(0) += 1;
            }
        }

        // 期望分布：Fast=4, Medium=4, Slow=2（Round 5/10 由 Slow 选中）
        assert_eq!(counts.get("Fast"), Some(&4));
        assert_eq!(counts.get("Medium"), Some(&4));
        assert_eq!(counts.get("Slow"), Some(&2));
    }

    #[test]
    fn test_frequency_controlled_rr_tie_breaker() {
        let providers = vec![
            create_weighted_provider("A", 10),
            create_weighted_provider("B", 10),
        ];

        let mut lb = FrequencyControlledRR {
            providers,
            global_round: 0,
        };

        let mut picks = Vec::new();
        for _ in 0..40 {
            let p_id = lb.select().map(|p| p.id.clone());
            let round = lb.current_round();
            if round % 10 == 0 {
                assert!(p_id.is_some());
                picks.push(p_id.unwrap());
            }
        }

        assert_eq!(picks, vec!["A", "B", "A", "B"]);
    }
}
