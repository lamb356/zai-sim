/// Block Time Sensitivity Test (F-040)
///
/// Zcash targets 75-second blocks but actual block times vary significantly.
/// The simulator's TWAP oracle is purely block-count based: each block gets
/// equal weight regardless of real-world duration. This test determines whether
/// irregular block timing produces dangerous TWAP errors or bad debt.
///
/// Approach: Define a ground-truth price curve, compute block arrival times for
/// each timing pattern, sample the curve at those times, and feed sampled prices
/// to scenario.run(). Then compare the system's block-count TWAP against the
/// "true" time-weighted average.
///
/// Sweep: 4 timing patterns × 2 scenarios = 8 runs.
use zai_sim::agents::*;
use zai_sim::controller::ControllerConfig;
use zai_sim::output;
use zai_sim::report;
use zai_sim::scenario::{Scenario, ScenarioConfig};

use std::path::PathBuf;

const BLOCKS: usize = 1000;
const SEED: u64 = 42;
const TWAP_WINDOW: u64 = 240;

// ═══════════════════════════════════════════════════════════════════════
// Ground-Truth Price Curves
// ═══════════════════════════════════════════════════════════════════════

/// Price as a function of fractional time [0, 1].
/// Mirrors the shapes in scenarios.rs for comparability.
fn price_at_frac(frac: f64, scenario: &str) -> f64 {
    match scenario {
        "black_thursday" => {
            // $50 flat → crash to $20 → recover to $35
            if frac < 0.25 {
                50.0
            } else if frac < 0.35 {
                50.0 - 30.0 * (frac - 0.25) / 0.10
            } else if frac < 0.60 {
                20.0 + 15.0 * (frac - 0.35) / 0.25
            } else {
                35.0
            }
        }
        "sustained_bear" => 50.0 - 35.0 * frac,
        _ => 50.0,
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Block Arrival Time Generators
// ═══════════════════════════════════════════════════════════════════════

/// Regular 75s blocks (baseline).
fn block_times_regular(blocks: usize) -> Vec<f64> {
    (1..=blocks).map(|i| i as f64 * 75.0).collect()
}

/// Bursty: 10 blocks at 5s apart, then 600s gap, repeat.
fn block_times_bursty(blocks: usize) -> Vec<f64> {
    let mut times = Vec::with_capacity(blocks);
    let mut t = 0.0;
    for i in 0..blocks {
        let pos = i % 10;
        if pos == 0 && i > 0 {
            t += 600.0; // 10-min gap before each burst
        }
        t += 5.0; // 5s between burst blocks
        times.push(t);
    }
    times
}

/// Slow: 150s per block (2x normal).
fn block_times_slow(blocks: usize) -> Vec<f64> {
    (1..=blocks).map(|i| i as f64 * 150.0).collect()
}

/// Mixed: alternating 50-block burst (5s) and 50-block slow (150s) phases.
fn block_times_mixed(blocks: usize) -> Vec<f64> {
    let mut times = Vec::with_capacity(blocks);
    let mut t = 0.0;
    for i in 0..blocks {
        let phase = (i / 50) % 2; // 0=burst, 1=slow
        t += if phase == 0 { 5.0 } else { 150.0 };
        times.push(t);
    }
    times
}

// ═══════════════════════════════════════════════════════════════════════
// Price Sampling
// ═══════════════════════════════════════════════════════════════════════

/// Sample the ground-truth price curve at block arrival times.
fn sample_prices(block_times: &[f64], scenario: &str) -> Vec<f64> {
    let max_time = *block_times.last().unwrap();
    block_times
        .iter()
        .map(|&t| price_at_frac(t / max_time, scenario))
        .collect()
}

// ═══════════════════════════════════════════════════════════════════════
// True Time-Weighted Average
// ═══════════════════════════════════════════════════════════════════════

/// Compute the true time-weighted average price over the last `window_blocks` blocks.
/// This weights each price by the real-time duration it was in effect, not by block count.
fn compute_true_twap(prices: &[f64], block_times: &[f64], window_blocks: usize) -> f64 {
    let n = prices.len();
    if n == 0 {
        return 0.0;
    }
    let start = n.saturating_sub(window_blocks);
    let mut weighted_sum = 0.0;
    let mut total_time = 0.0;
    for i in (start + 1)..n {
        let dt = block_times[i] - block_times[i - 1];
        weighted_sum += prices[i - 1] * dt;
        total_time += dt;
    }
    if total_time > 0.0 {
        weighted_sum / total_time
    } else {
        prices[n - 1]
    }
}

/// Compute mean and max TWAP error across all blocks (after warmup).
fn compute_twap_errors(
    metrics: &[zai_sim::scenario::BlockMetrics],
    prices: &[f64],
    block_times: &[f64],
    window_blocks: usize,
) -> (f64, f64) {
    let warmup = window_blocks; // skip first window_blocks for TWAP to fill
    let mut sum_error = 0.0;
    let mut max_error: f64 = 0.0;
    let mut count = 0;

    for (i, m) in metrics.iter().enumerate() {
        if i < warmup {
            continue;
        }
        let true_twap = compute_true_twap(
            &prices[..=i],
            &block_times[..=i],
            window_blocks,
        );
        if true_twap > 0.0 {
            let error = (m.twap_price - true_twap).abs() / true_twap;
            sum_error += error;
            max_error = max_error.max(error);
            count += 1;
        }
    }

    let mean_error = if count > 0 {
        sum_error / count as f64
    } else {
        0.0
    };
    (mean_error, max_error)
}

// ═══════════════════════════════════════════════════════════════════════
// Scenario Config & Agents
// ═══════════════════════════════════════════════════════════════════════

fn default_config() -> ScenarioConfig {
    let mut c = ScenarioConfig::default();
    c.amm_initial_zec = 100_000.0;
    c.amm_initial_zai = 5_000_000.0;
    c.cdp_config.min_ratio = 2.0;
    c.cdp_config.twap_window = TWAP_WINDOW;
    c.controller_config = ControllerConfig::default_tick();
    c.liquidation_config.max_liquidations_per_block = 50;
    c
}

fn add_agents(scenario: &mut Scenario) {
    scenario
        .arbers
        .push(Arbitrageur::new(ArbitrageurConfig::default()));
    scenario
        .miners
        .push(MinerAgent::new(MinerAgentConfig::default()));
    scenario
        .demand_agents
        .push(DemandAgent::new(DemandAgentConfig::default()));
    scenario
        .cdp_holders
        .push(CdpHolder::new(CdpHolderConfig::default()));
}

// ═══════════════════════════════════════════════════════════════════════
// Result Struct
// ═══════════════════════════════════════════════════════════════════════

struct BlockTimeRow {
    timing_pattern: String,
    scenario_name: String,
    verdict: String,
    mean_peg: f64,
    max_peg: f64,
    liqs: u32,
    bad_debt: f64,
    mean_twap_error: f64,
    max_twap_error: f64,
    final_twap: f64,
    final_true_twap: f64,
}

// ═══════════════════════════════════════════════════════════════════════
// Run Single
// ═══════════════════════════════════════════════════════════════════════

fn run_single(
    timing_name: &str,
    block_times: &[f64],
    scenario_name: &str,
) -> (BlockTimeRow, Scenario) {
    let config = default_config();
    let prices = sample_prices(block_times, scenario_name);
    let target = 50.0;

    let mut scenario = Scenario::new_with_seed(&config, SEED);
    add_agents(&mut scenario);
    scenario.run(&prices);

    let summary = output::compute_summary(&scenario.metrics, target);
    let verdict = report::evaluate_pass_fail(&scenario.metrics, target);

    let (mean_twap_error, max_twap_error) =
        compute_twap_errors(&scenario.metrics, &prices, block_times, TWAP_WINDOW as usize);

    let final_twap = scenario.metrics.last().unwrap().twap_price;
    let final_true_twap = compute_true_twap(&prices, block_times, TWAP_WINDOW as usize);

    let row = BlockTimeRow {
        timing_pattern: timing_name.to_string(),
        scenario_name: scenario_name.to_string(),
        verdict: verdict.overall.label().to_string(),
        mean_peg: summary.mean_peg_deviation,
        max_peg: summary.max_peg_deviation,
        liqs: summary.total_liquidations,
        bad_debt: summary.total_bad_debt,
        mean_twap_error,
        max_twap_error,
        final_twap,
        final_true_twap,
    };

    (row, scenario)
}

// ═══════════════════════════════════════════════════════════════════════
// Test
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn block_time_sensitivity_sweep() {
    let timing_patterns: Vec<(&str, Vec<f64>)> = vec![
        ("regular", block_times_regular(BLOCKS)),
        ("bursty", block_times_bursty(BLOCKS)),
        ("slow", block_times_slow(BLOCKS)),
        ("mixed", block_times_mixed(BLOCKS)),
    ];
    let scenarios: Vec<&str> = vec!["black_thursday", "sustained_bear"];

    let report_dir = PathBuf::from("reports/block_time_sensitivity");
    let _ = std::fs::create_dir_all(&report_dir);

    let mut rows: Vec<BlockTimeRow> = Vec::new();
    let mut entries = Vec::new();
    let target = 50.0;

    println!("\n  Running block time sensitivity sweep...");
    println!("  Config: $5M AMM, 200% CR, Tick controller, 240-block TWAP");
    println!("  Timing: regular (75s), bursty (5s×10 + 600s gap), slow (150s), mixed");
    println!("  Sweep: 4 patterns × 2 scenarios = 8 runs\n");

    for &(timing_name, ref block_times) in &timing_patterns {
        for &scenario_name in &scenarios {
            let run_name = format!("{}_{}", timing_name, scenario_name);
            println!("  Running {}...", run_name);

            let (row, scenario) = run_single(timing_name, block_times, scenario_name);

            let config = default_config();
            let verdict = report::evaluate_pass_fail(&scenario.metrics, target);
            let summary = output::compute_summary(&scenario.metrics, target);

            let html = report::generate_report(&scenario.metrics, &config, &run_name, target);
            let html_path = report_dir.join(format!("{}.html", run_name));
            report::save_report(&html, &html_path).expect("save report");

            entries.push((run_name, verdict, summary));
            rows.push(row);
        }
    }

    // Generate master index
    let master_html = report::generate_master_summary(&entries);
    let master_path = report_dir.join("index.html");
    report::save_report(&master_html, &master_path).expect("save master summary");

    // Print results table
    println!("\n═══════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════");
    println!("  ZAI SIMULATOR — BLOCK TIME SENSITIVITY SWEEP (F-040)");
    println!("═══════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════");
    println!(
        "  {:<10} {:<16} {:>10} {:>10} {:>10} {:>6} {:>10} {:>12} {:>12} {:>10} {:>10}",
        "Timing", "Scenario", "Verdict", "Mean Peg", "Max Peg", "Liqs", "Bad Debt",
        "Mean TWAP E", "Max TWAP E", "Sys TWAP", "True TWAP"
    );
    println!("  {}", "─".repeat(118));

    for r in &rows {
        println!(
            "  {:<10} {:<16} {:>10} {:>9.4}% {:>9.4}% {:>6} {:>10.2} {:>11.4}% {:>11.4}% {:>10.2} {:>10.2}",
            r.timing_pattern,
            r.scenario_name,
            r.verdict,
            r.mean_peg * 100.0,
            r.max_peg * 100.0,
            r.liqs,
            r.bad_debt,
            r.mean_twap_error * 100.0,
            r.max_twap_error * 100.0,
            r.final_twap,
            r.final_true_twap,
        );
    }

    println!("  {}", "─".repeat(118));

    // Per-scenario: timing vs TWAP error
    println!("\n  Per-scenario: timing pattern vs mean TWAP error");
    println!(
        "  {:<16} {:>12} {:>12} {:>12} {:>12}",
        "Scenario", "Regular", "Bursty", "Slow", "Mixed"
    );
    println!("  {}", "─".repeat(64));

    for &scenario_name in &scenarios {
        let errors: Vec<String> = ["regular", "bursty", "slow", "mixed"]
            .iter()
            .map(|&timing| {
                rows.iter()
                    .find(|r| r.timing_pattern == timing && r.scenario_name == scenario_name)
                    .map(|r| format!("{:.4}%", r.mean_twap_error * 100.0))
                    .unwrap_or_default()
            })
            .collect();
        println!(
            "  {:<16} {:>12} {:>12} {:>12} {:>12}",
            scenario_name, errors[0], errors[1], errors[2], errors[3]
        );
    }

    // Per-scenario: timing vs verdict
    println!("\n  Per-scenario: timing pattern vs verdict");
    println!(
        "  {:<16} {:>12} {:>12} {:>12} {:>12}",
        "Scenario", "Regular", "Bursty", "Slow", "Mixed"
    );
    println!("  {}", "─".repeat(64));

    for &scenario_name in &scenarios {
        let verdicts: Vec<String> = ["regular", "bursty", "slow", "mixed"]
            .iter()
            .map(|&timing| {
                rows.iter()
                    .find(|r| r.timing_pattern == timing && r.scenario_name == scenario_name)
                    .map(|r| r.verdict.clone())
                    .unwrap_or_default()
            })
            .collect();
        println!(
            "  {:<16} {:>12} {:>12} {:>12} {:>12}",
            scenario_name, verdicts[0], verdicts[1], verdicts[2], verdicts[3]
        );
    }

    // Per-scenario: timing vs bad debt
    println!("\n  Per-scenario: timing pattern vs bad debt");
    println!(
        "  {:<16} {:>12} {:>12} {:>12} {:>12}",
        "Scenario", "Regular", "Bursty", "Slow", "Mixed"
    );
    println!("  {}", "─".repeat(64));

    for &scenario_name in &scenarios {
        let debts: Vec<String> = ["regular", "bursty", "slow", "mixed"]
            .iter()
            .map(|&timing| {
                rows.iter()
                    .find(|r| r.timing_pattern == timing && r.scenario_name == scenario_name)
                    .map(|r| {
                        if r.bad_debt == 0.0 {
                            "$0".to_string()
                        } else {
                            format!("${:.0}", r.bad_debt)
                        }
                    })
                    .unwrap_or_default()
            })
            .collect();
        println!(
            "  {:<16} {:>12} {:>12} {:>12} {:>12}",
            scenario_name, debts[0], debts[1], debts[2], debts[3]
        );
    }

    println!("\n═══════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════");
    println!("  Reports saved to: reports/block_time_sensitivity/");
    println!("  Master summary:   reports/block_time_sensitivity/index.html\n");

    // Verify all reports exist
    assert!(master_path.exists(), "Master summary should exist");
    for &(timing_name, _) in &timing_patterns {
        for &scenario_name in &scenarios {
            let run_name = format!("{}_{}", timing_name, scenario_name);
            let path = report_dir.join(format!("{}.html", run_name));
            assert!(path.exists(), "Report should exist for {}", run_name);
        }
    }
}
