use crate::agents::*;
use crate::scenario::{Scenario, ScenarioConfig};
use rand::rngs::StdRng;
use rand::SeedableRng;
use rand_distr::{Distribution, Normal};

const DEFAULT_BLOCKS: usize = 1000;

/// Identifier for each of the 13 stress scenarios.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ScenarioId {
    SteadyState = 1,
    BlackThursday = 2,
    FlashCrash = 3,
    SustainedBear = 4,
    TwapManipulation = 5,
    LiquidityCrisis = 6,
    BankRun = 7,
    BullMarket = 8,
    OracleComparison = 9,
    CombinedStress = 10,
    DemandShock = 11,
    MinerCapitulation = 12,
    SequencerDowntime = 13,
}

impl ScenarioId {
    pub fn all() -> Vec<ScenarioId> {
        use ScenarioId::*;
        vec![
            SteadyState,
            BlackThursday,
            FlashCrash,
            SustainedBear,
            TwapManipulation,
            LiquidityCrisis,
            BankRun,
            BullMarket,
            OracleComparison,
            CombinedStress,
            DemandShock,
            MinerCapitulation,
            SequencerDowntime,
        ]
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::SteadyState => "steady_state",
            Self::BlackThursday => "black_thursday",
            Self::FlashCrash => "flash_crash",
            Self::SustainedBear => "sustained_bear",
            Self::TwapManipulation => "twap_manipulation",
            Self::LiquidityCrisis => "liquidity_crisis",
            Self::BankRun => "bank_run",
            Self::BullMarket => "bull_market",
            Self::OracleComparison => "oracle_comparison",
            Self::CombinedStress => "combined_stress",
            Self::DemandShock => "demand_shock",
            Self::MinerCapitulation => "miner_capitulation",
            Self::SequencerDowntime => "sequencer_downtime",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Self::SteadyState => "Constant price, baseline behavior",
            Self::BlackThursday => "Severe crash (60%+) with partial recovery",
            Self::FlashCrash => "Sudden drop and rapid recovery",
            Self::SustainedBear => "Gradual decline over extended period",
            Self::TwapManipulation => "Short-term price manipulation attempts",
            Self::LiquidityCrisis => "High volatility, thin liquidity",
            Self::BankRun => "Mass exit / cascading sells",
            Self::BullMarket => "Sustained price increase",
            Self::OracleComparison => "Volatile oscillations for TWAP testing",
            Self::CombinedStress => "Multiple stress events in sequence",
            Self::DemandShock => "Sudden ZAI demand surge then collapse",
            Self::MinerCapitulation => "Miner dump waves",
            Self::SequencerDowntime => "Network pause then resume with price gap",
        }
    }
}

/// Apply multiplicative noise to a price path.
/// Each price is multiplied by (1 + Normal(0, sigma)).
/// Uses a different seed offset to avoid correlation with price generation.
pub fn apply_price_noise(prices: &mut [f64], sigma: f64, seed: u64) {
    let mut rng = StdRng::seed_from_u64(seed.wrapping_add(0xCAFE_BABE));
    let normal = Normal::new(0.0, sigma).unwrap();
    for p in prices.iter_mut() {
        let noise: f64 = normal.sample(&mut rng);
        *p *= 1.0 + noise;
        *p = p.max(1.0); // floor at $1
    }
}

/// Generate a price path for the given scenario.
pub fn generate_prices(id: ScenarioId, blocks: usize, seed: u64) -> Vec<f64> {
    match id {
        ScenarioId::SteadyState => steady_state_prices(blocks),
        ScenarioId::BlackThursday => black_thursday_prices(blocks),
        ScenarioId::FlashCrash => flash_crash_prices(blocks),
        ScenarioId::SustainedBear => sustained_bear_prices(blocks),
        ScenarioId::TwapManipulation => twap_manipulation_prices(blocks),
        ScenarioId::LiquidityCrisis => liquidity_crisis_prices(blocks, seed),
        ScenarioId::BankRun => bank_run_prices(blocks),
        ScenarioId::BullMarket => bull_market_prices(blocks),
        ScenarioId::OracleComparison => oracle_comparison_prices(blocks),
        ScenarioId::CombinedStress => combined_stress_prices(blocks),
        ScenarioId::DemandShock => demand_shock_prices(blocks),
        ScenarioId::MinerCapitulation => miner_capitulation_prices(blocks),
        ScenarioId::SequencerDowntime => sequencer_downtime_prices(blocks),
    }
}

