use crate::agents::*;
use crate::amm::Amm;
use crate::cdp::{CdpConfig, VaultRegistry};
use crate::circuit_breaker::*;
use crate::controller::{Controller, ControllerConfig};
use crate::liquidation::{LiquidationConfig, LiquidationEngine};

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

/// Per-block metrics snapshot.
#[derive(Debug, Clone)]
pub struct BlockMetrics {
    pub block: u64,
    pub external_price: f64,
    pub amm_spot_price: f64,
    pub twap_price: f64,
    pub redemption_price: f64,
    pub redemption_rate: f64,
    pub total_debt: f64,
    pub amm_reserve_zec: f64,
    pub amm_reserve_zai: f64,
    pub vault_count: u64,
    pub liquidation_count: u32,
    pub bad_debt: f64,
    pub breaker_actions: Vec<BreakerAction>,
    pub debt_ceiling: f64,
    pub minting_paused: bool,
    pub halted: bool,
    pub total_collateral: f64,
    pub total_lp_shares: f64,
    pub arber_zai_total: f64,
    // Zombie vault metrics
    pub zombie_vault_count: u32,
    pub max_zombie_gap: f64,
    pub mean_collateral_ratio_twap: f64,
    pub mean_collateral_ratio_ext: f64,
    // Enhanced report metrics
    pub arber_zec_total: f64,
    pub cumulative_fees_zai: f64,
    pub cumulative_il_pct: f64,
}

/// Configuration for a scenario run.
#[derive(Debug, Clone)]
pub struct ScenarioConfig {
    pub amm_initial_zec: f64,
    pub amm_initial_zai: f64,
    pub amm_swap_fee: f64,
    pub cdp_config: CdpConfig,
    pub controller_config: ControllerConfig,
    pub liquidation_config: LiquidationConfig,
    pub twap_breaker_config: TwapBreakerConfig,
    pub cascade_breaker_config: CascadeBreakerConfig,
    pub debt_ceiling_config: DebtCeilingConfig,
    pub initial_redemption_price: f64,
    // Stochastic noise parameters
    pub stochastic: bool,
    pub noise_sigma: f64,
    pub arber_activity_rate: f64,
    pub demand_jitter_blocks: u64,
    pub miner_batch_window: u64,
    // AMM liquidation feedback
    pub use_amm_liquidation: bool,
    // Zombie vault mitigation
    pub zombie_detector: bool,
    pub zombie_gap_threshold: f64,
    // LP incentive mechanisms
    pub stability_fee_to_lps: bool,
    // Oracle-based liquidation: use external_price for liquidation eligibility
    // instead of AMM TWAP. Demonstrates death spiral when combined with
    // use_amm_liquidation=true (collateral sold through AMM).
    pub use_external_oracle_for_liquidation: bool,
}

impl Default for ScenarioConfig {
    fn default() -> Self {
        ScenarioConfig {
            amm_initial_zec: 10000.0,
            amm_initial_zai: 500000.0,
            amm_swap_fee: 0.003,
            cdp_config: CdpConfig::default(),
            controller_config: ControllerConfig::default_pi(),
            liquidation_config: LiquidationConfig::default(),
            twap_breaker_config: TwapBreakerConfig::default(),
            cascade_breaker_config: CascadeBreakerConfig::default(),
            debt_ceiling_config: DebtCeilingConfig::default(),
            initial_redemption_price: 50.0,
            stochastic: false,
            noise_sigma: 0.02,
            arber_activity_rate: 0.8,
            demand_jitter_blocks: 10,
            miner_batch_window: 10,
            use_amm_liquidation: false,
            zombie_detector: false,
            zombie_gap_threshold: 0.5,
            stability_fee_to_lps: false,
            use_external_oracle_for_liquidation: false,
        }
    }
}

/// The full simulation state.
pub struct Scenario {
    pub amm: Amm,
    pub registry: VaultRegistry,
    pub controller: Controller,
    pub liquidation_engine: LiquidationEngine,
    pub breakers: CircuitBreakerEngine,
    pub metrics: Vec<BlockMetrics>,

