/// Stochastic Monte Carlo test — with price noise + agent noise.
///
/// Unlike the deterministic Monte Carlo (F-016 where stddev=0),
/// this test enables stochastic mode so each seed produces different results:
///   - Price noise: ±2% per block (multiplicative Normal(0, 0.02))
///   - Arber activity: 80% chance of acting each block
///   - Demand jitter: ~33% chance of skipping each block
///   - Miner batching: accumulate 1-10 blocks then dump
///
/// 50 seeds × 4 scenarios at $5M/200%/tick/240.
/// Reports mean ± 2σ for all KPIs and PASS/SOFT FAIL fractions.
use zai_sim::controller::ControllerConfig;
use zai_sim::output;
use zai_sim::report;
use zai_sim::scenario::ScenarioConfig;
use zai_sim::scenarios::{run_stress, ScenarioId};

use std::path::PathBuf;

const BLOCKS: usize = 1000;
const NUM_SEEDS: u64 = 50;

fn config_stochastic() -> ScenarioConfig {
    let mut config = ScenarioConfig::default();
    config.amm_initial_zec = 100_000.0;
    config.amm_initial_zai = 5_000_000.0;
    config.cdp_config.min_ratio = 2.0;
    config.cdp_config.twap_window = 240;
    config.controller_config = ControllerConfig::default_tick();
    // Enable stochastic noise
    config.stochastic = true;
    config.noise_sigma = 0.02;
    config.arber_activity_rate = 0.8;
    config.demand_jitter_blocks = 10;
    config.miner_batch_window = 10;
    config
}

fn stddev(vals: &[f64]) -> f64 {
    let n = vals.len() as f64;
    let mean = vals.iter().sum::<f64>() / n;
    let var = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n;
    var.sqrt()
}

fn mean(vals: &[f64]) -> f64 {
    vals.iter().sum::<f64>() / vals.len() as f64
}

