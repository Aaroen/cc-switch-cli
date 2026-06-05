//! 负载均衡器
//!
//! 提供可插拔的多策略供应商选择：
//! - Frequency（频率控制）：
//!     weight=0 禁用；weight=N 表示频率 1/N。实现上先按频率换算供应商在轮询周期
//!     中的槽位次数，再在这些槽位上执行固定顺序轮询。
//! - WeightedRandom（加权随机）：被选中概率 = weight_i / Σweight。
//! - HardRoundRobin（硬全轮询）：在 weight>0 的供应商间等概率轮转，忽略权重大小。

use crate::provider::Provider;

const FREQUENCY_EXACT_SCALE_CAP: u32 = 10_000;

/// 负载均衡策略
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoadBalanceStrategy {
    /// 频率控制轮询（按 1/N 换算槽位次数后轮询）
    #[default]
    Frequency,
    /// 加权随机（p_i = weight_i / Σweight）
    WeightedRandom,
    /// 硬全轮询（等概率轮转 weight>0 的供应商，忽略权重大小）
    HardRoundRobin,
}

impl LoadBalanceStrategy {
    /// 规范化字符串（用于持久化与展示）
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Frequency => "frequency",
            Self::WeightedRandom => "weighted_random",
            Self::HardRoundRobin => "hard_round_robin",
        }
    }
}

impl std::str::FromStr for LoadBalanceStrategy {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        match s.trim().to_ascii_lowercase().as_str() {
            "frequency" => Ok(Self::Frequency),
            "weighted_random" | "weightedrandom" | "random" => Ok(Self::WeightedRandom),
            "hard_round_robin" | "hardroundrobin" | "roundrobin" | "rr" => Ok(Self::HardRoundRobin),
            _ => Err(()),
        }
    }
}

/// 加权Provider
#[derive(Debug, Clone)]
pub struct WeightedProvider {
    pub provider: Provider,
    pub weight: u32, // 0-100, 0表示禁用
}

/// 简易 FNV-1a 哈希。
///
/// 用于从供应商 id 派生每个均衡器实例的初始 RNG 种子，使不同实例（不同 app）
/// 的随机序列彼此独立。测试可通过显式设置 `rng_state` 覆盖该种子以保证确定性。
fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// 多策略负载均衡器
///
/// 通过全局轮询计数器与策略枚举控制供应商选择。
/// `select()` 为同步方法，可在 `load_balancers` 写锁临界区内安全调用（无 `.await`）。
pub struct LoadBalancer {
    strategy: LoadBalanceStrategy,
    providers: Vec<WeightedProvider>,
    frequency_order: Vec<usize>, // Frequency 策略的虚拟轮询槽位（provider 下标）
    global_round: u32,           // 全局轮询计数器
    rng_state: u64,              // WeightedRandom 用的 SplitMix64 状态
}

impl LoadBalancer {
    /// 创建新的负载均衡器
    pub fn new(providers: Vec<Provider>, strategy: LoadBalanceStrategy) -> Self {
        let weighted_providers: Vec<WeightedProvider> = providers
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

        // 从供应商 id 派生与实例相关的种子，使各实例随机序列相互独立。
        let seed = weighted_providers
            .iter()
            .fold(0x9E37_79B9_7F4A_7C15u64, |acc, wp| {
                acc.rotate_left(5) ^ fnv1a(&wp.provider.id)
            });

        let frequency_order = Self::build_frequency_order(&weighted_providers);

        Self {
            strategy,
            providers: weighted_providers,
            frequency_order,
            global_round: 0,
            rng_state: seed,
        }
    }

    fn gcd(mut a: u32, mut b: u32) -> u32 {
        while b != 0 {
            let r = a % b;
            a = b;
            b = r;
        }
        a.max(1)
    }

    fn frequency_scale(providers: &[WeightedProvider]) -> u32 {
        let mut scale = 1u32;
        for weight in providers.iter().filter(|p| p.weight > 0).map(|p| p.weight) {
            let gcd = Self::gcd(scale, weight);
            let next = (scale / gcd) as u64 * weight as u64;
            if next > FREQUENCY_EXACT_SCALE_CAP as u64 {
                return FREQUENCY_EXACT_SCALE_CAP;
            }
            scale = next as u32;
        }
        scale
    }

    fn build_frequency_order(providers: &[WeightedProvider]) -> Vec<usize> {
        let scale = Self::frequency_scale(providers);
        let slots: Vec<(usize, u32)> = providers
            .iter()
            .enumerate()
            .filter_map(|(idx, provider)| {
                if provider.weight == 0 {
                    None
                } else {
                    Some((idx, (scale / provider.weight).max(1)))
                }
            })
            .collect();

        let total_slots: usize = slots.iter().map(|(_, count)| *count as usize).sum();
        if total_slots == 0 {
            return Vec::new();
        }

        let max_slots = slots.iter().map(|(_, count)| *count).max().unwrap_or(0);
        let mut order = Vec::with_capacity(total_slots);
        for pass in 0..max_slots {
            for (idx, count) in &slots {
                if pass < *count {
                    order.push(*idx);
                }
            }
        }
        order
    }

