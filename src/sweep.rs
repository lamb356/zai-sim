use crate::scenario::ScenarioConfig;
use crate::scenarios::{run_stress, ScenarioId};
use rayon::prelude::*;

/// A parameter to sweep over.
#[derive(Debug, Clone)]
pub struct SweepParam {
    pub name: String,
    pub values: Vec<f64>,
}

/// Result of evaluating one parameter combination.
#[derive(Debug, Clone)]
pub struct SweepResult {
    pub params: Vec<(String, f64)>,
    pub scores: Vec<(ScenarioId, f64)>,
    pub overall_score: f64,
}

/// Engine that runs parameter sweeps across scenarios.
pub struct SweepEngine {
    pub blocks: usize,
    pub seed: u64,
    pub target_price: f64,
}

impl SweepEngine {
    pub fn new(blocks: usize, seed: u64, target_price: f64) -> Self {
        SweepEngine {
            blocks,
            seed,
            target_price,
        }
    }

    /// Score a completed scenario run. Higher = better.
    pub fn score(&self, scenario: &crate::scenario::Scenario) -> f64 {
        if scenario.metrics.is_empty() {
            return f64::NEG_INFINITY;
        }
        let n = scenario.metrics.len() as f64;

        // Peg stability: mean absolute deviation
        let mean_dev: f64 = scenario
            .metrics
            .iter()
            .map(|m| ((m.amm_spot_price - self.target_price) / self.target_price).abs())
            .sum::<f64>()
            / n;

        // Bad debt ratio
        let bad_debt = scenario
            .metrics
            .last()
            .map(|m| m.bad_debt)
            .unwrap_or(0.0);
        let max_debt = scenario
            .metrics
            .iter()
            .map(|m| m.total_debt)
            .fold(1.0_f64, f64::max);
        let bad_debt_ratio = bad_debt / max_debt;

        // Halt ratio
        let halt_blocks = scenario.metrics.iter().filter(|m| m.halted).count() as f64;
        let halt_ratio = halt_blocks / n;

        // Liquidation intensity
        let total_liqs: u32 = scenario
            .metrics
            .iter()
            .map(|m| m.liquidation_count)
            .sum();
        let liq_ratio = total_liqs as f64 / n;

        -(0.4 * mean_dev + 0.3 * bad_debt_ratio + 0.2 * halt_ratio + 0.1 * liq_ratio)
    }

    /// Apply parameter overrides to a config.
    fn apply_params(config: &mut ScenarioConfig, params: &[(String, f64)]) {
        for (name, val) in params {
            match name.as_str() {
                "min_ratio" => config.cdp_config.min_ratio = *val,
                "swap_fee" => config.amm_swap_fee = *val,
                "liquidation_penalty" => config.cdp_config.liquidation_penalty = *val,
                "stability_fee_rate" => config.cdp_config.stability_fee_rate = *val,
                "twap_breaker_threshold" => {
                    config.twap_breaker_config.max_twap_change_pct = *val
                }
                "cascade_max_liqs" => {
                    config.cascade_breaker_config.max_liquidations_in_window = *val as u32
                }
                _ => {}
            }
        }
    }

    /// Generate all parameter combinations (cartesian product).
    fn cartesian_product(params: &[SweepParam]) -> Vec<Vec<(String, f64)>> {
        if params.is_empty() {
            return vec![vec![]];
        }

        let rest = Self::cartesian_product(&params[1..]);
        let mut result = Vec::new();

        for val in &params[0].values {
            for combo in &rest {
                let mut new_combo = vec![(params[0].name.clone(), *val)];
                new_combo.extend(combo.iter().cloned());
                result.push(new_combo);
            }
        }
        result
    }

    /// Run a grid sweep: evaluate all param combos × scenarios.
    pub fn run_grid(
        &self,
        params: &[SweepParam],
        scenarios: &[ScenarioId],
    ) -> Vec<SweepResult> {
        let combos = Self::cartesian_product(params);

        combos
            .par_iter()
            .map(|combo| {
                let mut scores = Vec::new();
                let mut total = 0.0;

                for &sid in scenarios {
                    let mut config = ScenarioConfig::default();
                    Self::apply_params(&mut config, combo);
                    let scenario = run_stress(sid, &config, self.blocks, self.seed);
                    let s = self.score(&scenario);
                    scores.push((sid, s));
                    total += s;
                }

                SweepResult {
                    params: combo.clone(),
                    scores,
                    overall_score: total / scenarios.len() as f64,
                }
            })
            .collect()
    }

