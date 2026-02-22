use zai_sim::controller::ControllerConfig;
use zai_sim::output;
use zai_sim::report;
use zai_sim::scenario::ScenarioConfig;
use zai_sim::scenarios::{run_stress, ScenarioId};

use std::path::PathBuf;

const BLOCKS: usize = 1000;

fn config_5m() -> ScenarioConfig {
    let mut config = ScenarioConfig::default();
    config.amm_initial_zec = 100_000.0;
    config.amm_initial_zai = 5_000_000.0;
    config.cdp_config.min_ratio = 2.0;
    config.cdp_config.twap_window = 240;
    config.controller_config = ControllerConfig::default_tick();
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
fn monte_carlo_stability() {
    let config = config_5m();
    let target = config.initial_redemption_price;
    let report_dir = PathBuf::from("reports/monte_carlo");
    let _ = std::fs::create_dir_all(&report_dir);

    let scenarios = [
        ScenarioId::BlackThursday,
        ScenarioId::DemandShock,
        ScenarioId::SustainedBear,
        ScenarioId::BankRun,
    ];

    println!(
        "\n  Monte Carlo stability test: {} seeds x {} scenarios, {} blocks each\n",
        50,
        scenarios.len(),
        BLOCKS,
    );

    let mut all_scenario_stddevs: Vec<(String, f64, f64)> = Vec::new();

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

        for seed in 1..=50u64 {
            let scenario = run_stress(sid, &config, BLOCKS, seed);
            let summary = output::compute_summary(&scenario.metrics, target);
            let verdict = report::evaluate_pass_fail(&scenario.metrics, target);

            kpi_mean_peg.push(summary.mean_peg_deviation);
            kpi_max_peg.push(summary.max_peg_deviation);
            kpi_liqs.push(summary.total_liquidations as f64);
            kpi_bad_debt.push(summary.total_bad_debt);
            kpi_breakers.push(summary.breaker_triggers as f64);

            // Volatility ratio: std / mean of amm_spot_price
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
                let html =
                    report::generate_report(&scenario.metrics, &config, sid.name(), target);
                let html_path = report_dir.join(format!("{}.html", sid.name()));
                report::save_report(&html, &html_path).expect("save monte carlo report");
            }
        }

        // Compute stats across all 50 seeds
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
            "\n  ══════════════════════════════════════════════════════════════════════"
        );
        println!("  {} — 50 seeds", sid.name());
        println!(
            "  ──────────────────────────────────────────────────────────────────────"
        );
        println!(
            "  {:<24} {:>12} {:>12} {:>12} {:>12}",
            "KPI", "Mean", "StdDev", "Min", "Max"
        );
        println!("  {}", "─".repeat(72));

        for s in &stats {
            println!(
                "  {:<24} {:>12.6} {:>12.6} {:>12.6} {:>12.6}",
                s.name, s.mean, s.std, s.min, s.max,
            );
        }

        println!("  {}", "─".repeat(72));
        println!(
            "  Verdicts: {} PASS / {} SOFT FAIL / {} HARD FAIL",
            pass_count, soft_count, hard_count,
        );

        // Track for final assertion
        let mean_of_mean_peg = mean(&kpi_mean_peg);
        let std_of_mean_peg = stddev(&kpi_mean_peg);
        all_scenario_stddevs.push((sid.name().to_string(), std_of_mean_peg, mean_of_mean_peg));
    }

    // Summary across all scenarios
    println!(
        "\n  ══════════════════════════════════════════════════════════════════════"
    );
    println!("  MONTE CARLO SUMMARY");
    println!(
        "  ──────────────────────────────────────────────────────────────────────"
    );
    println!(
        "  {:<20} {:>14} {:>14} {:>10}",
        "Scenario", "Mean(MeanPeg)", "Std(MeanPeg)", "Consistent?"
    );
    println!("  {}", "─".repeat(62));

    for (name, std_val, mean_val) in &all_scenario_stddevs {
        let consistent = if *std_val < *mean_val { "YES" } else { "NO" };
        println!(
            "  {:<20} {:>14.6} {:>14.6} {:>10}",
            name, mean_val, std_val, consistent,
        );
    }
    println!(
        "  ══════════════════════════════════════════════════════════════════════\n"
    );

    println!("  Reports saved to: reports/monte_carlo/\n");

    // Assert: stddev of mean_peg_deviation < mean of mean_peg_deviation
    // (results should be somewhat consistent across seeds)
    for (name, std_val, mean_val) in &all_scenario_stddevs {
        assert!(
            *std_val < *mean_val,
            "Scenario {}: stddev of mean_peg_deviation ({:.6}) should be less than \
             mean of mean_peg_deviation ({:.6}) — results are too seed-dependent",
            name,
            std_val,
            mean_val,
        );
    }
}
