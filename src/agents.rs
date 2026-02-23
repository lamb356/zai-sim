use std::collections::VecDeque;

use crate::amm::Amm;
use crate::cdp::VaultRegistry;

// ═══════════════════════════════════════════════════════════════════════
// Agent action — returned from each agent's `act()` to describe what happened
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub enum AgentAction {
    None,
    /// Bought ZEC on AMM (sold ZAI)
    BuyZec { zai_spent: f64, zec_received: f64 },
    /// Sold ZEC on AMM (bought ZAI)
    SellZec { zec_spent: f64, zai_received: f64 },
    /// Demand agent bought ZAI on AMM
    BuyZai { zec_spent: f64, zai_received: f64 },
    /// Demand agent panic-sold ZAI on AMM
    PanicSellZai { zai_spent: f64, zec_received: f64 },
    /// Miner sold ZEC on AMM
    MinerSell { zec_sold: f64, zai_received: f64 },
    /// CDP holder took action on their vault
    CdpAction { vault_id: u64, description: String },
    /// LP added liquidity
    LpAdd { zec: f64, zai: f64, shares: f64 },
    /// LP removed liquidity
    LpRemove { zec: f64, zai: f64, shares: f64 },
    /// Attacker manipulated price
    AttackSwap { direction: String, amount: f64 },
    /// Queued action (latency pending)
    Queued { description: String },
}

// ═══════════════════════════════════════════════════════════════════════
// 1. Arbitrageur
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
struct PendingTrade {
    execute_at_block: u64,
    is_buy_zec: bool,
    amount: f64,
}

#[derive(Debug, Clone)]
pub struct ArbitrageurConfig {
    pub initial_zai_balance: f64,
    pub initial_zec_balance: f64,
    pub arb_threshold_pct: f64,
    /// Latency in blocks when buying ZEC on AMM (selling ZAI)
    pub arb_latency_buy_blocks: u64,
    /// Latency in blocks when selling ZEC on AMM (buying ZAI)
    pub arb_latency_sell_blocks: u64,
    /// ZAI replenished per block
    pub capital_replenish_rate: f64,
    /// Minimum expected profit (in ZAI) to execute a trade.
    /// Represents tx fee floor — arber skips trades below this threshold.
    pub min_arb_profit: f64,
    /// Per-arber activity rate (0.0–1.0). Default 1.0 = always active.
    /// When < 1.0, overrides the global arber_activity_rate in ScenarioConfig.
    pub activity_rate: f64,
    /// Fraction of balance to trade per opportunity. Default 0.1 (10%).
    pub max_trade_pct: f64,
}

impl Default for ArbitrageurConfig {
    fn default() -> Self {
        ArbitrageurConfig {
            initial_zai_balance: 100_000.0,
            initial_zec_balance: 2000.0,
            arb_threshold_pct: 0.5,
            arb_latency_buy_blocks: 0,
            arb_latency_sell_blocks: 10,
            capital_replenish_rate: 0.0,
            min_arb_profit: 0.0,
            activity_rate: 1.0,
            max_trade_pct: 0.1,
        }
    }
}

#[derive(Debug)]
pub struct Arbitrageur {
    pub config: ArbitrageurConfig,
    pub zai_balance: f64,
    pub zec_balance: f64,
    pub total_profit_zai: f64,
    pending_trades: VecDeque<PendingTrade>,
}

impl Arbitrageur {
    pub fn new(config: ArbitrageurConfig) -> Self {
        let zai = config.initial_zai_balance;
        let zec = config.initial_zec_balance;
        Arbitrageur {
            config,
            zai_balance: zai,
            zec_balance: zec,
            total_profit_zai: 0.0,
            pending_trades: VecDeque::new(),
        }
    }

