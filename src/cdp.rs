use std::collections::HashMap;

use crate::amm::Amm;

/// 75-second blocks → blocks per year
const BLOCKS_PER_YEAR: f64 = 365.25 * 24.0 * 3600.0 / 75.0; // ~420,768

#[derive(Debug, Clone)]
pub struct CdpConfig {
    /// Minimum collateral ratio (e.g., 1.5 = 150%)
    pub min_ratio: f64,
    /// Liquidation penalty (e.g., 0.13 = 13%)
    pub liquidation_penalty: f64,
    /// Minimum debt per vault in ZAI
    pub debt_floor: f64,
    /// Annual stability fee rate (e.g., 0.02 = 2%)
    pub stability_fee_rate: f64,
    /// TWAP window in blocks for collateral valuation
    pub twap_window: u64,
}

impl Default for CdpConfig {
    fn default() -> Self {
        CdpConfig {
            min_ratio: 1.5,
            liquidation_penalty: 0.13,
            debt_floor: 100.0,
            stability_fee_rate: 0.02,
            twap_window: 48, // ~1 hour at 75s blocks
        }
    }
}

#[derive(Debug, Clone)]
pub struct Vault {
    pub id: u64,
    pub owner: String,
    pub collateral_zec: f64,
    pub debt_zai: f64,
    pub last_fee_block: u64,
    pub created_block: u64,
}

impl Vault {
    /// Collateral ratio = (collateral * price) / debt
    pub fn collateral_ratio(&self, zec_price: f64) -> f64 {
        if self.debt_zai == 0.0 {
            return f64::INFINITY;
        }
        (self.collateral_zec * zec_price) / self.debt_zai
    }
}

#[derive(Debug)]
pub struct VaultRegistry {
    pub vaults: HashMap<u64, Vault>,
    pub config: CdpConfig,
    next_id: u64,
    pub total_debt: f64,
}

impl VaultRegistry {
    pub fn new(config: CdpConfig) -> Self {
        VaultRegistry {
            vaults: HashMap::new(),
            config,
            next_id: 1,
            total_debt: 0.0,
        }
    }

    /// Get ZEC price from AMM TWAP.
    fn get_price(&self, amm: &Amm) -> f64 {
        amm.get_twap(self.config.twap_window)
    }

    /// Accrue stability fee on a vault. Compounds per-block.
    /// debt_new = debt_old * (1 + annual_rate / blocks_per_year) ^ blocks_elapsed
    pub fn accrue_fees(&mut self, vault_id: u64, block: u64) -> Result<(), String> {
        let vault = self
            .vaults
            .get_mut(&vault_id)
            .ok_or_else(|| format!("Vault {} not found", vault_id))?;

        if block <= vault.last_fee_block {
            return Ok(());
        }

        let blocks_elapsed = block - vault.last_fee_block;
        let rate_per_block = self.config.stability_fee_rate / BLOCKS_PER_YEAR;
        let multiplier = (1.0 + rate_per_block).powi(blocks_elapsed as i32);

        let old_debt = vault.debt_zai;
        vault.debt_zai *= multiplier;
        vault.last_fee_block = block;

        self.total_debt += vault.debt_zai - old_debt;

        Ok(())
    }

    /// Open a new vault with collateral and debt.
    pub fn open_vault(
        &mut self,
        owner: &str,
        collateral_zec: f64,
        debt_zai: f64,
        block: u64,
        amm: &Amm,
    ) -> Result<u64, String> {
        if collateral_zec <= 0.0 {
            return Err("Collateral must be positive".to_string());
        }
        if debt_zai < 0.0 {
            return Err("Debt cannot be negative".to_string());
        }

        // Check debt floor (zero debt is allowed — collateral-only vault)
        if debt_zai > 0.0 && debt_zai < self.config.debt_floor {
            return Err(format!(
                "Debt {} below floor {}",
                debt_zai, self.config.debt_floor
            ));
        }

        // Check collateral ratio
        if debt_zai > 0.0 {
            let price = self.get_price(amm);
            let ratio = (collateral_zec * price) / debt_zai;
            if ratio < self.config.min_ratio {
                return Err(format!(
                    "Collateral ratio {:.4} below minimum {:.4}",
                    ratio, self.config.min_ratio
                ));
            }
        }

        let id = self.next_id;
        self.next_id += 1;

        let vault = Vault {
            id,
            owner: owner.to_string(),
            collateral_zec,
            debt_zai,
            last_fee_block: block,
            created_block: block,
        };

        self.vaults.insert(id, vault);
        self.total_debt += debt_zai;

        Ok(id)
    }

    /// Close a vault — repay all debt, return all collateral.
    /// Returns (collateral_returned, total_debt_owed) including accrued fees.
    pub fn close_vault(
        &mut self,
        vault_id: u64,
        block: u64,
    ) -> Result<(f64, f64), String> {
        self.accrue_fees(vault_id, block)?;

        let vault = self
            .vaults
            .remove(&vault_id)
            .ok_or_else(|| format!("Vault {} not found", vault_id))?;

        self.total_debt -= vault.debt_zai;

        Ok((vault.collateral_zec, vault.debt_zai))
    }