    // Agents
    pub arbers: Vec<Arbitrageur>,
    pub demand_agents: Vec<DemandAgent>,
    pub miners: Vec<MinerAgent>,
    pub cdp_holders: Vec<CdpHolder>,
    pub lp_agents: Vec<LpAgent>,
    pub il_aware_lps: Vec<IlAwareLpAgent>,
    pub attackers: Vec<Attacker>,

    // Stochastic state
    pub config: ScenarioConfig,
    rng: StdRng,
    miner_sell_countdowns: Vec<u64>,
}

impl Scenario {
    pub fn new(config: &ScenarioConfig) -> Self {
        Self::new_with_seed(config, 42)
    }

    pub fn new_with_seed(config: &ScenarioConfig, seed: u64) -> Self {
        Scenario {
            amm: Amm::new(config.amm_initial_zec, config.amm_initial_zai, config.amm_swap_fee),
            registry: VaultRegistry::new(config.cdp_config.clone()),
            controller: Controller::new(
                config.controller_config.clone(),
                config.initial_redemption_price,
                0,
            ),
            liquidation_engine: LiquidationEngine::new(config.liquidation_config.clone()),
            breakers: CircuitBreakerEngine::new(
                config.twap_breaker_config.clone(),
                config.cascade_breaker_config.clone(),
                config.debt_ceiling_config.clone(),
            ),
            metrics: Vec::new(),
            arbers: Vec::new(),
            demand_agents: Vec::new(),
            miners: Vec::new(),
            cdp_holders: Vec::new(),
            lp_agents: Vec::new(),
            il_aware_lps: Vec::new(),
            attackers: Vec::new(),
            config: config.clone(),
            rng: StdRng::seed_from_u64(seed.wrapping_add(0xBEEF)),
            miner_sell_countdowns: Vec::new(),
        }
    }

    /// Run the simulation for a given price series.
    /// `external_prices` maps block number to external ZEC price.
    pub fn run(&mut self, external_prices: &[f64]) {
        // Initialize LP agents
        for lp in &mut self.lp_agents {
            lp.provide_liquidity(&mut self.amm);
        }

        // Initialize IL-aware LP agents
        for lp in &mut self.il_aware_lps {
            lp.provide_liquidity(&mut self.amm);
        }

        // Initialize CDP holders
        for holder in &mut self.cdp_holders {
            let _ = holder.open_vault(&mut self.registry, &self.amm, 0);
        }

        // Initialize miner sell countdowns for stochastic mode
        if self.config.stochastic && self.miner_sell_countdowns.is_empty() {
            for _ in 0..self.miners.len() {
                let countdown = self.rng.gen_range(1..=self.config.miner_batch_window);
                self.miner_sell_countdowns.push(countdown);
            }
        }

        for (i, &ext_price) in external_prices.iter().enumerate() {
            let block = i as u64 + 1;
            self.step(block, ext_price);
        }
    }

