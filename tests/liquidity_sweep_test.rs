use zai_sim::controller::ControllerConfig;
use zai_sim::output;
use zai_sim::report;
use zai_sim::scenario::ScenarioConfig;
use zai_sim::scenarios::{run_stress, ScenarioId};

use std::path::PathBuf;

const BLOCKS: usize = 1000;
const SEED: u64 = 42;

fn config_with_liquidity(amm_zec: f64, amm_zai: f64) -> ScenarioConfig {
    let mut config = ScenarioConfig::default();
    config.amm_initial_zec = amm_zec;
    config.amm_initial_zai = amm_zai;
    config.cdp_config.min_ratio = 2.0;
    config.cdp_config.twap_window = 240;
    config.controller_config = ControllerConfig::default_tick();
    config
}

struct RunResult {
    label: String,
    _scenario: String,
    verdict: String,
    mean_peg: f64,
    max_peg: f64,
    liqs: u32,
    bad_debt: f64,
    volatility: f64,
    breaker_triggers: u32,
}

fn run_one(sid: ScenarioId, amm_zec: f64, amm_zai: f64, label: &str) -> RunResult {
    let config = config_with_liquidity(amm_zec, amm_zai);
    let target = config.initial_redemption_price;

    let scenario = run_stress(sid, &config, BLOCKS, SEED);

    // Save HTML report
    let report_dir = PathBuf::from("reports/liquidity_sweep");
    let _ = std::fs::create_dir_all(&report_dir);
    let html = report::generate_report(&scenario.metrics, &config, &format!("{}_{}", sid.name(), label), target);
    let html_path = report_dir.join(format!("{}_{}.html", sid.name(), label));
    let _ = report::save_report(&html, &html_path);

    let verdict = report::evaluate_pass_fail(&scenario.metrics, target);
    let summary = output::compute_summary(&scenario.metrics, target);

    let prices: Vec<f64> = scenario.metrics.iter().map(|m| m.amm_spot_price).collect();
    let mean = prices.iter().sum::<f64>() / prices.len() as f64;
    let variance = prices.iter().map(|p| (p - mean).powi(2)).sum::<f64>() / prices.len() as f64;
    let volatility = variance.sqrt() / mean;

    RunResult {
        label: label.to_string(),
        _scenario: sid.name().to_string(),
        verdict: verdict.overall.label().to_string(),
        mean_peg: summary.mean_peg_deviation,
        max_peg: summary.max_peg_deviation,
        liqs: summary.total_liquidations,
        bad_debt: summary.total_bad_debt,
        volatility,
        breaker_triggers: summary.breaker_triggers,
    }
}

#[test]
fn liquidity_sweep_demand_shock() {
    println!("\n═══════════════════════════════════════════════════════════════════════════════════════");
    println!("  DEMAND SHOCK — Liquidity Sweep ($5M reference, then $10M / $25M / $50M)");
    println!("  Config: 200% CR, Tick controller, 240-block TWAP");
    println!("═══════════════════════════════════════════════════════════════════════════════════════");

    // $5M reference (already known: SOFT FAIL)
    let r5 = run_one(ScenarioId::DemandShock, 100_000.0, 5_000_000.0, "5m");
    // $10M
    let r10 = run_one(ScenarioId::DemandShock, 200_000.0, 10_000_000.0, "10m");
    // $25M
    let r25 = run_one(ScenarioId::DemandShock, 500_000.0, 25_000_000.0, "25m");
    // $50M
    let r50 = run_one(ScenarioId::DemandShock, 1_000_000.0, 50_000_000.0, "50m");

    let results = [&r5, &r10, &r25, &r50];

    println!(
        "\n  {:<12} {:>10} {:>10} {:>10} {:>10} {:>6} {:>10} {:>8}",
        "Liquidity", "Verdict", "Mean Peg", "Max Peg", "Volatility", "Liqs", "Bad Debt", "Breakers"
    );
    println!("  {}", "─".repeat(88));
    for r in &results {
        println!(
            "  {:<12} {:>10} {:>9.4}% {:>9.4}% {:>10.4} {:>6} {:>10.2} {:>8}",
            r.label, r.verdict,
            r.mean_peg * 100.0, r.max_peg * 100.0,
            r.volatility, r.liqs, r.bad_debt, r.breaker_triggers,
        );
    }
    println!("  {}", "─".repeat(88));

    // Identify threshold
    let first_pass = results.iter().find(|r| r.verdict == "PASS");
    match first_pass {
        Some(r) => println!("\n  Minimum liquidity for PASS: ${}\n", r.label),
        None => println!("\n  No configuration achieved PASS\n"),
    }
}

#[test]
fn liquidity_sweep_black_thursday() {
    println!("\n═══════════════════════════════════════════════════════════════════════════════════════");
    println!("  BLACK THURSDAY — Minimum Viable Liquidity ($2M / $3M / $5M reference)");
    println!("  Config: 200% CR, Tick controller, 240-block TWAP");
    println!("═══════════════════════════════════════════════════════════════════════════════════════");

    // $2M
    let r2 = run_one(ScenarioId::BlackThursday, 40_000.0, 2_000_000.0, "2m");
    // $3M
    let r3 = run_one(ScenarioId::BlackThursday, 60_000.0, 3_000_000.0, "3m");
    // $5M reference (already known: PASS)
    let r5 = run_one(ScenarioId::BlackThursday, 100_000.0, 5_000_000.0, "5m");

    let results = [&r2, &r3, &r5];

    println!(
        "\n  {:<12} {:>10} {:>10} {:>10} {:>10} {:>6} {:>10} {:>8}",
        "Liquidity", "Verdict", "Mean Peg", "Max Peg", "Volatility", "Liqs", "Bad Debt", "Breakers"
    );
    println!("  {}", "─".repeat(88));
    for r in &results {
        println!(
            "  {:<12} {:>10} {:>9.4}% {:>9.4}% {:>10.4} {:>6} {:>10.2} {:>8}",
            r.label, r.verdict,
            r.mean_peg * 100.0, r.max_peg * 100.0,
            r.volatility, r.liqs, r.bad_debt, r.breaker_triggers,
        );
    }
    println!("  {}", "─".repeat(88));

    let first_pass = results.iter().find(|r| r.verdict == "PASS");
    match first_pass {
        Some(r) => println!("\n  Minimum liquidity for PASS: ${}\n", r.label),
        None => println!("\n  No configuration achieved PASS\n"),
    }
}
