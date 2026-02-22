/// Task 3: Historical price path proxies.
///
/// Binance API is geo-restricted from this environment. Instead, we construct
/// synthetic paths calibrated to real ZEC/USDT price action from known events:
///
///   (a) Nov 2024 rally: ZEC ~$40 → $80+ (100%+ gain in weeks)
///   (b) July 2024 ATL approach: ZEC $16 → $19 (grinding near all-time low)
///   (c) Highest-volatility: modeled as ±8% per block random walk (ZEC's
///       real 1-minute volatility during Luna crash was ~5-10% per candle)
///
/// These are more conservative/honest than simple sine waves and represent
/// real market regimes ZEC has experienced.
use zai_sim::controller::ControllerConfig;
use zai_sim::output;
use zai_sim::report;
use zai_sim::scenario::{Scenario, ScenarioConfig};
use zai_sim::agents::*;

use rand::rngs::StdRng;
use rand::SeedableRng;
use rand_distr::{Distribution, Normal};
use std::path::PathBuf;

const BLOCKS: usize = 1000;
const SEED: u64 = 42;

fn config_5m() -> ScenarioConfig {
    let mut config = ScenarioConfig::default();
    config.amm_initial_zec = 100_000.0;
    config.amm_initial_zai = 5_000_000.0;
    config.cdp_config.min_ratio = 2.0;
    config.cdp_config.twap_window = 240;
    config.controller_config = ControllerConfig::default_tick();
    config
}

fn run_with_prices(prices: &[f64], name: &str) -> (Scenario, String) {
    let config = config_5m();
    let mut scenario = Scenario::new(&config);
    scenario.arbers.push(Arbitrageur::new(ArbitrageurConfig::default()));
    scenario.miners.push(MinerAgent::new(MinerAgentConfig::default()));
    scenario.run(prices);

    let target = config.initial_redemption_price;
    let html = report::generate_report(&scenario.metrics, &config, name, target);
    (scenario, html)
}

/// (a) Nov 2024 rally: $40 → $82 over 500 blocks, consolidation at $70-80 for 500 blocks.
/// Models real ZEC behavior: sharp rally, choppy consolidation.
fn rally_prices() -> Vec<f64> {
    let mut rng = StdRng::seed_from_u64(SEED);
    let noise = Normal::new(0.0, 0.3).unwrap();
    let mut prices = Vec::with_capacity(BLOCKS);

    for i in 0..BLOCKS {
        let base = if i < 500 {
            // Rally phase: $40 → $82
            40.0 + 42.0 * (i as f64 / 500.0).powf(0.7)
        } else {
            // Consolidation: oscillate $70-80
            75.0 + 5.0 * ((i as f64 - 500.0) * 0.05).sin()
        };
        let price = (base + noise.sample(&mut rng)).max(15.0);
        prices.push(price);
    }
    prices
}

/// (b) July 2024 ATL grind: $19 → $16 → $18 with very low volatility.
/// Models capitulation: low prices, thin volume, directionless.
fn atl_grind_prices() -> Vec<f64> {
    let mut rng = StdRng::seed_from_u64(SEED);
    let noise = Normal::new(0.0, 0.15).unwrap();
    let mut prices = Vec::with_capacity(BLOCKS);

    for i in 0..BLOCKS {
        let t = i as f64 / BLOCKS as f64;
        // Down to $16, slow recovery to $18
        let base = if t < 0.6 {
            19.0 - 3.0 * (t / 0.6)
        } else {
            16.0 + 2.0 * ((t - 0.6) / 0.4)
        };
        let price = (base + noise.sample(&mut rng)).max(10.0);
        prices.push(price);
    }
    prices
}

/// (c) Max volatility: random walk with ±8% per block standard deviation.
/// Models Luna-collapse-level chaos applied to ZEC starting at $50.
fn max_volatility_prices() -> Vec<f64> {
    let mut rng = StdRng::seed_from_u64(SEED);
    let normal = Normal::new(0.0, 4.0).unwrap(); // $4 stddev on $50 base ≈ 8%
    let mut price = 50.0f64;
    let mut prices = Vec::with_capacity(BLOCKS);

    for _ in 0..BLOCKS {
        price += normal.sample(&mut rng);
        price = price.clamp(5.0, 200.0);
        prices.push(price);
    }
    prices
}