    /// Execute any pending trades that have reached their execution block.
    fn execute_pending(&mut self, amm: &mut Amm, block: u64) -> Vec<AgentAction> {
        let mut actions = Vec::new();

        while let Some(front) = self.pending_trades.front() {
            if front.execute_at_block > block {
                break;
            }
            let trade = self.pending_trades.pop_front().unwrap();

            if trade.is_buy_zec {
                // Buy ZEC = sell ZAI on AMM
                let spend = trade.amount.min(self.zai_balance);
                if spend > 0.0 {
                    if let Ok(zec_out) = amm.swap_zai_for_zec(spend, block) {
                        self.zai_balance -= spend;
                        self.zec_balance += zec_out;
                        actions.push(AgentAction::BuyZec {
                            zai_spent: spend,
                            zec_received: zec_out,
                        });
                    }
                }
            } else {
                // Sell ZEC = buy ZAI on AMM
                let spend = trade.amount.min(self.zec_balance);
                if spend > 0.0 {
                    if let Ok(zai_out) = amm.swap_zec_for_zai(spend, block) {
                        self.zec_balance -= spend;
                        self.zai_balance += zai_out;
                        self.total_profit_zai += zai_out - spend * amm.spot_price();
                        actions.push(AgentAction::SellZec {
                            zec_spent: spend,
                            zai_received: zai_out,
                        });
                    }
                }
            }
        }

        actions
    }