    /// 当前策略
    pub fn strategy(&self) -> LoadBalanceStrategy {
        self.strategy
    }

    /// 选择下一个 Provider（按当前策略分派）
    ///
    /// 重要：本方法为同步且不含 `.await`，在 `load_balancers` 写锁内调用安全。
    /// `global_round` 仅在此自增一次，各策略分支不得再次自增。
    ///
    /// 时间复杂度: O(n)
    pub fn select(&mut self) -> Option<&Provider> {
        if self.providers.is_empty() {
            return None;
        }

        // 递增全局轮询计数器（唯一自增点）
        self.global_round += 1;

        match self.strategy {
            LoadBalanceStrategy::Frequency => self.select_frequency(),
            LoadBalanceStrategy::WeightedRandom => self.select_weighted_random(),
            LoadBalanceStrategy::HardRoundRobin => self.select_hard_round_robin(),
        }
    }

    /// 频率控制轮询：在硬全轮询基础上按频率分配槽位次数。
    ///
    /// weight=N 表示频率 1/N。对当前供应商集合先取一个有界公共尺度，
    /// 将每个供应商换算为 `scale / N` 个虚拟槽位，然后按供应商顺序逐层
    /// 轮询这些槽位。权重越小，槽位次数越多；weight=0 不参与。
    fn select_frequency(&self) -> Option<&Provider> {
        if self.frequency_order.is_empty() {
            return None;
        }
        let slot = ((self.global_round - 1) as usize) % self.frequency_order.len();
        let provider_index = self.frequency_order[slot];
        self.providers.get(provider_index).map(|p| &p.provider)
    }

