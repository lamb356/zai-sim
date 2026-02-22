use zai_sim::agents::{Arbitrageur, ArbitrageurConfig, MinerAgent, MinerAgentConfig};
use zai_sim::controller::ControllerConfig;
use zai_sim::output;
use zai_sim::report;
use zai_sim::scenario::{Scenario, ScenarioConfig};
use zai_sim::scenarios::generate_prices;
use zai_sim::scenarios::ScenarioId;

use std::path::PathBuf;

const BLOCKS: usize = 1000;
const SEED: u64 = 42;

fn base_config() -> ScenarioConfig {
    let mut config = ScenarioConfig::default();
    config.amm_initial_zec = 100_000.0;
    config.amm_initial_zai = 5_000_000.0;
    config.cdp_config.min_ratio = 2.0;
    config.cdp_config.twap_window = 240;
    config.controller_config = ControllerConfig::default_tick();
    config
}

fn run_with_arber(arber_config: ArbitrageurConfig) -> Scenario {
    let config = base_config();
    let prices = generate_prices(ScenarioId::BlackThursday, BLOCKS, SEED);
    let mut scenario = Scenario::new(&config);
    scenario.arbers.push(Arbitrageur::new(arber_config));
    scenario.miners.push(MinerAgent::new(MinerAgentConfig::default()));
    scenario.run(&prices);
    scenario
}

#[test]
fn arber_degradation() {
    let config = base_config();
    let target = config.initial_redemption_price; // 50.0

    // Define the 5 arber configurations
    let configs: Vec<(&str, ArbitrageurConfig)> = vec![
        ("baseline", ArbitrageurConfig::default()),
        (
            "50pct_capital",
            ArbitrageurConfig {
                initial_zai_balance: 50_000.0,
                initial_zec_balance: 1000.0,
                ..ArbitrageurConfig::default()
            },
        ),
        (
            "2x_latency",
            ArbitrageurConfig {
                arb_latency_sell_blocks: 20,
                ..ArbitrageurConfig::default()
            },
        ),
        (
            "50pct_detection",
            ArbitrageurConfig {
                arb_threshold_pct: 1.0,
                ..ArbitrageurConfig::default()
            },
        ),
        (
            "all_degraded",
            ArbitrageurConfig {
                initial_zai_balance: 50_000.0,
                initial_zec_balance: 1000.0,
                arb_latency_sell_blocks: 20,
                arb_threshold_pct: 1.0,
                ..ArbitrageurConfig::default()
            },
        ),
    ];

    struct ResultRow {
        label: String,
        verdict: String,
        mean_peg: f64,
        max_peg: f64,
        breaker_triggers: u32,
        volatility: f64,
    }

    let mut rows: Vec<ResultRow> = Vec::new();

    let report_dir = PathBuf::from("reports/arber_degradation");
    let _ = std::fs::create_dir_all(&report_dir);

    println!(
        "\n  Running arber degradation analysis ({} blocks, seed={})...\n",
        BLOCKS, SEED
    );

    for (label, arber_config) in &configs {
        let scenario = run_with_arber(arber_config.clone());

        // Evaluate verdict
        let verdict = report::evaluate_pass_fail(&scenario.metrics, target);
        let summary = output::compute_summary(&scenario.metrics, target);

        // Compute volatility (std/mean of amm_spot_price)
        let prices: Vec<f64> = scenario.metrics.iter().map(|m| m.amm_spot_price).collect();
        let mean = prices.iter().sum::<f64>() / prices.len() as f64;
        let variance =
            prices.iter().map(|p| (p - mean).powi(2)).sum::<f64>() / prices.len() as f64;
        let volatility = if mean > 0.0 {
            variance.sqrt() / mean
        } else {
            0.0
        };

        rows.push(ResultRow {
            label: label.to_string(),
            verdict: verdict.overall.label().to_string(),
            mean_peg: summary.mean_peg_deviation,
            max_peg: summary.max_peg_deviation,
            breaker_triggers: summary.breaker_triggers,
            volatility,
        });

        // Generate and save HTML report
        let html = report::generate_report(&scenario.metrics, &config, label, target);
        let html_path = report_dir.join(format!("{}.html", label));
        report::save_report(&html, &html_path).expect("save arber degradation report");
    }

    // Print comparison table
    println!("\n===========================================================================================");
    println!("  ARBER DEGRADATION ANALYSIS (Black Thursday, {} blocks)", BLOCKS);
    println!("===========================================================================================");
    println!(
        "  {:<20} {:>10} {:>12} {:>12} {:>10} {:>12}",
        "Config", "Verdict", "Mean Peg", "Max Peg", "Breakers", "Volatility"
    );
    println!("  {}", "-".repeat(80));

    for r in &rows {
        println!(
            "  {:<20} {:>10} {:>11.4}% {:>11.4}% {:>10} {:>12.6}",
            r.label,
            r.verdict,
            r.mean_peg * 100.0,
            r.max_peg * 100.0,
            r.breaker_triggers,
            r.volatility,
        );
    }

    // Print degradation factors (ratio of each config's mean_peg to baseline)
    let baseline_mean_peg = rows[0].mean_peg;
    println!("  {}", "-".repeat(80));
    print!("  {:<20}", "Degradation Factor");
    for r in &rows {
        let factor = if baseline_mean_peg > 0.0 {
            r.mean_peg / baseline_mean_peg
        } else {
            0.0
        };
        print!(" {:>12.2}x", factor);
    }
    println!();
    println!("===========================================================================================");
    println!("\n  Reports saved to: reports/arber_degradation/");
    println!(
        "  Configs: {}\n",
        rows.iter()
            .map(|r| r.label.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    // Verify all report files were created
    for (label, _) in &configs {
        let path = report_dir.join(format!("{}.html", label));
        assert!(
            path.exists(),
            "Report should exist for arber config: {}",
            label
        );
    }
}