#[test]
fn stochastic_monte_carlo() {
    let config = config_stochastic();
    let target = config.initial_redemption_price;
    let report_dir = PathBuf::from("reports/stochastic_monte_carlo");
    let _ = std::fs::create_dir_all(&report_dir);

    let scenarios = [
        ScenarioId::BlackThursday,
        ScenarioId::DemandShock,
        ScenarioId::SustainedBear,
        ScenarioId::BankRun,
    ];

    println!(
        "\n═══════════════════════════════════════════════════════════════════════════════════════════════════"
    );
    println!(
        "  STOCHASTIC MONTE CARLO: {} seeds × {} scenarios, {} blocks",
        NUM_SEEDS,
        scenarios.len(),
        BLOCKS,
    );
    println!("  Config: $5M AMM, 200% CR, Tick, 240-block TWAP");
    println!("  Noise: σ=0.02, arber_rate=0.8, demand_jitter=10, miner_batch=10");
    println!(
        "═══════════════════════════════════════════════════════════════════════════════════════════════════"
    );

    let mut scenario_summaries: Vec<(String, f64, f64, u32, u32, u32, bool)> = Vec::new();

    for &sid in &scenarios {
        let mut kpi_mean_peg: Vec<f64> = Vec::new();
        let mut kpi_max_peg: Vec<f64> = Vec::new();
        let mut kpi_liqs: Vec<f64> = Vec::new();
        let mut kpi_bad_debt: Vec<f64> = Vec::new();
        let mut kpi_breakers: Vec<f64> = Vec::new();
        let mut kpi_volatility: Vec<f64> = Vec::new();

        let mut pass_count = 0u32;
        let mut soft_count = 0u32;
        let mut hard_count = 0u32;

        for seed in 1..=NUM_SEEDS {
            let scenario = run_stress(sid, &config, BLOCKS, seed);
            let summary = output::compute_summary(&scenario.metrics, target);
            let verdict = report::evaluate_pass_fail(&scenario.metrics, target);

            kpi_mean_peg.push(summary.mean_peg_deviation);
            kpi_max_peg.push(summary.max_peg_deviation);
            kpi_liqs.push(summary.total_liquidations as f64);
            kpi_bad_debt.push(summary.total_bad_debt);
            kpi_breakers.push(summary.breaker_triggers as f64);

            let prices: Vec<f64> = scenario.metrics.iter().map(|m| m.amm_spot_price).collect();
            let price_mean = mean(&prices);
            let price_std = stddev(&prices);
            let vol_ratio = if price_mean > 0.0 {
                price_std / price_mean
            } else {
                0.0
            };
            kpi_volatility.push(vol_ratio);

            match verdict.overall {
                report::Verdict::Pass => pass_count += 1,
                report::Verdict::SoftFail => soft_count += 1,
                report::Verdict::HardFail => hard_count += 1,
            }

            // Save HTML report for seed=42
            if seed == 42 {
                let html = report::generate_report(
                    &scenario.metrics,
                    &config,
                    &format!("{}_stochastic", sid.name()),
                    target,
                );
                let html_path = report_dir.join(format!("{}.html", sid.name()));
                report::save_report(&html, &html_path).expect("save report");
            }
        }

        // Compute stats across all seeds
        struct KpiStats {
            name: &'static str,
            mean: f64,
            std: f64,
            min: f64,
            max: f64,
        }

        let compute_stats = |name: &'static str, vals: &[f64]| -> KpiStats {
            KpiStats {
                name,
                mean: mean(vals),
                std: stddev(vals),
                min: vals.iter().cloned().fold(f64::INFINITY, f64::min),
                max: vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
            }
        };

        let stats = vec![
            compute_stats("mean_peg_deviation", &kpi_mean_peg),
            compute_stats("max_peg_deviation", &kpi_max_peg),
            compute_stats("total_liquidations", &kpi_liqs),
            compute_stats("total_bad_debt", &kpi_bad_debt),
            compute_stats("breaker_triggers", &kpi_breakers),
            compute_stats("volatility_ratio", &kpi_volatility),
        ];

        println!(
            "\n  ┌─ {} ── {} seeds ─────────────────────────────────",
            sid.name(),
            NUM_SEEDS
        );
        println!(
            "  │ {:<24} {:>12} {:>12} {:>12} {:>12}",
            "KPI", "Mean", "StdDev", "Min", "Max"
        );
        println!("  │ {}", "─".repeat(72));

        for s in &stats {
            println!(
                "  │ {:<24} {:>12.6} {:>12.6} {:>12.6} {:>12.6}",
                s.name, s.mean, s.std, s.min, s.max,
            );
        }

        println!("  │ {}", "─".repeat(72));
        println!(
            "  │ Verdicts: {} PASS / {} SOFT FAIL / {} HARD FAIL",
            pass_count, soft_count, hard_count,
        );

        // Is this a boundary scenario? (flips between PASS and SOFT FAIL)
        let is_boundary = pass_count > 0 && soft_count > 0;
        if is_boundary {
            println!(
                "  │ ⚠ BOUNDARY SCENARIO: {:.0}% PASS, {:.0}% SOFT FAIL across seeds",
                pass_count as f64 / NUM_SEEDS as f64 * 100.0,
                soft_count as f64 / NUM_SEEDS as f64 * 100.0,
            );
        }

        // Mean ± 2σ intervals
        let m = mean(&kpi_mean_peg);
        let s = stddev(&kpi_mean_peg);
        println!("  │ Mean peg dev: {:.6} ± {:.6} (2σ)", m, 2.0 * s);
        println!("  └────────────────────────────────────────────────────────────\n");

        scenario_summaries.push((
            sid.name().to_string(),
            m,
            s,
            pass_count,
            soft_count,
            hard_count,
            is_boundary,
        ));
    }

    // Summary table
    println!(
        "\n  ══════════════════════════════════════════════════════════════════════"
    );
    println!("  STOCHASTIC MONTE CARLO SUMMARY");
    println!(
        "  ──────────────────────────────────────────────────────────────────────"
    );
    println!(
        "  {:<20} {:>12} {:>12} {:>6} {:>6} {:>6} {:>10}",
        "Scenario", "Mean±2σ", "StdDev", "PASS", "SOFT", "HARD", "Boundary?"
    );
    println!("  {}", "─".repeat(78));

    for (name, m, s, pass, soft, hard, boundary) in &scenario_summaries {
        println!(
            "  {:<20} {:>5.4}±{:<5.4} {:>12.6} {:>6} {:>6} {:>6} {:>10}",
            name,
            m,
            2.0 * s,
            s,
            pass,
            soft,
            hard,
            if *boundary { "YES" } else { "no" },
        );
    }
    println!(
        "  ══════════════════════════════════════════════════════════════════════\n"
    );

    // Assertions: stochastic mode should produce non-zero standard deviation
    // (unlike F-016 where deterministic paths gave stddev=0)
    for (name, _m, s, _pass, _soft, _hard, _boundary) in &scenario_summaries {
        assert!(
            *s > 0.0,
            "Scenario {}: stochastic mode should produce non-zero stddev, got {:.10}",
            name,
            s,
        );
    }

    // stddev should be less than mean (results still somewhat consistent)
    for (name, m, s, _pass, _soft, _hard, _boundary) in &scenario_summaries {
        assert!(
            *s < *m,
            "Scenario {}: stddev ({:.6}) should be less than mean ({:.6}) — results are too noisy",
            name,
            s,
            m,
        );
    }
}
