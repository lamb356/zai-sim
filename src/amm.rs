use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct PriceObservation {
    pub block: u64,
    pub cumulative_price: f64,
    pub spot_price: f64,
}

#[derive(Debug)]
pub struct Amm {
    pub reserve_zec: f64,
    pub reserve_zai: f64,
    pub k: f64,
    pub swap_fee: f64,
    pub total_lp_shares: f64,
    pub lp_shares: HashMap<String, f64>,

    cumulative_price: f64,
    price_observations: Vec<PriceObservation>,
    last_update_block: u64,

    /// Total swap fees collected, denominated in ZAI-equivalent.
    pub cumulative_fees_zai: f64,
}

impl Amm {
    pub fn new(initial_zec: f64, initial_zai: f64, swap_fee: f64) -> Self {
        let k = initial_zec * initial_zai;
        let spot = initial_zai / initial_zec;
        let initial_shares = (initial_zec * initial_zai).sqrt();

        let mut lp_shares = HashMap::new();
        lp_shares.insert("genesis".to_string(), initial_shares);

        let obs = PriceObservation {
            block: 0,
            cumulative_price: 0.0,
            spot_price: spot,
        };

        Amm {
            reserve_zec: initial_zec,
            reserve_zai: initial_zai,
            k,
            swap_fee,
            total_lp_shares: initial_shares,
            lp_shares,
            cumulative_price: 0.0,
            price_observations: vec![obs],
            last_update_block: 0,
            cumulative_fees_zai: 0.0,
        }
    }

    pub fn spot_price(&self) -> f64 {
        self.reserve_zai / self.reserve_zec
    }

    pub fn record_price(&mut self, block: u64) {
        if block <= self.last_update_block {
            return;
        }
        let blocks_elapsed = block - self.last_update_block;
        let spot = self.spot_price();
        self.cumulative_price += spot * blocks_elapsed as f64;

        self.price_observations.push(PriceObservation {
            block,
            cumulative_price: self.cumulative_price,
            spot_price: spot,
        });
        self.last_update_block = block;
    }

    pub fn get_twap(&self, window_blocks: u64) -> f64 {
        if self.price_observations.is_empty() {
            return self.spot_price();
        }

        let current = self.price_observations.last().unwrap();
        let target_block = current.block.saturating_sub(window_blocks);

        // Find the observation at or just before target_block
        let start_obs = self
            .price_observations
            .iter()
            .rev()
            .find(|obs| obs.block <= target_block)
            .unwrap_or(&self.price_observations[0]);

        let cumulative_diff = current.cumulative_price - start_obs.cumulative_price;
        let block_diff = current.block - start_obs.block;

        if block_diff == 0 {
            return current.spot_price;
        }

        cumulative_diff / block_diff as f64
    }

    pub fn swap_zec_for_zai(&mut self, zec_in: f64, block: u64) -> Result<f64, String> {
        if zec_in <= 0.0 {
            return Err("Input must be positive".to_string());
        }

        // Record price before swap
        self.record_price(block);

        // Track fees in ZAI-equivalent terms (pre-swap spot)
        let fee_zai = zec_in * self.swap_fee * self.spot_price();
        self.cumulative_fees_zai += fee_zai;

        let effective_input = zec_in * (1.0 - self.swap_fee);
        let new_reserve_zec = self.reserve_zec + effective_input;
        let new_reserve_zai = self.k / new_reserve_zec;
        let zai_out = self.reserve_zai - new_reserve_zai;

        if zai_out <= 0.0 {
            return Err("Insufficient output".to_string());
        }

        // Update reserves: full input goes in (fee stays in pool)
        self.reserve_zec += zec_in;
        self.reserve_zai -= zai_out;
        // k increases because the fee portion stays in the pool
        self.k = self.reserve_zec * self.reserve_zai;

        Ok(zai_out)
    }