    /// Execute a single block of the simulation.
    pub fn step(&mut self, block: u64, external_price: f64) {
        let halted = self.breakers.is_halted(block);
        let minting_paused = self.breakers.is_minting_paused(block);
        let redemption_price = self.controller.redemption_price;
        let stochastic = self.config.stochastic;

        // (1) External price is provided as parameter

        // (2) Arbitrageurs trade
        if !halted {
            let activity_rate = self.config.arber_activity_rate;
            for arber in &mut self.arbers {
                // Stochastic: skip with probability (1 - activity_rate)
                if stochastic && self.rng.gen::<f64>() >= activity_rate {
                    continue;
                }
                arber.act(&mut self.amm, external_price, block);
            }
        }

        // (3) CDP holders act
        if !halted {
            for holder in &mut self.cdp_holders {
                holder.act(&mut self.registry, &self.amm, block);
            }
        }

        // (4) Demand agents act
        if !halted {
            let jitter = self.config.demand_jitter_blocks;
            for demand in &mut self.demand_agents {
                // Stochastic: skip with probability jitter/(jitter+20)
                if stochastic && self.rng.gen_range(0..jitter + 20) < jitter {
                    continue;
                }
                demand.act(&mut self.amm, redemption_price, block);
            }
        }

        // (4b) Miners act
        if !halted {
            if stochastic && !self.miner_sell_countdowns.is_empty() {
                for i in 0..self.miners.len() {
                    // Always receive block reward
                    self.miners[i].zec_balance += self.miners[i].config.block_reward;

                    self.miner_sell_countdowns[i] =
                        self.miner_sell_countdowns[i].saturating_sub(1);
                    if self.miner_sell_countdowns[i] == 0 {
                        // Batch sell accumulated ZEC
                        let sell_frac = self.miners[i].config.miner_sell_fraction;
                        let amm_frac = self.miners[i].config.miner_amm_fraction;
                        let sell_amount =
                            self.miners[i].zec_balance * sell_frac * amm_frac;
                        if sell_amount > 0.001 {
                            if let Ok(zai_out) =
                                self.amm.swap_zec_for_zai(sell_amount, block)
                            {
                                self.miners[i].zec_balance -= sell_amount;
                                self.miners[i].zai_balance += zai_out;
                            }
                        }
                        let bw = self.config.miner_batch_window;
                        self.miner_sell_countdowns[i] = self.rng.gen_range(1..=bw);
                    }
                }
            } else {
                for miner in &mut self.miners {
                    miner.act(&mut self.amm, block);
                }
            }
        }

        // (4c) LPs act
        if !halted {
            for lp in &mut self.lp_agents {
                lp.act(&mut self.amm);
            }
            for lp in &mut self.il_aware_lps {
                lp.act(&mut self.amm, external_price);
            }
        }

        // (4e) Stability fee routing to LPs
        if self.config.stability_fee_to_lps {
            let fee_delta = self.registry.accrue_all_fees(block);
            if fee_delta > 0.0 {
                self.amm.reserve_zai += fee_delta;
                self.amm.k = self.amm.reserve_zec * self.amm.reserve_zai;
                self.amm.cumulative_fees_zai += fee_delta;
            }
        }

        // (4d) Attackers act
        for attacker in &mut self.attackers {
            attacker.act(&mut self.amm, block);
        }

        // (5) AMM records price for TWAP
        self.amm.record_price(block);

        // (6 & 7) Liquidation engine scans and executes
        let liq_results = if self.config.use_external_oracle_for_liquidation {
            // Oracle mode: use external price for eligibility, sell through AMM
            self.liquidation_engine
                .oracle_liquidate(&mut self.registry, &mut self.amm, block, external_price)
        } else if self.config.use_amm_liquidation {
            self.liquidation_engine
                .cascading_spot_liquidate(&mut self.registry, &mut self.amm, block)
        } else {
            self.liquidation_engine
                .transparent_liquidate(&mut self.registry, &mut self.amm, block)
        };

        // Zombie vault detection and liquidation
        let zombie_liq_results = if self.config.zombie_detector {
            self.liquidation_engine.zombie_detect_and_liquidate(
                &mut self.registry,
                &mut self.amm,
                block,
                self.config.zombie_gap_threshold,
            )
        } else {
            Vec::new()
        };

        let liq_count = (liq_results.len() + zombie_liq_results.len()) as u32;

        // Record liquidations for cascade breaker
        self.breakers.record_liquidations(block, liq_count);

        // (8) Controller updates redemption rate
        let market_price = self.amm.spot_price();
        self.controller.update(market_price, block);

        // (9) Circuit breaker checks
        let breaker_actions = self.breakers.check_all(
            &self.amm,
            &self.registry,
            self.controller.redemption_price,
            block,
        );

        // (10) Record metrics
        let mut metrics = BlockMetrics {
            block,
            external_price,
            amm_spot_price: self.amm.spot_price(),
            twap_price: self.amm.get_twap(self.registry.config.twap_window),
            redemption_price: self.controller.redemption_price,
            redemption_rate: self.controller.redemption_rate,
            total_debt: self.registry.total_debt,
            amm_reserve_zec: self.amm.reserve_zec,
            amm_reserve_zai: self.amm.reserve_zai,
            vault_count: self.registry.vaults.len() as u64,
            liquidation_count: liq_count,
            bad_debt: self.liquidation_engine.total_bad_debt,
            breaker_actions,
            debt_ceiling: self.breakers.debt_ceiling.current_ceiling,
            minting_paused,
            halted,
            total_collateral: self
                .registry
                .vaults
                .values()
                .map(|v| v.collateral_zec)
                .sum::<f64>(),
            total_lp_shares: self.amm.total_lp_shares,
            arber_zai_total: self.arbers.iter().map(|a| a.zai_balance).sum::<f64>(),
            zombie_vault_count: 0,
            max_zombie_gap: 0.0,
            mean_collateral_ratio_twap: 0.0,
            mean_collateral_ratio_ext: 0.0,
            arber_zec_total: self.arbers.iter().map(|a| a.zec_balance).sum::<f64>(),
            cumulative_fees_zai: self.amm.cumulative_fees_zai,
            cumulative_il_pct: self.amm.impermanent_loss(self.config.initial_redemption_price),
        };

        // Compute zombie vault metrics
        let twap = self.amm.get_twap(self.registry.config.twap_window);
        let min_ratio = self.registry.config.min_ratio;
        let mut zombie_count = 0u32;
        let mut max_gap = 0.0f64;
        let mut twap_ratios_sum = 0.0f64;
        let mut ext_ratios_sum = 0.0f64;
        let mut vault_with_debt = 0u32;

        for vault in self.registry.vaults.values() {
            if vault.debt_zai > 0.0 {
                let twap_ratio = vault.collateral_ratio(twap);
                let ext_ratio = vault.collateral_ratio(external_price);
                twap_ratios_sum += twap_ratio;
                ext_ratios_sum += ext_ratio;
                vault_with_debt += 1;

                if twap_ratio >= min_ratio && ext_ratio < min_ratio {
                    zombie_count += 1;
                    let gap = twap_ratio - ext_ratio;
                    if gap > max_gap {
                        max_gap = gap;
                    }
                }
            }
        }

        if vault_with_debt > 0 {
            metrics.mean_collateral_ratio_twap = twap_ratios_sum / vault_with_debt as f64;
            metrics.mean_collateral_ratio_ext = ext_ratios_sum / vault_with_debt as f64;
        }
        metrics.zombie_vault_count = zombie_count;
        metrics.max_zombie_gap = max_gap;

        self.metrics.push(metrics);
    }