    /// Run Monte Carlo: multiple iterations per config for robustness.
    pub fn run_monte_carlo(
        &self,
        configs: &[Vec<(String, f64)>],
        scenarios: &[ScenarioId],
        iterations: usize,
    ) -> Vec<SweepResult> {
        configs
            .par_iter()
            .map(|combo| {
                let mut total_score = 0.0;
                let mut count = 0usize;
                let mut scenario_totals: Vec<(ScenarioId, f64, usize)> = scenarios
                    .iter()
                    .map(|&sid| (sid, 0.0, 0))
                    .collect();

                for iter in 0..iterations {
                    let seed = self.seed.wrapping_add(iter as u64);
                    for entry in &mut scenario_totals {
                        let mut config = ScenarioConfig::default();
                        Self::apply_params(&mut config, combo);
                        let scenario = run_stress(entry.0, &config, self.blocks, seed);
                        let s = self.score(&scenario);
                        entry.1 += s;
                        entry.2 += 1;
                        total_score += s;
                        count += 1;
                    }
                }

                let scores: Vec<(ScenarioId, f64)> = scenario_totals
                    .into_iter()
                    .map(|(sid, total, n)| (sid, total / n as f64))
                    .collect();

                SweepResult {
                    params: combo.clone(),
                    scores,
                    overall_score: total_score / count as f64,
                }
            })
            .collect()
    }

    /// Generate refined parameter ranges centered on the best result.
    fn refine_params(results: &[SweepResult], original: &[SweepParam]) -> Vec<SweepParam> {
        if results.is_empty() {
            return original.to_vec();
        }

        let best = &results[0];
        let mut refined = Vec::new();

        for param in original {
            let best_val = best
                .params
                .iter()
                .find(|(n, _)| n == &param.name)
                .map(|(_, v)| *v)
                .unwrap_or(param.values[param.values.len() / 2]);

            // 5 values centered on best, spanning ±30%
            let delta = best_val * 0.15;
            let values: Vec<f64> = (-2..=2)
                .map(|i| (best_val + delta * i as f64).max(0.001))
                .collect();

            refined.push(SweepParam {
                name: param.name.clone(),
                values,
            });
        }

        refined
    }

    fn sort_results(results: &mut [SweepResult]) {
        results.sort_by(|a, b| {
            b.overall_score
                .partial_cmp(&a.overall_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    /// Default coarse parameters for the 4-stage sweep.
    pub fn default_coarse_params() -> Vec<SweepParam> {
        vec![
            SweepParam {
                name: "min_ratio".into(),
                values: vec![1.2, 1.5, 2.0],
            },
            SweepParam {
                name: "swap_fee".into(),
                values: vec![0.001, 0.003, 0.01],
            },
            SweepParam {
                name: "liquidation_penalty".into(),
                values: vec![0.05, 0.13, 0.20],
            },
            SweepParam {
                name: "stability_fee_rate".into(),
                values: vec![0.01, 0.02, 0.05],
            },
        ]
    }

    /// Run the full 4-stage parameter sweep.
    pub fn run_full_sweep(&self) -> Vec<SweepResult> {
        self.run_staged_sweep(
            &Self::default_coarse_params(),
            20,   // top N for Monte Carlo
            1000, // MC iterations
            3,    // top N for final
            10000, // final iterations
        )
    }

    /// Run a staged sweep with configurable iteration counts.
    /// Used by both the full sweep and tests (with smaller counts).
    pub fn run_staged_sweep(
        &self,
        coarse_params: &[SweepParam],
        top_n_mc: usize,
        mc_iterations: usize,
        top_n_final: usize,
        final_iterations: usize,
    ) -> Vec<SweepResult> {
        let coarse_scenarios = vec![
            ScenarioId::SteadyState,
            ScenarioId::BlackThursday,
            ScenarioId::SustainedBear,
            ScenarioId::OracleComparison,
        ];

        // Stage 1: Coarse grid
        let mut coarse_results = self.run_grid(coarse_params, &coarse_scenarios);
        Self::sort_results(&mut coarse_results);

        // Stage 2: Fine grid around best, all 13 scenarios
        let fine_params = Self::refine_params(&coarse_results, coarse_params);
        let all_scenarios = ScenarioId::all();
        let mut fine_results = self.run_grid(&fine_params, &all_scenarios);
        Self::sort_results(&mut fine_results);

        // Stage 3: Monte Carlo on top N
        let top_mc: Vec<Vec<(String, f64)>> = fine_results
            .iter()
            .take(top_n_mc)
            .map(|r| r.params.clone())
            .collect();
        let mut mc_results =
            self.run_monte_carlo(&top_mc, &all_scenarios, mc_iterations);
        Self::sort_results(&mut mc_results);

        // Stage 4: Final validation on top N
        let top_final: Vec<Vec<(String, f64)>> = mc_results
            .iter()
            .take(top_n_final)
            .map(|r| r.params.clone())
            .collect();
        let mut final_results =
            self.run_monte_carlo(&top_final, &all_scenarios, final_iterations);
        Self::sort_results(&mut final_results);

        final_results
    }
}