    /// 加权随机：p_i = weight_i / Σweight（仅 weight>0 参与）
    ///
    /// 采用同步、无分配、可种子化的 SplitMix64，保证：
    /// - 不在写锁临界区内引入 `.await`；
    /// - 测试可通过显式 `rng_state` 复现序列。
    fn select_weighted_random(&mut self) -> Option<&Provider> {
        // u64 累加器纯防御性（weight<=100 且供应商数量有限，实际不可能溢出）
        let total: u64 = self
            .providers
            .iter()
            .filter(|p| p.weight > 0)
            .map(|p| p.weight as u64)
            .sum();
        if total == 0 {
            return None; // 全零守卫：无可用供应商，交由上层回退
        }

        // SplitMix64：同步、确定性、无分配
        self.rng_state = self
            .rng_state
            .wrapping_add(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(self.global_round as u64);
        let mut z = self.rng_state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        // 模偏置在 total 为数百量级时可忽略
        let mut pick = z % total;

        // 先确定中标的原始下标（释放可变借用后再以不可变借用返回 &Provider），
        // 避免双重 filter 与 idx 默认值导致的静默错路。
        let mut winner: Option<usize> = None;
        for (i, p) in self.providers.iter().enumerate() {
            if p.weight == 0 {
                continue;
            }
            let w = p.weight as u64;
            if pick < w {
                winner = Some(i);
                break;
            }
            pick -= w;
        }
        debug_assert!(
            winner.is_some(),
            "weighted_random 未命中：total/pick 不变量被破坏"
        );
        winner.map(|i| &self.providers[i].provider)
    }

    /// 硬全轮询：等概率轮转 weight>0 的供应商，忽略权重大小
    ///
    /// 仅 0/非0 决定启用与否；轮转顺序遵循 `providers` 现有顺序
    /// （在 provider_router 中为权重升序，由测试固定）。非空时必返回 Some。
    fn select_hard_round_robin(&self) -> Option<&Provider> {
        let enabled: Vec<&WeightedProvider> =
            self.providers.iter().filter(|p| p.weight > 0).collect();
        if enabled.is_empty() {
            return None;
        }
        let index = ((self.global_round - 1) as usize) % enabled.len();
        Some(&enabled[index].provider)
    }

    /// 重置全局计数器
    #[allow(dead_code)] // 保留 LB API（曾由分层转发器使用）
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
    #[allow(dead_code)] // 保留 LB API
    pub fn len(&self) -> usize {
        self.providers.len()
    }

    /// 检查是否为空
    #[allow(dead_code)] // 保留 LB API
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    /// 更新单个Provider的权重
    #[allow(dead_code)] // 保留 LB API（曾由分层转发器使用）
    pub fn update_weight(&mut self, provider_id: &str, weight: u32) -> bool {
        if let Some(p) = self
            .providers
            .iter_mut()
            .find(|p| p.provider.id == provider_id)
        {
            p.weight = weight;
            self.frequency_order = Self::build_frequency_order(&self.providers);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

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

    /// 显式构造 Frequency 均衡器（rng_state 固定为 0 以保证测试确定性）。
    fn freq_lb(providers: Vec<WeightedProvider>) -> LoadBalancer {
        let frequency_order = LoadBalancer::build_frequency_order(&providers);
        LoadBalancer {
            strategy: LoadBalanceStrategy::Frequency,
            providers,
            frequency_order,
            global_round: 0,
            rng_state: 0,
        }
    }

    fn lb(strategy: LoadBalanceStrategy, providers: Vec<WeightedProvider>) -> LoadBalancer {
        let frequency_order = LoadBalancer::build_frequency_order(&providers);
        LoadBalancer {
            strategy,
            providers,
            frequency_order,
            global_round: 0,
            rng_state: 0,
        }
    }

    #[test]
    fn test_strategy_str_round_trip() {
        for s in [
            LoadBalanceStrategy::Frequency,
            LoadBalanceStrategy::WeightedRandom,
            LoadBalanceStrategy::HardRoundRobin,
        ] {
            assert_eq!(s.as_str().parse::<LoadBalanceStrategy>().unwrap(), s);
        }
        assert_eq!(
            LoadBalanceStrategy::default(),
            LoadBalanceStrategy::Frequency
        );
        assert!("nope".parse::<LoadBalanceStrategy>().is_err());
    }

    #[test]
    fn test_frequency_controlled_rr_basic() {
        let providers = vec![
            create_weighted_provider("A", 1),
            create_weighted_provider("B", 2),
            create_weighted_provider("C", 3),
        ];

        let mut lb = freq_lb(providers);

        let seq: Vec<String> = (0..11).map(|_| lb.select().unwrap().id.clone()).collect();

        // scale=lcm(1,2,3)=6，槽位数 A=6、B=3、C=2。
        assert_eq!(
            seq,
            vec!["A", "B", "C", "A", "B", "C", "A", "B", "A", "A", "A"]
        );
        assert_eq!(lb.current_round(), 11);
    }

    #[test]
    fn test_frequency_controlled_rr_no_weight_1() {
        let providers = vec![
            create_weighted_provider("B", 2),
            create_weighted_provider("C", 3),
        ];

        let mut lb = freq_lb(providers);

        let seq: Vec<String> = (0..5).map(|_| lb.select().unwrap().id.clone()).collect();

        // scale=lcm(2,3)=6，槽位数 B=3、C=2；没有 weight=1 时也正常轮询。
        assert_eq!(seq, vec!["B", "C", "B", "C", "B"]);
    }

    #[test]
    fn test_frequency_controlled_rr_weight_0() {
        // 测试weight=0（禁用）
        let providers = vec![
            create_weighted_provider("A", 1),
            create_weighted_provider("B", 0), // 禁用
        ];

        let mut lb = freq_lb(providers);

        // 所有轮次都应该选A，B被禁用
        for _ in 0..10 {
            assert_eq!(lb.select().unwrap().id, "A");
        }
    }

    #[test]
    fn test_reset_counter() {
        let providers = vec![create_weighted_provider("A", 1)];

        let mut lb = freq_lb(providers);

        lb.select(); // Round 1
        lb.select(); // Round 2
        assert_eq!(lb.current_round(), 2);

        lb.reset_counter();
        assert_eq!(lb.current_round(), 0);
    }

    #[test]
    fn test_update_weight() {
        let providers = vec![create_weighted_provider("A", 1)];

        let mut lb = freq_lb(providers);

        assert_eq!(lb.providers[0].weight, 1);

        // 更新权重
        assert!(lb.update_weight("A", 5));
        assert_eq!(lb.providers[0].weight, 5);
        assert_eq!(lb.frequency_order.len(), 1);

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

        let mut lb = freq_lb(providers);

        let mut counts = HashMap::new();

        // 完整周期：scale=10，槽位数 Fast=10、Medium=5、Slow=2，总计17。
        for _ in 0..17 {
            if let Some(p) = lb.select() {
                *counts.entry(p.id.clone()).or_insert(0) += 1;
            }
        }

        assert_eq!(counts.get("Fast"), Some(&10));
        assert_eq!(counts.get("Medium"), Some(&5));
        assert_eq!(counts.get("Slow"), Some(&2));
    }

    #[test]
    fn test_frequency_controlled_rr_tie_breaker() {
        let providers = vec![
            create_weighted_provider("A", 10),
            create_weighted_provider("B", 10),
        ];

        let mut lb = freq_lb(providers);

        let seq: Vec<String> = (0..6).map(|_| lb.select().unwrap().id.clone()).collect();
        assert_eq!(seq, vec!["A", "B", "A", "B", "A", "B"]);
    }

    // ---- WeightedRandom ----

    #[test]
    fn test_weighted_random_distribution() {
        // weight 越大流量越多。A=1,B=3,C=6 -> 期望 0.1/0.3/0.6
        let providers = vec![
            create_weighted_provider("A", 1),
            create_weighted_provider("B", 3),
            create_weighted_provider("C", 6),
        ];
        let mut lb = lb(LoadBalanceStrategy::WeightedRandom, providers);

        let n = 10_000;
        let mut counts: HashMap<String, u32> = HashMap::new();
        for _ in 0..n {
            let id = lb.select().unwrap().id.clone();
            *counts.entry(id).or_insert(0) += 1;
        }

        let frac = |id: &str| *counts.get(id).unwrap_or(&0) as f64 / n as f64;
        assert!((frac("A") - 0.10).abs() < 0.03, "A={}", frac("A"));
        assert!((frac("B") - 0.30).abs() < 0.03, "B={}", frac("B"));
        assert!((frac("C") - 0.60).abs() < 0.03, "C={}", frac("C"));
    }

    #[test]
    fn test_weighted_random_is_deterministic_for_fixed_seed() {
        let build = || {
            lb(
                LoadBalanceStrategy::WeightedRandom,
                vec![
                    create_weighted_provider("A", 1),
                    create_weighted_provider("B", 3),
                    create_weighted_provider("C", 6),
                ],
            )
        };
        let mut lb1 = build();
        let mut lb2 = build();
        for _ in 0..1000 {
            assert_eq!(lb1.select().unwrap().id, lb2.select().unwrap().id);
        }
    }

    #[test]
    fn test_weighted_random_zero_sum_returns_none() {
        let providers = vec![
            create_weighted_provider("A", 0),
            create_weighted_provider("B", 0),
        ];
        let mut lb = lb(LoadBalanceStrategy::WeightedRandom, providers);
        for _ in 0..10 {
            assert!(lb.select().is_none());
        }
    }

    #[test]
    fn test_weighted_random_never_selects_zero_weight() {
        // 中间夹一个 weight=0，确保永不被选且分布大致均匀
        let providers = vec![
            create_weighted_provider("A", 1),
            create_weighted_provider("B", 0),
            create_weighted_provider("C", 1),
        ];
        let mut lb = lb(LoadBalanceStrategy::WeightedRandom, providers);
        let mut counts: HashMap<String, u32> = HashMap::new();
        for _ in 0..2000 {
            let id = lb.select().unwrap().id.clone();
            *counts.entry(id).or_insert(0) += 1;
        }
        assert_eq!(counts.get("B"), None, "weight=0 不应被选中");
        let a = *counts.get("A").unwrap_or(&0);
        let c = *counts.get("C").unwrap_or(&0);
        assert_eq!(a + c, 2000);
        assert!((a as i32 - c as i32).abs() < 300, "A={a} C={c} 应大致均匀");
    }

    #[test]
    fn test_weighted_random_single_provider() {
        let providers = vec![create_weighted_provider("Only", 7)];
        let mut lb = lb(LoadBalanceStrategy::WeightedRandom, providers);
        for _ in 0..50 {
            assert_eq!(lb.select().unwrap().id, "Only");
        }
    }

    // ---- HardRoundRobin（硬全轮询）----

    #[test]
    fn test_hard_round_robin_rotation() {
        // A,B,C 等权，D 禁用 -> 顺序 A,B,C,A,B,C,A，D 永不出现
        let providers = vec![
            create_weighted_provider("A", 5),
            create_weighted_provider("B", 5),
            create_weighted_provider("C", 5),
            create_weighted_provider("D", 0),
        ];
        let mut lb = lb(LoadBalanceStrategy::HardRoundRobin, providers);
        let seq: Vec<String> = (0..7).map(|_| lb.select().unwrap().id.clone()).collect();
        assert_eq!(seq, vec!["A", "B", "C", "A", "B", "C", "A"]);
    }

    #[test]
    fn test_hard_round_robin_ignores_weight_magnitude() {
        // 权重大小被忽略：A=1,B=100 -> 严格交替
        let providers = vec![
            create_weighted_provider("A", 1),
            create_weighted_provider("B", 100),
        ];
        let mut lb = lb(LoadBalanceStrategy::HardRoundRobin, providers);
        let seq: Vec<String> = (0..4).map(|_| lb.select().unwrap().id.clone()).collect();
        assert_eq!(seq, vec!["A", "B", "A", "B"]);
    }
}