    /// Deposit additional collateral into a vault.
    pub fn deposit_collateral(
        &mut self,
        vault_id: u64,
        amount: f64,
    ) -> Result<(), String> {
        if amount <= 0.0 {
            return Err("Amount must be positive".to_string());
        }

        let vault = self
            .vaults
            .get_mut(&vault_id)
            .ok_or_else(|| format!("Vault {} not found", vault_id))?;

        vault.collateral_zec += amount;
        Ok(())
    }

    /// Withdraw collateral from a vault. Checks min ratio after withdrawal.
    pub fn withdraw_collateral(
        &mut self,
        vault_id: u64,
        amount: f64,
        block: u64,
        amm: &Amm,
    ) -> Result<(), String> {
        if amount <= 0.0 {
            return Err("Amount must be positive".to_string());
        }

        self.accrue_fees(vault_id, block)?;

        let price = self.get_price(amm);

        let vault = self
            .vaults
            .get_mut(&vault_id)
            .ok_or_else(|| format!("Vault {} not found", vault_id))?;

        if amount > vault.collateral_zec {
            return Err(format!(
                "Insufficient collateral: have {}, requested {}",
                vault.collateral_zec, amount
            ));
        }

        let new_collateral = vault.collateral_zec - amount;

        // Check ratio if there's outstanding debt
        if vault.debt_zai > 0.0 {
            let new_ratio = (new_collateral * price) / vault.debt_zai;
            if new_ratio < self.config.min_ratio {
                return Err(format!(
                    "Withdrawal would drop ratio to {:.4}, below minimum {:.4}",
                    new_ratio, self.config.min_ratio
                ));
            }
        }

        vault.collateral_zec = new_collateral;
        Ok(())
    }

    /// Borrow additional ZAI against existing collateral.
    pub fn borrow_zai(
        &mut self,
        vault_id: u64,
        amount: f64,
        block: u64,
        amm: &Amm,
    ) -> Result<(), String> {
        if amount <= 0.0 {
            return Err("Amount must be positive".to_string());
        }

        self.accrue_fees(vault_id, block)?;

        let price = self.get_price(amm);

        let vault = self
            .vaults
            .get_mut(&vault_id)
            .ok_or_else(|| format!("Vault {} not found", vault_id))?;

        let new_debt = vault.debt_zai + amount;

        // Check debt floor
        if new_debt < self.config.debt_floor {
            return Err(format!(
                "Total debt {} would be below floor {}",
                new_debt, self.config.debt_floor
            ));
        }

        // Check collateral ratio
        let new_ratio = (vault.collateral_zec * price) / new_debt;
        if new_ratio < self.config.min_ratio {
            return Err(format!(
                "Borrow would drop ratio to {:.4}, below minimum {:.4}",
                new_ratio, self.config.min_ratio
            ));
        }

        self.total_debt += amount;
        vault.debt_zai = new_debt;
        Ok(())
    }

    /// Repay ZAI debt. Full repayment (to zero) is always allowed.
    /// Partial repayment must not leave debt below the floor.
    pub fn repay_zai(
        &mut self,
        vault_id: u64,
        amount: f64,
        block: u64,
    ) -> Result<(), String> {
        if amount <= 0.0 {
            return Err("Amount must be positive".to_string());
        }

        self.accrue_fees(vault_id, block)?;

        let vault = self
            .vaults
            .get_mut(&vault_id)
            .ok_or_else(|| format!("Vault {} not found", vault_id))?;

        if amount > vault.debt_zai {
            return Err(format!(
                "Repayment {} exceeds debt {}",
                amount, vault.debt_zai
            ));
        }

        let new_debt = vault.debt_zai - amount;

        // Partial repayment must respect debt floor (full repay to 0 is fine)
        if new_debt > 0.0 && new_debt < self.config.debt_floor {
            return Err(format!(
                "Partial repayment would leave debt {} below floor {}. Repay fully or leave above floor.",
                new_debt, self.config.debt_floor
            ));
        }

        self.total_debt -= amount;
        vault.debt_zai = new_debt;
        Ok(())
    }

    /// Accrue stability fees on all vaults and return the total fee delta in ZAI.
    pub fn accrue_all_fees(&mut self, block: u64) -> f64 {
        let vault_ids: Vec<u64> = self.vaults.keys().copied().collect();
        let mut total_fees = 0.0;
        for id in vault_ids {
            let old_debt = self.vaults[&id].debt_zai;
            let _ = self.accrue_fees(id, block);
            total_fees += self.vaults[&id].debt_zai - old_debt;
        }
        total_fees
    }

    /// Check if a vault is liquidatable (ratio below min_ratio).
    pub fn is_liquidatable(&self, vault_id: u64, amm: &Amm) -> bool {
        let vault = match self.vaults.get(&vault_id) {
            Some(v) => v,
            None => return false,
        };

        if vault.debt_zai == 0.0 {
            return false;
        }

        let price = self.get_price(amm);
        vault.collateral_ratio(price) < self.config.min_ratio
    }

    /// Calculate the liquidation penalty amount for a vault.
    pub fn liquidation_penalty_amount(&self, vault_id: u64) -> Option<f64> {
        let vault = self.vaults.get(&vault_id)?;
        Some(vault.debt_zai * self.config.liquidation_penalty)
    }

    /// Get a vault by ID (immutable).
    pub fn get_vault(&self, vault_id: u64) -> Option<&Vault> {
        self.vaults.get(&vault_id)
    }
}