    /// Export metrics to CSV.
    pub fn save_metrics_csv(
        &self,
        path: &std::path::Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut wtr = csv::Writer::from_path(path)?;
        wtr.write_record([
            "block",
            "external_price",
            "amm_spot_price",
            "twap_price",
            "redemption_price",
            "redemption_rate",
            "total_debt",
            "reserve_zec",
            "reserve_zai",
            "vault_count",
            "liquidations",
            "bad_debt",
            "debt_ceiling",
            "minting_paused",
            "halted",
            "total_collateral",
            "total_lp_shares",
            "arber_zai_total",
            "zombie_vault_count",
            "max_zombie_gap",
            "mean_cr_twap",
            "mean_cr_ext",
            "arber_zec_total",
            "cumulative_fees_zai",
            "cumulative_il_pct",
        ])?;

        for m in &self.metrics {
            wtr.write_record(&[
                m.block.to_string(),
                format!("{:.4}", m.external_price),
                format!("{:.4}", m.amm_spot_price),
                format!("{:.4}", m.twap_price),
                format!("{:.6}", m.redemption_price),
                format!("{:.12}", m.redemption_rate),
                format!("{:.2}", m.total_debt),
                format!("{:.2}", m.amm_reserve_zec),
                format!("{:.2}", m.amm_reserve_zai),
                m.vault_count.to_string(),
                m.liquidation_count.to_string(),
                format!("{:.2}", m.bad_debt),
                format!("{:.0}", m.debt_ceiling),
                m.minting_paused.to_string(),
                m.halted.to_string(),
                format!("{:.2}", m.total_collateral),
                format!("{:.2}", m.total_lp_shares),
                format!("{:.2}", m.arber_zai_total),
                m.zombie_vault_count.to_string(),
                format!("{:.4}", m.max_zombie_gap),
                format!("{:.4}", m.mean_collateral_ratio_twap),
                format!("{:.4}", m.mean_collateral_ratio_ext),
                format!("{:.2}", m.arber_zec_total),
                format!("{:.2}", m.cumulative_fees_zai),
                format!("{:.6}", m.cumulative_il_pct),
            ])?;
        }
        wtr.flush()?;
        Ok(())
    }
}