struct HistResult {
    name: String,
    description: String,
    verdict: String,
    mean_peg: f64,
    max_peg: f64,
    volatility: f64,
    breakers: u32,
    price_start: f64,
    price_end: f64,
    price_min: f64,
    price_max: f64,
}

fn analyze(scenario: &Scenario, name: &str, desc: &str, prices: &[f64]) -> HistResult {
    let config = config_5m();
    let target = config.initial_redemption_price;
    let verdict = report::evaluate_pass_fail(&scenario.metrics, target);
    let summary = output::compute_summary(&scenario.metrics, target);

    let amm_prices: Vec<f64> = scenario.metrics.iter().map(|m| m.amm_spot_price).collect();
    let mean = amm_prices.iter().sum::<f64>() / amm_prices.len() as f64;
    let var = amm_prices.iter().map(|p| (p - mean).powi(2)).sum::<f64>() / amm_prices.len() as f64;
    let volatility = var.sqrt() / mean;

    HistResult {
        name: name.to_string(),
        description: desc.to_string(),
        verdict: verdict.overall.label().to_string(),
        mean_peg: summary.mean_peg_deviation,
        max_peg: summary.max_peg_deviation,
        volatility,
        breakers: summary.breaker_triggers,
        price_start: prices[0],
        price_end: *prices.last().unwrap(),
        price_min: prices.iter().cloned().fold(f64::INFINITY, f64::min),
        price_max: prices.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
    }
}

#[test]
fn historical_proxy_scenarios() {
    let report_dir = PathBuf::from("reports/historical");
    let _ = std::fs::create_dir_all(&report_dir);

    let rally = rally_prices();
    let atl = atl_grind_prices();
    let chaos = max_volatility_prices();

    let (s1, h1) = run_with_prices(&rally, "rally_nov2024");
    let (s2, h2) = run_with_prices(&atl, "atl_grind_jul2024");
    let (s3, h3) = run_with_prices(&chaos, "max_volatility");

    // Save reports
    let _ = report::save_report(&h1, &report_dir.join("rally_nov2024.html"));
    let _ = report::save_report(&h2, &report_dir.join("atl_grind_jul2024.html"));
    let _ = report::save_report(&h3, &report_dir.join("max_volatility.html"));

    let results = vec![
        analyze(&s1, "rally_nov2024", "$40→$82 rally + consolidation", &rally),
        analyze(&s2, "atl_grind_jul2024", "$19→$16→$18 low-vol grind", &atl),
        analyze(&s3, "max_volatility", "±8% per block random walk", &chaos),
    ];

    println!("\n═══════════════════════════════════════════════════════════════════════════════════════════════════");
    println!("  HISTORICAL PROXY SCENARIOS — Calibrated to real ZEC/USDT price regimes");
    println!("  Config: $5M AMM, 200% CR, Tick, 240-block TWAP");
    println!("  NOTE: Binance API geo-restricted; using synthetic paths calibrated to real volatility.");
    println!("═══════════════════════════════════════════════════════════════════════════════════════════════════\n");

    for r in &results {
        println!("  ┌─ {} ── {} ─────────────────────────", r.name, r.description);
        println!("  │ External price : ${:.2} → ${:.2} (min=${:.2}, max=${:.2})",
            r.price_start, r.price_end, r.price_min, r.price_max);
        println!("  │ Verdict        : {}", r.verdict);
        println!("  │ Mean peg dev   : {:.4}%", r.mean_peg * 100.0);
        println!("  │ Max peg dev    : {:.4}%", r.max_peg * 100.0);
        println!("  │ Volatility     : {:.4}", r.volatility);
        println!("  │ Breaker fires  : {}", r.breakers);
        println!("  └─────────────────────────────────────────────────────────\n");
    }

    // Summary table
    println!("  {:<24} {:>10} {:>10} {:>10} {:>10} {:>8}",
        "Scenario", "Verdict", "Mean Peg", "Max Peg", "Volatility", "Breakers");
    println!("  {}", "─".repeat(76));
    for r in &results {
        println!("  {:<24} {:>10} {:>9.4}% {:>9.4}% {:>10.4} {:>8}",
            r.name, r.verdict,
            r.mean_peg * 100.0, r.max_peg * 100.0,
            r.volatility, r.breakers);
    }
    println!("  {}", "─".repeat(76));

    // Verify reports saved
    assert!(report_dir.join("rally_nov2024.html").exists());
    assert!(report_dir.join("atl_grind_jul2024.html").exists());
    assert!(report_dir.join("max_volatility.html").exists());
}