/// Add appropriate agents to a scenario based on scenario type.
pub fn add_agents(id: ScenarioId, scenario: &mut Scenario) {
    // All scenarios get at least one arber and one miner
    scenario
        .arbers
        .push(Arbitrageur::new(ArbitrageurConfig::default()));
    scenario
        .miners
        .push(MinerAgent::new(MinerAgentConfig::default()));

    match id {
        ScenarioId::BankRun => {
            // Demand agents configured for panic selling
            scenario
                .demand_agents
                .push(DemandAgent::new(DemandAgentConfig {
                    demand_elasticity: 0.02,
                    demand_exit_threshold_pct: 3.0,
                    demand_exit_window_blocks: 20,
                    demand_panic_sell_fraction: 0.8,
                    initial_zec_balance: 10_000.0,
                    ..DemandAgentConfig::default()
                }));
        }
        ScenarioId::TwapManipulation => {
            scenario.attackers.push(Attacker::new(AttackerConfig {
                attack_capital_zec: 5000.0,
                hold_blocks: 3,
                attack_at_block: 500,
            }));
        }
        ScenarioId::MinerCapitulation => {
            // Extra miners with aggressive selling
            for _ in 0..3 {
                scenario
                    .miners
                    .push(MinerAgent::new(MinerAgentConfig {
                        miner_sell_fraction: 1.0,
                        miner_amm_fraction: 1.0,
                        ..MinerAgentConfig::default()
                    }));
            }
        }
        ScenarioId::DemandShock => {
            scenario
                .demand_agents
                .push(DemandAgent::new(DemandAgentConfig {
                    demand_elasticity: 0.10,
                    demand_base_rate: 5.0,
                    initial_zec_balance: 20_000.0,
                    ..DemandAgentConfig::default()
                }));
        }
        ScenarioId::LiquidityCrisis => {
            // LP that may withdraw under stress
            scenario.lp_agents.push(LpAgent::new(LpAgentConfig {
                il_threshold: 0.03, // sensitive to IL
                ..LpAgentConfig::default()
            }));
        }
        _ => {}
    }
}

/// Build and run a complete stress scenario.
pub fn run_stress(
    id: ScenarioId,
    config: &ScenarioConfig,
    blocks: usize,
    seed: u64,
) -> Scenario {
    let mut prices = generate_prices(id, blocks, seed);
    if config.stochastic {
        apply_price_noise(&mut prices, config.noise_sigma, seed);
    }
    let mut scenario = Scenario::new_with_seed(config, seed);
    add_agents(id, &mut scenario);
    scenario.run(&prices);
    scenario
}

/// Build and run with default config and block count.
pub fn run_stress_default(id: ScenarioId) -> Scenario {
    run_stress(id, &ScenarioConfig::default(), DEFAULT_BLOCKS, 42)
}

// ═══════════════════════════════════════════════════════════════════════
// Price Path Generators
// ═══════════════════════════════════════════════════════════════════════

fn steady_state_prices(blocks: usize) -> Vec<f64> {
    vec![50.0; blocks]
}

fn black_thursday_prices(blocks: usize) -> Vec<f64> {
    let mut prices = Vec::with_capacity(blocks);
    let crash_start = blocks / 4;
    let crash_end = crash_start + blocks / 10;
    let recovery_end = crash_end + blocks / 4;

    for i in 0..blocks {
        let price = if i < crash_start {
            50.0
        } else if i < crash_end {
            let t = (i - crash_start) as f64 / (crash_end - crash_start) as f64;
            50.0 - 30.0 * t
        } else if i < recovery_end {
            let t = (i - crash_end) as f64 / (recovery_end - crash_end) as f64;
            20.0 + 15.0 * t
        } else {
            35.0
        };
        prices.push(price);
    }
    prices
}

fn flash_crash_prices(blocks: usize) -> Vec<f64> {
    let mut prices = Vec::with_capacity(blocks);
    let crash_block = blocks / 2;
    let crash_depth = 10;
    let recovery_length = 50;

    for i in 0..blocks {
        let price = if i < crash_block {
            50.0
        } else if i < crash_block + crash_depth {
            let t = (i - crash_block) as f64 / crash_depth as f64;
            50.0 - 25.0 * t
        } else if i < crash_block + crash_depth + recovery_length {
            let t = (i - crash_block - crash_depth) as f64 / recovery_length as f64;
            25.0 + 23.0 * t
        } else {
            48.0
        };
        prices.push(price);
    }
    prices
}

fn sustained_bear_prices(blocks: usize) -> Vec<f64> {
    (0..blocks)
        .map(|i| {
            let t = i as f64 / blocks as f64;
            50.0 - 35.0 * t
        })
        .collect()
}

