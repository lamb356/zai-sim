/// Stability controller: adjusts redemption_price via redemption_rate
/// based on the deviation between market_price and redemption_price.
///
/// Two modes:
/// - PI: proportional + integral with anti-windup clamping
/// - Tick: Rico-style integral-only, log-scale, with sensitivity parameter

#[derive(Debug, Clone)]
pub enum ControllerMode {
    /// Classic PI controller with proportional and integral gains.
    PI {
        /// Proportional gain (immediate response to error)
        kp: f64,
        /// Integral gain (accumulated error over time)
        ki: f64,
    },
    /// Rico-style integral-only controller operating in log space.
    Tick {
        /// How much the rate moves per unit of log-deviation per block
        sensitivity: f64,
    },
}

#[derive(Debug, Clone)]
pub struct ControllerConfig {
    pub mode: ControllerMode,
    /// Minimum redemption rate per block (negative = price falling)
    pub min_rate: f64,
    /// Maximum redemption rate per block (positive = price rising)
    pub max_rate: f64,
    /// Anti-windup: lower bound on the integral accumulator (PI mode)
    pub integral_min: f64,
    /// Anti-windup: upper bound on the integral accumulator (PI mode)
    pub integral_max: f64,
}

impl ControllerConfig {
    /// Default PI controller config.
    pub fn default_pi() -> Self {
        ControllerConfig {
            mode: ControllerMode::PI {
                kp: 2e-7,
                ki: 5e-9,
            },
            // Per-block rate bounds (75s blocks, ~420768 blocks/year)
            // ±0.0001 per block ≈ ±4.2% per year
            min_rate: -1e-4,
            max_rate: 1e-4,
            integral_min: -1e-4,
            integral_max: 1e-4,
        }
    }

    /// Default Tick (Rico-style) controller config.
    pub fn default_tick() -> Self {
        ControllerConfig {
            mode: ControllerMode::Tick {
                sensitivity: 1e-7,
            },
            min_rate: -1e-4,
            max_rate: 1e-4,
            integral_min: -1e-4,
            integral_max: 1e-4,
        }
    }
}

#[derive(Debug)]
pub struct Controller {
    pub config: ControllerConfig,
    /// Target price of ZAI in USD
    pub redemption_price: f64,
    /// Per-block rate of change of redemption_price
    pub redemption_rate: f64,
    /// Accumulated integral term
    pub integral: f64,
    /// Last block the controller was updated
    pub last_block: u64,
}

impl Controller {
    pub fn new(config: ControllerConfig, initial_redemption_price: f64, start_block: u64) -> Self {
        Controller {
            config,
            redemption_price: initial_redemption_price,
            redemption_rate: 0.0,
            integral: 0.0,
            last_block: start_block,
        }
    }

    /// Advance redemption_price to the given block using current redemption_rate.
    /// redemption_price *= (1 + redemption_rate) ^ blocks_elapsed
    pub fn step(&mut self, block: u64) {
        if block <= self.last_block {
            return;
        }
        let blocks_elapsed = block - self.last_block;
        self.redemption_price *= (1.0 + self.redemption_rate).powi(blocks_elapsed as i32);
        self.last_block = block;
    }

    /// Compute error signal and update redemption_rate.
    /// Call this after step() to use the current redemption_price.
    ///
    /// Returns the new redemption_rate.
    pub fn update(&mut self, market_price: f64, block: u64) -> f64 {
        // First advance redemption_price to current block
        self.step(block);

        match self.config.mode {
            ControllerMode::PI { kp, ki } => self.update_pi(market_price, kp, ki),
            ControllerMode::Tick { sensitivity } => self.update_tick(market_price, sensitivity),
        }
    }

    /// PI controller update.
    ///
    /// deviation = (market - target) / target
    /// Negative feedback: when market > target, push rate down.
    fn update_pi(&mut self, market_price: f64, kp: f64, ki: f64) -> f64 {
        let deviation = (market_price - self.redemption_price) / self.redemption_price;

        // Proportional term: immediate negative feedback
        let p_term = -kp * deviation;

        // Integral term: accumulate error (negative feedback)
        self.integral += -ki * deviation;

        // Anti-windup: clamp integral
        self.integral = self
            .integral
            .clamp(self.config.integral_min, self.config.integral_max);

        // Combined rate
        let raw_rate = p_term + self.integral;
        self.redemption_rate = raw_rate.clamp(self.config.min_rate, self.config.max_rate);

        self.redemption_rate
    }

    /// Tick (Rico-style) controller update.
    ///
    /// error_log = ln(market / target)
    /// Integral-only with negative feedback on log scale.
    fn update_tick(&mut self, market_price: f64, sensitivity: f64) -> f64 {
        let error_log = (market_price / self.redemption_price).ln();

        // Integral accumulates with negative feedback
        self.integral += -sensitivity * error_log;

        // Clamp integral (this IS the rate in Tick mode)
        self.integral = self
            .integral
            .clamp(self.config.min_rate, self.config.max_rate);

        self.redemption_rate = self.integral;

        self.redemption_rate
    }

    /// Convenience: get the current deviation from peg.
    /// Returns (market - target) / target.
    pub fn deviation(&self, market_price: f64) -> f64 {
        (market_price - self.redemption_price) / self.redemption_price
    }
}
