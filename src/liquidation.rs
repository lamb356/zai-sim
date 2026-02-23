use crate::amm::Amm;
use crate::cdp::VaultRegistry;

#[derive(Debug, Clone)]
pub struct LiquidationConfig {
    /// Maximum liquidations allowed per block
    pub max_liquidations_per_block: u32,
    /// Keeper reward as fraction of liquidation penalty (for challenge_response)
    pub keeper_reward_pct: f64,
    /// Fraction of penalty applied during self-liquidation (0.0 = no penalty)
    pub self_liquidation_penalty_pct: f64,
    /// Fraction of non-keeper penalty routed to LPs via AMM reserve injection (0.0 = none)
    pub liquidation_penalty_to_lps_pct: f64,
    /// Enable graduated (partial) liquidation for warning-zone vaults
    pub graduated_liquidation: bool,
    /// Fraction of vault collateral seized per block during graduated liquidation
    pub graduated_pct_per_block: f64,
    /// CR floor for graduated liquidation — vaults below this get full liquidation
    pub graduated_cr_floor: f64,
}

impl Default for LiquidationConfig {
    fn default() -> Self {
        LiquidationConfig {
            max_liquidations_per_block: 5,
            keeper_reward_pct: 0.50,
            self_liquidation_penalty_pct: 0.0,
            liquidation_penalty_to_lps_pct: 0.0,
            graduated_liquidation: false,
            graduated_pct_per_block: 0.10,
            graduated_cr_floor: 1.5,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum LiquidationMode {
    Transparent,
    SelfLiquidation,
    ChallengeResponse { keeper: String },
    /// Spot-price-based cascading liquidation (death spiral mode)
    AmmLiquidation,
    /// Zombie vault detection: TWAP says safe, spot says liquidate
    ZombieDetection,
    /// Oracle-based liquidation: external price determines eligibility,
    /// collateral sold through AMM (creates death spiral feedback loop)
    OracleLiquidation,
    /// Graduated (partial) liquidation: seize a fraction per block to deleverage
    GraduatedPartial,
}

#[derive(Debug, Clone)]
pub struct LiquidationResult {
    pub vault_id: u64,
    pub owner: String,
    pub mode: LiquidationMode,
    pub collateral_seized: f64,
    pub debt_to_cover: f64,
    pub zai_from_amm: f64,
    pub penalty_amount: f64,
    pub keeper_reward: f64,
    pub surplus_to_owner: f64,
    pub bad_debt: f64,
    pub block: u64,
}

#[derive(Debug)]
pub struct LiquidationEngine {
    pub config: LiquidationConfig,
    pub total_bad_debt: f64,
    pub total_penalties_collected: f64,
    pub total_keeper_rewards: f64,
    pub history: Vec<LiquidationResult>,
    liquidations_this_block: u32,
    current_block: u64,
}

impl LiquidationEngine {
    pub fn new(config: LiquidationConfig) -> Self {
        LiquidationEngine {
            config,
            total_bad_debt: 0.0,
            total_penalties_collected: 0.0,
            total_keeper_rewards: 0.0,
            history: Vec::new(),
            liquidations_this_block: 0,
            current_block: 0,
        }
    }

    /// Reset the per-block counter when advancing to a new block.
    fn advance_block(&mut self, block: u64) {
        if block > self.current_block {
            self.current_block = block;
            self.liquidations_this_block = 0;
        }
    }

    fn check_velocity(&self) -> Result<(), String> {
        if self.liquidations_this_block >= self.config.max_liquidations_per_block {
            return Err(format!(
                "Velocity limit reached: {} liquidations in block {}",
                self.liquidations_this_block, self.current_block
            ));
        }
        Ok(())
    }

    /// Scan all vaults and return IDs of those below min_ratio.
    pub fn scan_liquidatable(&self, registry: &VaultRegistry, amm: &Amm) -> Vec<u64> {
        let mut ids: Vec<u64> = registry
            .vaults
            .iter()
            .filter(|(id, _)| registry.is_liquidatable(**id, amm))
            .map(|(id, _)| *id)
            .collect();
        ids.sort();
        ids
    }

    /// Core liquidation logic shared by all modes.
    /// Seizes collateral, sells on AMM, settles debt.
    #[allow(clippy::too_many_arguments)]
    fn execute_core(
        &mut self,
        vault_id: u64,
        mode: LiquidationMode,
        penalty_fraction: f64,
        keeper_reward_fraction: f64,
        registry: &mut VaultRegistry,
        amm: &mut Amm,
        block: u64,
    ) -> Result<LiquidationResult, String> {
        self.advance_block(block);
        self.check_velocity()?;

        // Accrue fees first
        registry.accrue_fees(vault_id, block)?;

        // For standard modes, vault must be undercollateralized per TWAP
        // AmmLiquidation and ZombieDetection bypass TWAP check (already verified at spot)
        if !matches!(
            mode,
            LiquidationMode::SelfLiquidation
                | LiquidationMode::AmmLiquidation
                | LiquidationMode::ZombieDetection
                | LiquidationMode::OracleLiquidation
                | LiquidationMode::GraduatedPartial
        ) && !registry.is_liquidatable(vault_id, amm)
        {
            return Err(format!("Vault {} is not liquidatable", vault_id));
        }

        // Snapshot vault before removal
        let vault = registry
            .vaults
            .get(&vault_id)
            .ok_or_else(|| format!("Vault {} not found", vault_id))?;

        let collateral_seized = vault.collateral_zec;
        let debt_to_cover = vault.debt_zai;
        let owner = vault.owner.clone();

        if debt_to_cover == 0.0 {
            return Err("Cannot liquidate vault with no debt".to_string());
        }

        // Remove vault from registry and adjust total_debt
        registry.vaults.remove(&vault_id);
        registry.total_debt -= debt_to_cover;

        // Sell seized collateral on AMM
        let zai_from_amm = amm
            .swap_zec_for_zai(collateral_seized, block)
            .unwrap_or(0.0);

        // Calculate penalty
        let penalty_amount = debt_to_cover * penalty_fraction;

        // Settle: debt + penalty must be covered by AMM proceeds
        let total_obligation = debt_to_cover + penalty_amount;

        let (bad_debt, surplus_to_owner, actual_penalty) = if zai_from_amm >= total_obligation {
            // Fully covered: surplus goes to owner
            (0.0, zai_from_amm - total_obligation, penalty_amount)
        } else if zai_from_amm >= debt_to_cover {
            // Debt covered but not full penalty
            let partial_penalty = zai_from_amm - debt_to_cover;
            (0.0, 0.0, partial_penalty)
        } else {
            // Bad debt: AMM proceeds don't cover the debt
            (debt_to_cover - zai_from_amm, 0.0, 0.0)
        };

        // Keeper reward is a fraction of actual penalty collected
        let keeper_reward = actual_penalty * keeper_reward_fraction;

        // Route a share of the non-keeper penalty to LPs via AMM reserves
        let lp_penalty_share =
            (actual_penalty - keeper_reward) * self.config.liquidation_penalty_to_lps_pct;
        if lp_penalty_share > 0.0 {
            amm.reserve_zai += lp_penalty_share;
            amm.k = amm.reserve_zec * amm.reserve_zai;
            amm.cumulative_fees_zai += lp_penalty_share;
        }

        // Update engine state
        self.total_bad_debt += bad_debt;
        self.total_penalties_collected += actual_penalty - keeper_reward - lp_penalty_share;
        self.total_keeper_rewards += keeper_reward;
        self.liquidations_this_block += 1;

        let result = LiquidationResult {
            vault_id,
            owner,
            mode,
            collateral_seized,
            debt_to_cover,
            zai_from_amm,
            penalty_amount: actual_penalty,
            keeper_reward,
            surplus_to_owner,
            bad_debt,
            block,
        };

        self.history.push(result.clone());

        Ok(result)
    }

    /// Transparent liquidation: system auto-scans and liquidates all underwater vaults.
    pub fn transparent_liquidate(
        &mut self,
        registry: &mut VaultRegistry,
        amm: &mut Amm,
        block: u64,
    ) -> Vec<LiquidationResult> {
        let ids = self.scan_liquidatable(registry, amm);
        let mut results = Vec::new();

        for id in ids {
            let penalty_frac = registry.config.liquidation_penalty;
            match self.execute_core(
                id,
                LiquidationMode::Transparent,
                penalty_frac,
                0.0, // no keeper in transparent mode
                registry,
                amm,
                block,
            ) {
                Ok(result) => results.push(result),
                Err(_) => break, // velocity limit hit
            }
        }

        results
    }

    /// Self-liquidation: vault owner voluntarily liquidates to avoid full penalty.
    pub fn self_liquidate(
        &mut self,
        vault_id: u64,
        registry: &mut VaultRegistry,
        amm: &mut Amm,
        block: u64,
    ) -> Result<LiquidationResult, String> {
        // Self-liquidation is allowed even if vault is above min ratio
        // (owner may want to exit during volatile conditions)
        let penalty_frac =
            registry.config.liquidation_penalty * self.config.self_liquidation_penalty_pct;

        self.execute_core(
            vault_id,
            LiquidationMode::SelfLiquidation,
            penalty_frac,
            0.0,
            registry,
            amm,
            block,
        )
    }

    /// Challenge-response: external keeper identifies undercollateralized vault.
    pub fn challenge_liquidate(
        &mut self,
        vault_id: u64,
        keeper: &str,
        registry: &mut VaultRegistry,
        amm: &mut Amm,
        block: u64,
    ) -> Result<LiquidationResult, String> {
        let penalty_frac = registry.config.liquidation_penalty;
        let keeper_frac = self.config.keeper_reward_pct;

        self.execute_core(
            vault_id,
            LiquidationMode::ChallengeResponse {
                keeper: keeper.to_string(),
            },
            penalty_frac,
            keeper_frac,
            registry,
            amm,
            block,
        )
    }

    /// Scan vaults liquidatable at a given price (spot or external).
    pub fn scan_liquidatable_at_price(
        &self,
        registry: &VaultRegistry,
        price: f64,
    ) -> Vec<u64> {
        let mut ids: Vec<u64> = registry
            .vaults
            .iter()
            .filter(|(_, vault)| {
                vault.debt_zai > 0.0
                    && vault.collateral_ratio(price) < registry.config.min_ratio
            })
            .map(|(id, _)| *id)
            .collect();
        ids.sort();
        ids
    }

    /// Cascading spot-price liquidation: uses AMM spot price instead of TWAP.
    /// After each liquidation, re-scans because the AMM sell depresses spot price,
    /// potentially making more vaults liquidatable. Models the death spiral.
    pub fn cascading_spot_liquidate(
        &mut self,
        registry: &mut VaultRegistry,
        amm: &mut Amm,
        block: u64,
    ) -> Vec<LiquidationResult> {
        let mut results = Vec::new();
        loop {
            let spot_price = amm.spot_price();
            let ids = self.scan_liquidatable_at_price(registry, spot_price);
            if ids.is_empty() {
                break;
            }

            let mut any_liquidated = false;
            for id in ids {
                let penalty_frac = registry.config.liquidation_penalty;
                match self.execute_core(
                    id,
                    LiquidationMode::AmmLiquidation,
                    penalty_frac,
                    0.0,
                    registry,
                    amm,
                    block,
                ) {
                    Ok(result) => {
                        results.push(result);
                        any_liquidated = true;
                    }
                    Err(_) => return results, // velocity limit hit
                }
            }
            if !any_liquidated {
                break;
            }
        }
        results
    }

    /// Zombie vault detection: find vaults that look safe by TWAP but are
    /// undercollateralized by spot price, with gap exceeding threshold.
    /// Liquidates them using spot price to prevent delayed bad debt.
    pub fn zombie_detect_and_liquidate(
        &mut self,
        registry: &mut VaultRegistry,
        amm: &mut Amm,
        block: u64,
        gap_threshold: f64,
    ) -> Vec<LiquidationResult> {
        let twap = amm.get_twap(registry.config.twap_window);
        let spot = amm.spot_price();
        let min_ratio = registry.config.min_ratio;

        let zombie_ids: Vec<u64> = registry
            .vaults
            .iter()
            .filter(|(_, vault)| {
                if vault.debt_zai == 0.0 {
                    return false;
                }
                let twap_ratio = vault.collateral_ratio(twap);
                let spot_ratio = vault.collateral_ratio(spot);
                // Zombie: safe by TWAP, unsafe by spot, gap above threshold
                twap_ratio >= min_ratio
                    && spot_ratio < min_ratio
                    && (twap_ratio - spot_ratio) > gap_threshold
            })
            .map(|(id, _)| *id)
            .collect();

        let mut results = Vec::new();
        for id in zombie_ids {
            let penalty_frac = registry.config.liquidation_penalty;
            match self.execute_core(
                id,
                LiquidationMode::ZombieDetection,
                penalty_frac,
                0.0,
                registry,
                amm,
                block,
            ) {
                Ok(result) => results.push(result),
                Err(_) => break,
            }
        }
        results
    }

    /// Oracle-based liquidation: uses an external price for eligibility checks,
    /// but sells seized collateral through the AMM.
    ///
    /// This creates a death spiral feedback loop:
    /// 1. External oracle reports low price → vaults become eligible for liquidation
    /// 2. Seized collateral is dumped on AMM → AMM price drops
    /// 3. Next block: oracle still reports low price → more vaults eligible
    /// 4. Repeat until all vaults are liquidated or velocity limit stops it
    ///
    /// Unlike `cascading_spot_liquidate`, this does NOT re-scan after each
    /// liquidation within a single block — the oracle price is fixed per block.
    /// The cascade happens across blocks as the AMM price deteriorates.
    pub fn oracle_liquidate(
        &mut self,
        registry: &mut VaultRegistry,
        amm: &mut Amm,
        block: u64,
        oracle_price: f64,
    ) -> Vec<LiquidationResult> {
        let ids = self.scan_liquidatable_at_price(registry, oracle_price);
        let mut results = Vec::new();

        for id in ids {
            let penalty_frac = registry.config.liquidation_penalty;
            match self.execute_core(
                id,
                LiquidationMode::OracleLiquidation,
                penalty_frac,
                0.0,
                registry,
                amm,
                block,
            ) {
                Ok(result) => results.push(result),
                Err(_) => break, // velocity limit hit
            }
        }

        results
    }

    /// Scan vaults eligible for graduated (partial) liquidation.
    /// Returns IDs of vaults whose TWAP-based CR is between graduated_cr_floor and min_ratio.
    pub fn scan_graduated_eligible(
        &self,
        registry: &VaultRegistry,
        amm: &Amm,
    ) -> Vec<u64> {
        let twap = amm.get_twap(registry.config.twap_window);
        let min_ratio = registry.config.min_ratio;
        let cr_floor = self.config.graduated_cr_floor;

        let mut ids: Vec<u64> = registry
            .vaults
            .iter()
            .filter(|(_, vault)| {
                if vault.debt_zai <= 0.0 {
                    return false;
                }
                let cr = vault.collateral_ratio(twap);
                cr >= cr_floor && cr < min_ratio
            })
            .map(|(id, _)| *id)
            .collect();
        ids.sort();
        ids
    }

    /// Execute a graduated (partial) liquidation on a single vault.
    /// Seizes `graduated_pct_per_block` of collateral, sells on AMM,
    /// reduces debt by the ZAI received. Vault survives unless debt <= debt_floor.
    fn execute_graduated(
        &mut self,
        vault_id: u64,
        registry: &mut VaultRegistry,
        amm: &mut Amm,
        block: u64,
    ) -> Result<LiquidationResult, String> {
        self.advance_block(block);
        self.check_velocity()?;

        // Accrue fees first
        registry.accrue_fees(vault_id, block)?;

        let vault = registry
            .vaults
            .get(&vault_id)
            .ok_or_else(|| format!("Vault {} not found", vault_id))?;

        if vault.debt_zai <= 0.0 {
            return Err("Cannot liquidate vault with no debt".to_string());
        }

        let pct = self.config.graduated_pct_per_block;
        let collateral_to_seize = vault.collateral_zec * pct;
        let owner = vault.owner.clone();

        // Sell seized collateral on AMM
        let zai_from_amm = amm
            .swap_zec_for_zai(collateral_to_seize, block)
            .unwrap_or(0.0);

        // Split AMM proceeds into debt_covered + penalty
        let penalty_fraction = registry.config.liquidation_penalty;
        let debt_covered = zai_from_amm / (1.0 + penalty_fraction);
        let actual_penalty = zai_from_amm - debt_covered;

        // Cap debt reduction at actual vault debt
        let vault = registry
            .vaults
            .get(&vault_id)
            .ok_or_else(|| format!("Vault {} not found after AMM swap", vault_id))?;
        let debt_reduction = debt_covered.min(vault.debt_zai);
        let bad_debt = if debt_covered < 0.0 { -debt_covered } else { 0.0 };

        // Route penalty share to LPs
        let lp_penalty_share =
            actual_penalty * self.config.liquidation_penalty_to_lps_pct;
        if lp_penalty_share > 0.0 {
            amm.reserve_zai += lp_penalty_share;
            amm.k = amm.reserve_zec * amm.reserve_zai;
            amm.cumulative_fees_zai += lp_penalty_share;
        }

        // Update vault in place
        let vault = registry
            .vaults
            .get_mut(&vault_id)
            .ok_or_else(|| format!("Vault {} disappeared", vault_id))?;
        vault.collateral_zec -= collateral_to_seize;
        vault.debt_zai -= debt_reduction;

        // Update registry total debt
        registry.total_debt -= debt_reduction;

        // If vault debt is at or below floor (or zero), remove it entirely
        if vault.debt_zai <= registry.config.debt_floor || vault.debt_zai <= 0.0 {
            registry.vaults.remove(&vault_id);
        }

        // Update engine state
        self.total_bad_debt += bad_debt;
        self.total_penalties_collected += actual_penalty - lp_penalty_share;
        self.liquidations_this_block += 1;

        let result = LiquidationResult {
            vault_id,
            owner,
            mode: LiquidationMode::GraduatedPartial,
            collateral_seized: collateral_to_seize,
            debt_to_cover: debt_reduction,
            zai_from_amm,
            penalty_amount: actual_penalty,
            keeper_reward: 0.0,
            surplus_to_owner: 0.0,
            bad_debt,
            block,
        };

        self.history.push(result.clone());
        Ok(result)
    }

    /// Graduated liquidation: partially liquidate vaults in the warning zone.
    /// Vaults with CR between graduated_cr_floor and min_ratio are partially
    /// liquidated (graduated_pct_per_block of collateral seized per block).
    pub fn graduated_liquidate(
        &mut self,
        registry: &mut VaultRegistry,
        amm: &mut Amm,
        block: u64,
    ) -> Vec<LiquidationResult> {
        if !self.config.graduated_liquidation {
            return Vec::new();
        }

        let ids = self.scan_graduated_eligible(registry, amm);
        let mut results = Vec::new();

        for id in ids {
            match self.execute_graduated(id, registry, amm, block) {
                Ok(result) => results.push(result),
                Err(_) => break, // velocity limit hit
            }
        }

        results
    }
}
