use crate::amm::Amm;
use crate::cdp::VaultRegistry;

/// Circuit breaker actions the simulation loop should take.
#[derive(Debug, Clone, PartialEq)]
pub enum BreakerAction {
    /// No action needed.
    None,
    /// Pause new CDP minting for N blocks.
    PauseMinting { blocks: u64, reason: String },
    /// Reduce debt ceiling.
    ReduceDebtCeiling { new_ceiling: f64, reason: String },
    /// Halt all non-liquidation activity.
    EmergencyHalt { reason: String },
}

// ═══════════════════════════════════════════════════════════════════════
// TWAP Movement Circuit Breaker
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct TwapBreakerConfig {
    /// Maximum allowed TWAP change (fraction) per window before triggering.
    /// E.g., 0.15 = 15% movement triggers breaker.
    pub max_twap_change_pct: f64,
    /// TWAP window in blocks for the short-term reading.
    pub short_window: u64,
    /// TWAP window in blocks for the long-term reading.
    pub long_window: u64,
    /// Blocks to pause minting when triggered.
    pub pause_blocks: u64,
}

impl Default for TwapBreakerConfig {
    fn default() -> Self {
        TwapBreakerConfig {
            max_twap_change_pct: 0.15,
            short_window: 12,   // ~15 minutes
            long_window: 48,    // ~1 hour
            pause_blocks: 48,
        }
    }
}

#[derive(Debug)]
pub struct TwapBreaker {
    pub config: TwapBreakerConfig,
    pub triggered: bool,
    pub resume_at_block: u64,
    pub trigger_count: u64,
}

impl TwapBreaker {
    pub fn new(config: TwapBreakerConfig) -> Self {
        TwapBreaker {
            config,
            triggered: false,
            resume_at_block: 0,
            trigger_count: 0,
        }
    }

    /// Check TWAP divergence and return action if breaker should trigger.
    pub fn check(&mut self, amm: &Amm, block: u64) -> BreakerAction {
        // If currently triggered, check if we can resume
        if self.triggered {
            if block >= self.resume_at_block {
                self.triggered = false;
            }
            return BreakerAction::None;
        }

        let twap_short = amm.get_twap(self.config.short_window);
        let twap_long = amm.get_twap(self.config.long_window);

        if twap_long == 0.0 {
            return BreakerAction::None;
        }

        let change = ((twap_short - twap_long) / twap_long).abs();

        if change > self.config.max_twap_change_pct {
            self.triggered = true;
            self.resume_at_block = block + self.config.pause_blocks;
            self.trigger_count += 1;

            BreakerAction::PauseMinting {
                blocks: self.config.pause_blocks,
                reason: format!(
                    "TWAP divergence {:.2}% exceeds {:.2}% threshold (short={:.2}, long={:.2})",
                    change * 100.0,
                    self.config.max_twap_change_pct * 100.0,
                    twap_short,
                    twap_long,
                ),
            }
        } else {
            BreakerAction::None
        }
    }