    pub fn swap_zai_for_zec(&mut self, zai_in: f64, block: u64) -> Result<f64, String> {
        if zai_in <= 0.0 {
            return Err("Input must be positive".to_string());
        }

        // Record price before swap
        self.record_price(block);

        // Track fees in ZAI terms
        self.cumulative_fees_zai += zai_in * self.swap_fee;

        let effective_input = zai_in * (1.0 - self.swap_fee);
        let new_reserve_zai = self.reserve_zai + effective_input;
        let new_reserve_zec = self.k / new_reserve_zai;
        let zec_out = self.reserve_zec - new_reserve_zec;

        if zec_out <= 0.0 {
            return Err("Insufficient output".to_string());
        }

        self.reserve_zai += zai_in;
        self.reserve_zec -= zec_out;
        self.k = self.reserve_zec * self.reserve_zai;

        Ok(zec_out)
    }

    pub fn add_liquidity(&mut self, zec: f64, zai: f64, owner: &str) -> Result<f64, String> {
        if zec <= 0.0 || zai <= 0.0 {
            return Err("Amounts must be positive".to_string());
        }

        let shares = if self.total_lp_shares == 0.0 {
            (zec * zai).sqrt()
        } else {
            // Proportional to existing reserves
            let share_zec = (zec / self.reserve_zec) * self.total_lp_shares;
            let share_zai = (zai / self.reserve_zai) * self.total_lp_shares;
            share_zec.min(share_zai)
        };

        self.reserve_zec += zec;
        self.reserve_zai += zai;
        self.k = self.reserve_zec * self.reserve_zai;
        self.total_lp_shares += shares;

        let entry = self.lp_shares.entry(owner.to_string()).or_insert(0.0);
        *entry += shares;

        Ok(shares)
    }

    pub fn remove_liquidity(&mut self, shares: f64, owner: &str) -> Result<(f64, f64), String> {
        let owner_shares = self.lp_shares.get(owner).copied().unwrap_or(0.0);
        if shares > owner_shares {
            return Err(format!(
                "Insufficient shares: have {}, requested {}",
                owner_shares, shares
            ));
        }
        if shares <= 0.0 {
            return Err("Shares must be positive".to_string());
        }

        let fraction = shares / self.total_lp_shares;
        let zec_out = self.reserve_zec * fraction;
        let zai_out = self.reserve_zai * fraction;

        self.reserve_zec -= zec_out;
        self.reserve_zai -= zai_out;
        self.k = self.reserve_zec * self.reserve_zai;
        self.total_lp_shares -= shares;

        let entry = self.lp_shares.get_mut(owner).unwrap();
        *entry -= shares;
        if *entry < 1e-15 {
            self.lp_shares.remove(owner);
        }

        Ok((zec_out, zai_out))
    }

    /// Compute impermanent loss percentage given entry price.
    /// Returns a value <= 0 (e.g., -0.05 means 5% loss from IL).
    pub fn impermanent_loss(&self, entry_price: f64) -> f64 {
        let price_ratio = self.spot_price() / entry_price;
        2.0 * price_ratio.sqrt() / (1.0 + price_ratio) - 1.0
    }

    /// Quote output for selling ZEC without executing the swap.
    pub fn quote_zec_for_zai(&self, zec_in: f64) -> f64 {
        let effective_input = zec_in * (1.0 - self.swap_fee);
        let new_reserve_zec = self.reserve_zec + effective_input;
        let new_reserve_zai = self.k / new_reserve_zec;
        (self.reserve_zai - new_reserve_zai).max(0.0)
    }

    /// Quote output for selling ZAI without executing the swap.
    pub fn quote_zai_for_zec(&self, zai_in: f64) -> f64 {
        let effective_input = zai_in * (1.0 - self.swap_fee);
        let new_reserve_zai = self.reserve_zai + effective_input;
        let new_reserve_zec = self.k / new_reserve_zai;
        (self.reserve_zec - new_reserve_zec).max(0.0)
    }
}