fn twap_manipulation_prices(blocks: usize) -> Vec<f64> {
    let mut prices = Vec::with_capacity(blocks);
    for i in 0..blocks {
        if i > 200 && i % 100 < 2 {
            prices.push(100.0); // 2x spike for 2 blocks
        } else {
            prices.push(50.0);
        }
    }
    prices
}

fn liquidity_crisis_prices(blocks: usize, seed: u64) -> Vec<f64> {
    let mut rng = StdRng::seed_from_u64(seed);
    let normal = Normal::new(0.0, 2.0).unwrap();
    let mut price: f64 = 50.0;
    let mut prices = Vec::with_capacity(blocks);

    for _ in 0..blocks {
        price += normal.sample(&mut rng);
        price = price.clamp(10.0, 120.0);
        prices.push(price);
    }
    prices
}

fn bank_run_prices(blocks: usize) -> Vec<f64> {
    let mut prices = Vec::with_capacity(blocks);
    let panic_start = blocks / 3;

    for i in 0..blocks {
        let price = if i < panic_start {
            50.0
        } else {
            let t = (i - panic_start) as f64 / (blocks - panic_start) as f64;
            50.0 - 30.0 * t.powf(1.5)
        };
        prices.push(price.max(10.0));
    }
    prices
}

fn bull_market_prices(blocks: usize) -> Vec<f64> {
    (0..blocks)
        .map(|i| {
            let t = i as f64 / blocks as f64;
            30.0 + 70.0 * t
        })
        .collect()
}

fn oracle_comparison_prices(blocks: usize) -> Vec<f64> {
    (0..blocks)
        .map(|i| {
            let cycle = 50.0;
            let t = (i as f64 % cycle) / cycle;
            50.0 + 15.0 * (2.0 * std::f64::consts::PI * t).sin()
        })
        .collect()
}

fn combined_stress_prices(blocks: usize) -> Vec<f64> {
    let mut prices = Vec::with_capacity(blocks);
    let phase1 = blocks / 4;
    let phase2 = blocks / 2;
    let phase3 = 3 * blocks / 4;

    for i in 0..blocks {
        let price = if i < phase1 {
            // Gradual decline
            50.0 - 10.0 * (i as f64 / phase1 as f64)
        } else if i < phase2 {
            // Flash crash phase
            let t = (i - phase1) as f64 / (phase2 - phase1) as f64;
            if t < 0.1 {
                40.0 - 15.0 * (t / 0.1)
            } else {
                25.0 + 10.0 * ((t - 0.1) / 0.9)
            }
        } else if i < phase3 {
            // Slow recovery
            let t = (i - phase2) as f64 / (phase3 - phase2) as f64;
            35.0 + 10.0 * t
        } else {
            45.0
        };
        prices.push(price.max(10.0));
    }
    prices
}

fn demand_shock_prices(blocks: usize) -> Vec<f64> {
    let mut prices = Vec::with_capacity(blocks);
    let surge_start = blocks / 3;
    let surge_end = blocks / 2;

    for i in 0..blocks {
        let price = if i < surge_start {
            50.0
        } else if i < surge_end {
            let t = (i - surge_start) as f64 / (surge_end - surge_start) as f64;
            50.0 + 20.0 * t
        } else {
            let t = (i - surge_end) as f64 / (blocks - surge_end) as f64;
            70.0 - 30.0 * t
        };
        prices.push(price.max(10.0));
    }
    prices
}

fn miner_capitulation_prices(blocks: usize) -> Vec<f64> {
    let mut prices = Vec::with_capacity(blocks);

    for i in 0..blocks {
        let phase = (i * 3 / blocks).min(2);
        let base = 50.0 - 10.0 * phase as f64;
        let within = (i as f64 * 3.0 / blocks as f64) % 1.0;

        // Each wave has a dip and partial recovery
        let price = if within < 0.3 {
            base - 8.0 * (within / 0.3)
        } else {
            (base - 8.0) + 5.0 * ((within - 0.3) / 0.7)
        };
        prices.push(price.max(10.0));
    }
    prices
}

fn sequencer_downtime_prices(blocks: usize) -> Vec<f64> {
    let mut prices = Vec::with_capacity(blocks);
    let downtime_start = blocks * 2 / 5;
    let downtime_end = blocks * 3 / 5;

    for i in 0..blocks {
        let price = if i < downtime_start {
            50.0
        } else if i < downtime_end {
            // During downtime, external price holds (network frozen)
            50.0
        } else {
            // After downtime, price jumps to reflect new reality
            35.0
        };
        prices.push(price);
    }
    prices
}