    pub fn is_active(&self, block: u64) -> bool {
        self.triggered && block < self.resume_at_block
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Cascade Circuit Breaker
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct CascadeBreakerConfig {
    /// Maximum liquidations within the window before triggering.
    pub max_liquidations_in_window: u32,
    /// Window size in blocks to count liquidations.
    pub window_blocks: u64,
    /// Blocks to pause when triggered.
    pub pause_blocks: u64,
}

impl Default for CascadeBreakerConfig {
    fn default() -> Self {
        CascadeBreakerConfig {
            max_liquidations_in_window: 10,
            window_blocks: 48,
            pause_blocks: 96,
        }
    }
}

#[derive(Debug)]
pub struct CascadeBreaker {
    pub config: CascadeBreakerConfig,
    pub triggered: bool,
    pub resume_at_block: u64,
    pub trigger_count: u64,
    /// Ring buffer of (block, liquidation_count) per block.
    liquidation_log: Vec<(u64, u32)>,
}

impl CascadeBreaker {
    pub fn new(config: CascadeBreakerConfig) -> Self {
        CascadeBreaker {
            config,
            triggered: false,
            resume_at_block: 0,
            trigger_count: 0,
            liquidation_log: Vec::new(),
        }
    }

    /// Record liquidations that happened at a given block.
    pub fn record_liquidations(&mut self, block: u64, count: u32) {
        if count > 0 {
            self.liquidation_log.push((block, count));
        }
    }

    /// Check if cascade threshold has been reached.
    pub fn check(&mut self, block: u64) -> BreakerAction {
        if self.triggered {
            if block >= self.resume_at_block {
                self.triggered = false;
            }
            return BreakerAction::None;
        }

        let window_start = block.saturating_sub(self.config.window_blocks);
        let total_in_window: u32 = self
            .liquidation_log
            .iter()
            .filter(|(b, _)| *b >= window_start)
            .map(|(_, c)| c)
            .sum();

        if total_in_window > self.config.max_liquidations_in_window {
            self.triggered = true;
            self.resume_at_block = block + self.config.pause_blocks;
            self.trigger_count += 1;

            // Prune old entries
            self.liquidation_log.retain(|(b, _)| *b >= window_start);

            BreakerAction::EmergencyHalt {
                reason: format!(
                    "Cascade: {} liquidations in {} blocks exceeds limit of {}",
                    total_in_window, self.config.window_blocks, self.config.max_liquidations_in_window,
                ),
            }
        } else {
            BreakerAction::None
        }
    }

    pub fn is_active(&self, block: u64) -> bool {
        self.triggered && block < self.resume_at_block
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Dynamic Debt Ceiling
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct DebtCeilingConfig {
    /// Initial maximum total debt allowed.
    pub initial_ceiling: f64,
    /// Minimum ceiling (floor for reduction).
    pub min_ceiling: f64,
    /// Fraction to reduce ceiling by when triggered.
    pub reduction_factor: f64,
    /// Rate at which ceiling grows per block when healthy.
    pub growth_rate_per_block: f64,
    /// TWAP deviation threshold that triggers ceiling reduction.
    pub deviation_threshold: f64,
}

impl Default for DebtCeilingConfig {
    fn default() -> Self {
        DebtCeilingConfig {
            initial_ceiling: 1_000_000.0,
            min_ceiling: 100_000.0,
            reduction_factor: 0.10,
            growth_rate_per_block: 0.1,
            deviation_threshold: 0.10,
        }
    }
}

#[derive(Debug)]
pub struct DebtCeiling {
    pub config: DebtCeilingConfig,
    pub current_ceiling: f64,
    pub reductions: u64,
}

impl DebtCeiling {
    pub fn new(config: DebtCeilingConfig) -> Self {
        let ceiling = config.initial_ceiling;
        DebtCeiling {
            config,
            current_ceiling: ceiling,
            reductions: 0,
        }
    }

    /// Update the debt ceiling based on system health.
    pub fn update(
        &mut self,
        amm: &Amm,
        _registry: &VaultRegistry,
        redemption_price: f64,
    ) -> BreakerAction {
        let market_price = amm.spot_price();
        let deviation = ((market_price - redemption_price) / redemption_price).abs();

        if deviation > self.config.deviation_threshold {
            // Reduce ceiling
            let reduction = self.current_ceiling * self.config.reduction_factor;
            self.current_ceiling =
                (self.current_ceiling - reduction).max(self.config.min_ceiling);
            self.reductions += 1;

            BreakerAction::ReduceDebtCeiling {
                new_ceiling: self.current_ceiling,
                reason: format!(
                    "Price deviation {:.2}% > {:.2}% threshold; ceiling reduced to {:.0}",
                    deviation * 100.0,
                    self.config.deviation_threshold * 100.0,
                    self.current_ceiling,
                ),
            }
        } else {
            // Slowly grow ceiling back toward initial
            if self.current_ceiling < self.config.initial_ceiling {
                self.current_ceiling = (self.current_ceiling
                    + self.config.growth_rate_per_block)
                    .min(self.config.initial_ceiling);
            }
            BreakerAction::None
        }
    }

    /// Check if new minting would exceed the ceiling.
    pub fn can_mint(&self, current_total_debt: f64, new_debt: f64) -> bool {
        current_total_debt + new_debt <= self.current_ceiling
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Combined Circuit Breaker Engine
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug)]
pub struct CircuitBreakerEngine {
    pub twap_breaker: TwapBreaker,
    pub cascade_breaker: CascadeBreaker,
    pub debt_ceiling: DebtCeiling,
    pub minting_paused_until: u64,
    pub halted_until: u64,
}

impl CircuitBreakerEngine {
    pub fn new(
        twap_config: TwapBreakerConfig,
        cascade_config: CascadeBreakerConfig,
        ceiling_config: DebtCeilingConfig,
    ) -> Self {
        CircuitBreakerEngine {
            twap_breaker: TwapBreaker::new(twap_config),
            cascade_breaker: CascadeBreaker::new(cascade_config),
            debt_ceiling: DebtCeiling::new(ceiling_config),
            minting_paused_until: 0,
            halted_until: 0,
        }
    }

    /// Run all breaker checks for a block. Returns all triggered actions.
    pub fn check_all(
        &mut self,
        amm: &Amm,
        registry: &VaultRegistry,
        redemption_price: f64,
        block: u64,
    ) -> Vec<BreakerAction> {
        let mut actions = Vec::new();

        // TWAP breaker
        let twap_action = self.twap_breaker.check(amm, block);
        if let BreakerAction::PauseMinting { blocks, .. } = &twap_action {
            self.minting_paused_until = self.minting_paused_until.max(block + blocks);
        }
        if twap_action != BreakerAction::None {
            actions.push(twap_action);
        }

        // Cascade breaker
        let cascade_action = self.cascade_breaker.check(block);
        if let BreakerAction::EmergencyHalt { .. } = &cascade_action {
            self.halted_until = self.halted_until.max(block + self.cascade_breaker.config.pause_blocks);
        }
        if cascade_action != BreakerAction::None {
            actions.push(cascade_action);
        }

        // Debt ceiling
        let ceiling_action = self.debt_ceiling.update(amm, registry, redemption_price);
        if ceiling_action != BreakerAction::None {
            actions.push(ceiling_action);
        }

        actions
    }

    pub fn is_minting_paused(&self, block: u64) -> bool {
        block < self.minting_paused_until
    }

    pub fn is_halted(&self, block: u64) -> bool {
        block < self.halted_until
    }

    pub fn record_liquidations(&mut self, block: u64, count: u32) {
        self.cascade_breaker.record_liquidations(block, count);
    }
}