    /// Observe prices and decide whether to arb. `external_price` is the
    /// off-chain ZEC/ZAI price (e.g., from Binance).
    pub fn act(
        &mut self,
        amm: &mut Amm,
        external_price: f64,
        block: u64,
    ) -> Vec<AgentAction> {
        // Replenish capital from external sources
        self.zai_balance += self.config.capital_replenish_rate;

        // External market access: when arber is low on ZEC but has ZAI,
        // model buying ZEC on Binance/Coinbase (converting ZAI → ZEC at external price).
        // Limited to capital_replenish_rate per block to bound throughput.
        if self.config.capital_replenish_rate > 0.0
            && self.zec_balance < 10.0
            && self.zai_balance > 0.0
            && external_price > 0.0
        {
            let convert = self.config.capital_replenish_rate.min(self.zai_balance);
            self.zai_balance -= convert;
            self.zec_balance += convert / external_price;
        }

        // Execute any matured pending trades
        let mut actions = self.execute_pending(amm, block);

        let amm_price = amm.spot_price();
        let deviation_pct = ((amm_price - external_price) / external_price) * 100.0;

        if deviation_pct > self.config.arb_threshold_pct {
            // AMM price too high → sell ZEC on AMM (get ZAI) → buy ZEC cheaper externally
            // This pushes AMM price down
            let trade_size = self.zec_balance * self.config.max_trade_pct;
            if trade_size > 0.01 {
                // Profitability check: expected profit must exceed tx fee floor
                let expected_zai = amm.quote_zec_for_zai(trade_size);
                let expected_profit = expected_zai - trade_size * external_price;
                if expected_profit < self.config.min_arb_profit {
                    return actions;
                }

                let latency = self.config.arb_latency_sell_blocks;
                if latency == 0 {
                    // Execute immediately
                    let spend = trade_size.min(self.zec_balance);
                    if let Ok(zai_out) = amm.swap_zec_for_zai(spend, block) {
                        self.zec_balance -= spend;
                        self.zai_balance += zai_out;
                        actions.push(AgentAction::SellZec {
                            zec_spent: spend,
                            zai_received: zai_out,
                        });
                    }
                } else {
                    self.pending_trades.push_back(PendingTrade {
                        execute_at_block: block + latency,
                        is_buy_zec: false,
                        amount: trade_size,
                    });
                    actions.push(AgentAction::Queued {
                        description: format!("sell {} ZEC at block {}", trade_size, block + latency),
                    });
                }
            }
        } else if deviation_pct < -self.config.arb_threshold_pct {
            // AMM price too low → buy ZEC on AMM (sell ZAI) → sell ZEC externally
            // This pushes AMM price up
            let trade_value = self.zai_balance * self.config.max_trade_pct;
            if trade_value > 0.01 {
                // Profitability check: expected profit must exceed tx fee floor
                let expected_zec = amm.quote_zai_for_zec(trade_value);
                let expected_profit = expected_zec * external_price - trade_value;
                if expected_profit < self.config.min_arb_profit {
                    return actions;
                }

                let latency = self.config.arb_latency_buy_blocks;
                if latency == 0 {
                    let spend = trade_value.min(self.zai_balance);
                    if let Ok(zec_out) = amm.swap_zai_for_zec(spend, block) {
                        self.zai_balance -= spend;
                        self.zec_balance += zec_out;
                        actions.push(AgentAction::BuyZec {
                            zai_spent: spend,
                            zec_received: zec_out,
                        });
                    }
                } else {
                    self.pending_trades.push_back(PendingTrade {
                        execute_at_block: block + latency,
                        is_buy_zec: true,
                        amount: trade_value,
                    });
                    actions.push(AgentAction::Queued {
                        description: format!("buy ZEC with {} ZAI at block {}", trade_value, block + latency),
                    });
                }
            }
        }

        actions
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 2. Demand Agent
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct DemandAgentConfig {
    /// Fraction of ZEC balance to spend per 1% discount to par
    pub demand_elasticity: f64,
    /// Base ZAI purchased per block (denominated in ZEC spent)
    pub demand_base_rate: f64,
    /// If ZAI deviates more than this % from par, start exit timer
    pub demand_exit_threshold_pct: f64,
    /// Blocks of sustained deviation before panic sell
    pub demand_exit_window_blocks: u64,
    /// Fraction of ZAI balance to panic sell
    pub demand_panic_sell_fraction: f64,
    pub initial_zec_balance: f64,
}

impl Default for DemandAgentConfig {
    fn default() -> Self {
        DemandAgentConfig {
            demand_elasticity: 0.05,
            demand_base_rate: 1.0,
            demand_exit_threshold_pct: 5.0,
            demand_exit_window_blocks: 48,
            demand_panic_sell_fraction: 0.5,
            initial_zec_balance: 5000.0,
        }
    }
}

#[derive(Debug)]
pub struct DemandAgent {
    pub config: DemandAgentConfig,
    pub zec_balance: f64,
    pub zai_balance: f64,
    deviation_blocks: u64,
    pub panicked: bool,
}

impl DemandAgent {
    pub fn new(config: DemandAgentConfig) -> Self {
        let zec = config.initial_zec_balance;
        DemandAgent {
            config,
            zec_balance: zec,
            zai_balance: 0.0,
            deviation_blocks: 0,
            panicked: false,
        }
    }

    pub fn act(
        &mut self,
        amm: &mut Amm,
        redemption_price: f64,
        block: u64,
    ) -> AgentAction {
        let market_price = amm.spot_price();
        // ZAI price in terms of ZEC: how many ZEC per ZAI
        // If AMM spot = ZAI/ZEC, then ZAI price in ZEC = 1/spot
        // But we work in ZAI/ZEC terms. deviation = (redemption - market) / redemption
        // Positive deviation = ZAI is cheap (below par) = buying opportunity
        let deviation_pct =
            ((redemption_price - market_price) / redemption_price) * 100.0;

        // Check exit condition: sustained deviation beyond threshold
        if deviation_pct.abs() > self.config.demand_exit_threshold_pct {
            self.deviation_blocks += 1;
        } else {
            self.deviation_blocks = 0;
        }

        // Panic sell if deviation sustained too long (only once)
        if !self.panicked
            && self.deviation_blocks >= self.config.demand_exit_window_blocks
            && self.zai_balance > 0.01
        {
            let sell_amount = self.zai_balance * self.config.demand_panic_sell_fraction;
            if sell_amount > 0.01 {
                if let Ok(zec_out) = amm.swap_zai_for_zec(sell_amount, block) {
                    self.zai_balance -= sell_amount;
                    self.zec_balance += zec_out;
                    self.panicked = true;
                    return AgentAction::PanicSellZai {
                        zai_spent: sell_amount,
                        zec_received: zec_out,
                    };
                }
            }
        }

        // Normal buying: base rate + elasticity bonus when ZAI is cheap
        let mut buy_amount_zec = self.config.demand_base_rate;

        if deviation_pct > 0.0 {
            // ZAI below par → buying opportunity
            buy_amount_zec += self.zec_balance * self.config.demand_elasticity
                * (deviation_pct / 100.0);
        }

        buy_amount_zec = buy_amount_zec.min(self.zec_balance);

        if buy_amount_zec > 0.01 {
            if let Ok(zai_out) = amm.swap_zec_for_zai(buy_amount_zec, block) {
                self.zec_balance -= buy_amount_zec;
                self.zai_balance += zai_out;
                return AgentAction::BuyZai {
                    zec_spent: buy_amount_zec,
                    zai_received: zai_out,
                };
            }
        }

        AgentAction::None
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 3. Miner Agent
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct MinerAgentConfig {
    /// ZEC received per block (block reward)
    pub block_reward: f64,
    /// Fraction of reward to sell
    pub miner_sell_fraction: f64,
    /// Fraction of sell that goes through AMM (rest is off-chain)
    pub miner_amm_fraction: f64,
    /// true = sell immediately each block, false = batch
    pub sell_immediately: bool,
    /// Blocks between batch sells (only used when sell_immediately=false)
    pub batch_interval: u64,
}

impl Default for MinerAgentConfig {
    fn default() -> Self {
        MinerAgentConfig {
            block_reward: 1.25,
            miner_sell_fraction: 0.5,
            miner_amm_fraction: 0.3,
            sell_immediately: true,
            batch_interval: 48,
        }
    }
}

#[derive(Debug)]
pub struct MinerAgent {
    pub config: MinerAgentConfig,
    pub zec_balance: f64,
    pub zai_balance: f64,
    accumulated_sell: f64,
    last_batch_block: u64,
}

impl MinerAgent {
    pub fn new(config: MinerAgentConfig) -> Self {
        MinerAgent {
            config,
            zec_balance: 0.0,
            zai_balance: 0.0,
            accumulated_sell: 0.0,
            last_batch_block: 0,
        }
    }

    pub fn act(&mut self, amm: &mut Amm, block: u64) -> AgentAction {
        // Receive block reward
        self.zec_balance += self.config.block_reward;

        let sell_total = self.config.block_reward * self.config.miner_sell_fraction;
        let amm_sell = sell_total * self.config.miner_amm_fraction;

        if self.config.sell_immediately {
            if amm_sell > 0.001 {
                if let Ok(zai_out) = amm.swap_zec_for_zai(amm_sell, block) {
                    self.zec_balance -= amm_sell;
                    self.zai_balance += zai_out;
                    return AgentAction::MinerSell {
                        zec_sold: amm_sell,
                        zai_received: zai_out,
                    };
                }
            }
        } else {
            self.accumulated_sell += amm_sell;
            if block >= self.last_batch_block + self.config.batch_interval
                && self.accumulated_sell > 0.001
            {
                let batch = self.accumulated_sell.min(self.zec_balance);
                self.accumulated_sell = 0.0;
                self.last_batch_block = block;
                if let Ok(zai_out) = amm.swap_zec_for_zai(batch, block) {
                    self.zec_balance -= batch;
                    self.zai_balance += zai_out;
                    return AgentAction::MinerSell {
                        zec_sold: batch,
                        zai_received: zai_out,
                    };
                }
            }
        }

        AgentAction::None
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 4. CDP Holder
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct CdpHolderConfig {
    /// Collateral ratio the holder targets (e.g., 2.0 = 200%)
    pub target_ratio: f64,
    /// Ratio at which holder takes action to improve (e.g., 1.8 = 180%)
    pub action_threshold_ratio: f64,
    /// ZEC available to add as collateral
    pub reserve_zec: f64,
    /// Initial collateral to deposit
    pub initial_collateral: f64,
    /// Initial debt to draw
    pub initial_debt: f64,
}

impl Default for CdpHolderConfig {
    fn default() -> Self {
        CdpHolderConfig {
            target_ratio: 2.5,
            action_threshold_ratio: 1.8,
            reserve_zec: 100.0,
            initial_collateral: 50.0,
            initial_debt: 1000.0,
        }
    }
}

#[derive(Debug)]
pub struct CdpHolder {
    pub config: CdpHolderConfig,
    pub vault_id: Option<u64>,
    pub reserve_zec: f64,
}

impl CdpHolder {
    pub fn new(config: CdpHolderConfig) -> Self {
        let reserve = config.reserve_zec;
        CdpHolder {
            config,
            vault_id: None,
            reserve_zec: reserve,
        }
    }

    /// Open the initial vault. Call once at simulation start.
    pub fn open_vault(
        &mut self,
        registry: &mut VaultRegistry,
        amm: &Amm,
        block: u64,
    ) -> Result<u64, String> {
        let id = registry.open_vault(
            "cdp_holder",
            self.config.initial_collateral,
            self.config.initial_debt,
            block,
            amm,
        )?;
        self.vault_id = Some(id);
        Ok(id)
    }

    /// Monitor vault and take protective action if ratio drops.
    pub fn act(
        &mut self,
        registry: &mut VaultRegistry,
        amm: &Amm,
        _block: u64,
    ) -> AgentAction {
        let vault_id = match self.vault_id {
            Some(id) => id,
            None => return AgentAction::None,
        };

        // Check if vault still exists
        let price = amm.get_twap(registry.config.twap_window);
        let vault = match registry.get_vault(vault_id) {
            Some(v) => v,
            None => {
                self.vault_id = None;
                return AgentAction::None;
            }
        };

        let ratio = vault.collateral_ratio(price);

        if ratio < self.config.action_threshold_ratio && ratio > 0.0 {
            // Try to add collateral first
            if self.reserve_zec > 0.0 {
                // How much ZEC needed to reach target ratio?
                // target = (collateral + add) * price / debt
                // add = (target * debt / price) - collateral
                let needed = (self.config.target_ratio * vault.debt_zai / price)
                    - vault.collateral_zec;
                let add_amount = needed.max(0.0).min(self.reserve_zec);

                if add_amount > 0.01 {
                    self.reserve_zec -= add_amount;
                    if registry.deposit_collateral(vault_id, add_amount).is_ok() {
                        return AgentAction::CdpAction {
                            vault_id,
                            description: format!("added {:.2} ZEC collateral", add_amount),
                        };
                    }
                }
            }

            // If can't add collateral, try to repay debt
            // (would need ZAI balance — simplified: just report)
            return AgentAction::CdpAction {
                vault_id,
                description: format!("ratio low ({:.2}), no reserves to add", ratio),
            };
        }

        AgentAction::None
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 5. LP Agent
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct LpAgentConfig {
    pub initial_zec: f64,
    pub initial_zai: f64,
    /// Withdraw if impermanent loss exceeds this fraction
    pub il_threshold: f64,
    /// Withdraw if price volatility exceeds this per-block measure
    pub volatility_threshold: f64,
}

impl Default for LpAgentConfig {
    fn default() -> Self {
        LpAgentConfig {
            initial_zec: 500.0,
            initial_zai: 25000.0,
            il_threshold: 0.05,
            volatility_threshold: 0.10,
        }
    }
}

#[derive(Debug)]
pub struct LpAgent {
    pub config: LpAgentConfig,
    pub shares: f64,
    pub zec_balance: f64,
    pub zai_balance: f64,
    /// Price at which liquidity was added (for IL calculation)
    entry_price: f64,
    pub is_providing: bool,
}

impl LpAgent {
    pub fn new(config: LpAgentConfig) -> Self {
        LpAgent {
            config,
            shares: 0.0,
            zec_balance: 0.0,
            zai_balance: 0.0,
            entry_price: 0.0,
            is_providing: false,
        }
    }

    /// Add liquidity to the AMM.
    pub fn provide_liquidity(&mut self, amm: &mut Amm) -> AgentAction {
        let zec = self.config.initial_zec;
        let zai = self.config.initial_zai;

        match amm.add_liquidity(zec, zai, "lp_agent") {
            Ok(shares) => {
                self.shares = shares;
                self.entry_price = amm.spot_price();
                self.is_providing = true;
                AgentAction::LpAdd { zec, zai, shares }
            }
            Err(_) => AgentAction::None,
        }
    }

    /// Monitor and potentially withdraw liquidity.
    pub fn act(&mut self, amm: &mut Amm) -> AgentAction {
        if !self.is_providing || self.shares <= 0.0 {
            return AgentAction::None;
        }

        let current_price = amm.spot_price();

        // Impermanent loss calculation:
        // IL = 2 * sqrt(price_ratio) / (1 + price_ratio) - 1
        let price_ratio = current_price / self.entry_price;
        let il = 2.0 * price_ratio.sqrt() / (1.0 + price_ratio) - 1.0;

        if il.abs() > self.config.il_threshold {
            // Withdraw
            return self.withdraw(amm);
        }

        AgentAction::None
    }

    fn withdraw(&mut self, amm: &mut Amm) -> AgentAction {
        if let Ok((zec, zai)) = amm.remove_liquidity(self.shares, "lp_agent") {
            let shares = self.shares;
            self.zec_balance += zec;
            self.zai_balance += zai;
            self.shares = 0.0;
            self.is_providing = false;
            AgentAction::LpRemove { zec, zai, shares }
        } else {
            AgentAction::None
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 6. IL-Aware LP Agent
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct IlAwareLpConfig {
    pub initial_zec: f64,
    pub initial_zai: f64,
    /// Net P&L threshold (fraction of initial value) below which LP starts withdrawing.
    /// Default: -0.02 (-2%)
    pub withdrawal_threshold: f64,
    /// Fraction of remaining position to withdraw each block when below threshold.
    /// Default: 0.10 (10%)
    pub withdrawal_rate: f64,
}

impl Default for IlAwareLpConfig {
    fn default() -> Self {
        IlAwareLpConfig {
            initial_zec: 10000.0,
            initial_zai: 500000.0,
            withdrawal_threshold: -0.02,
            withdrawal_rate: 0.10,
        }
    }
}

#[derive(Debug)]
pub struct IlAwareLpAgent {
    pub config: IlAwareLpConfig,
    pub shares: f64,
    pub initial_shares: f64,
    pub entry_price: f64,
    pub entry_value: f64,
    pub owner: String,
    pub is_providing: bool,
    pub fees_earned_zai: f64,
    last_cumulative_fees: f64,
    pub withdrawn_zec: f64,
    pub withdrawn_zai: f64,
}

impl IlAwareLpAgent {
    pub fn new(config: IlAwareLpConfig, owner: &str) -> Self {
        IlAwareLpAgent {
            config,
            shares: 0.0,
            initial_shares: 0.0,
            entry_price: 0.0,
            entry_value: 0.0,
            owner: owner.to_string(),
            is_providing: false,
            fees_earned_zai: 0.0,
            last_cumulative_fees: 0.0,
            withdrawn_zec: 0.0,
            withdrawn_zai: 0.0,
        }
    }

    /// Add liquidity to the AMM.
    pub fn provide_liquidity(&mut self, amm: &mut Amm) -> AgentAction {
        let zec = self.config.initial_zec;
        let zai = self.config.initial_zai;

        match amm.add_liquidity(zec, zai, &self.owner) {
            Ok(shares) => {
                self.shares = shares;
                self.initial_shares = shares;
                self.entry_price = amm.spot_price();
                self.entry_value = zec * self.entry_price + zai;
                self.is_providing = true;
                self.last_cumulative_fees = amm.cumulative_fees_zai;
                AgentAction::LpAdd { zec, zai, shares }
            }
            Err(_) => AgentAction::None,
        }
    }

    /// Monitor net P&L and gradually withdraw if losing money.
    /// Uses external_price to value the position (LP checks Binance to see real P&L).
    pub fn act(&mut self, amm: &mut Amm, external_price: f64) -> AgentAction {
        if !self.is_providing || self.shares <= 0.001 {
            return AgentAction::None;
        }

        // Track fee earnings since last check
        let fee_delta = amm.cumulative_fees_zai - self.last_cumulative_fees;
        if fee_delta > 0.0 {
            let my_share_frac = self.shares / amm.total_lp_shares;
            self.fees_earned_zai += fee_delta * my_share_frac;
        }
        self.last_cumulative_fees = amm.cumulative_fees_zai;

        // Compute real P&L using external price
        // What LP's pool shares are worth at external market prices
        let pool_frac = self.shares / amm.total_lp_shares;
        let zec_in_pool = amm.reserve_zec * pool_frac;
        let zai_in_pool = amm.reserve_zai * pool_frac;
        let pool_value = zec_in_pool * external_price + zai_in_pool;

        // Net P&L: (current pool value + fees earned) vs initial value
        let net_pnl_pct = (pool_value + self.fees_earned_zai - self.entry_value)
            / self.entry_value;

        if net_pnl_pct < self.config.withdrawal_threshold {
            // Withdraw a fraction of remaining position
            let withdraw_shares = self.shares * self.config.withdrawal_rate;
            if withdraw_shares > 0.001 {
                if let Ok((zec, zai)) = amm.remove_liquidity(withdraw_shares, &self.owner) {
                    self.shares -= withdraw_shares;
                    self.withdrawn_zec += zec;
                    self.withdrawn_zai += zai;
                    if self.shares < 0.001 {
                        self.is_providing = false;
                    }
                    return AgentAction::LpRemove {
                        zec,
                        zai,
                        shares: withdraw_shares,
                    };
                }
            }
        }

        AgentAction::None
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 7. Attacker
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, PartialEq)]
pub enum AttackPhase {
    Idle,
    Manipulating { revert_at_block: u64 },
    Done,
}

#[derive(Debug, Clone)]
pub struct AttackerConfig {
    /// ZEC capital available for the attack
    pub attack_capital_zec: f64,
    /// How many blocks to hold the manipulated position
    pub hold_blocks: u64,
    /// Block at which to begin the attack
    pub attack_at_block: u64,
}

impl Default for AttackerConfig {
    fn default() -> Self {
        AttackerConfig {
            attack_capital_zec: 5000.0,
            hold_blocks: 3,
            attack_at_block: 100,
        }
    }
}

#[derive(Debug)]
pub struct Attacker {
    pub config: AttackerConfig,
    pub phase: AttackPhase,
    pub zec_balance: f64,
    pub zai_balance: f64,
    zai_received_from_attack: f64,
}

impl Attacker {
    pub fn new(config: AttackerConfig) -> Self {
        let zec = config.attack_capital_zec;
        Attacker {
            config,
            phase: AttackPhase::Idle,
            zec_balance: zec,
            zai_balance: 0.0,
            zai_received_from_attack: 0.0,
        }
    }

    pub fn act(&mut self, amm: &mut Amm, block: u64) -> AgentAction {
        match &self.phase {
            AttackPhase::Idle => {
                if block >= self.config.attack_at_block {
                    // Phase 1: dump ZEC on AMM to crash price
                    let spend = self.zec_balance;
                    if let Ok(zai_out) = amm.swap_zec_for_zai(spend, block) {
                        self.zec_balance = 0.0;
                        self.zai_balance += zai_out;
                        self.zai_received_from_attack = zai_out;
                        self.phase = AttackPhase::Manipulating {
                            revert_at_block: block + self.config.hold_blocks,
                        };
                        return AgentAction::AttackSwap {
                            direction: "sell_zec".to_string(),
                            amount: spend,
                        };
                    }
                }
                AgentAction::None
            }
            AttackPhase::Manipulating { revert_at_block } => {
                if block >= *revert_at_block {
                    // Phase 2: buy back ZEC with the ZAI received
                    let spend = self.zai_received_from_attack.min(self.zai_balance);
                    if let Ok(zec_out) = amm.swap_zai_for_zec(spend, block) {
                        self.zai_balance -= spend;
                        self.zec_balance += zec_out;
                        self.phase = AttackPhase::Done;
                        return AgentAction::AttackSwap {
                            direction: "buy_zec".to_string(),
                            amount: spend,
                        };
                    }
                }
                AgentAction::None
            }
            AttackPhase::Done => AgentAction::None,
        }
    }
}
