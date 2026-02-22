use zai_sim::controller::ControllerConfig;
use zai_sim::output;
use zai_sim::report;
use zai_sim::scenario::ScenarioConfig;
use zai_sim::scenarios::{run_stress, ScenarioId};

use std::path::PathBuf;

/// Build the custom config per user spec:
/// - 200% min collateral ratio
/// - 4h TWAP window (4 * 48 = 192 blocks)
/// - Tick controller
/// - $5M AMM liquidity (100k ZEC @ $50 + 5M ZAI)
/// - Transparent liquidation (default)
/// - Circuit breakers on (default)
fn custom_config() -> ScenarioConfig {
    let mut config = ScenarioConfig::default();
    config.cdp_config.min_ratio = 2.0;
    config.cdp_config.twap_window = 192;
    config.controller_config = ControllerConfig::default_tick();
    config.amm_initial_zec = 100_000.0;
    config.amm_initial_zai = 5_000_000.0;
    config
}

#[test]
fn integration_black_thursday_default() {
    let config = custom_config();
    let target = config.initial_redemption_price;
    let blocks = 1000;
    let seed = 42;

    let scenario = run_stress(ScenarioId::BlackThursday, &config, blocks, seed);

    // Generate HTML report
    let html = report::generate_report(&scenario.metrics, &config, "black_thursday", target);
    let html_path = PathBuf::from("reports/black_thursday_default.html");
    report::save_report(&html, &html_path).expect("save report");

    // Evaluate
    let verdict = report::evaluate_pass_fail(&scenario.metrics, target);
    let summary = output::compute_summary(&scenario.metrics, target);

    println!("\n═══════════════════════════════════════════════════════════════");
    println!("  BLACK THURSDAY — Default Params (Tick controller, 200% CR)");
    println!("═══════════════════════════════════════════════════════════════");
    println!("  Overall verdict : {}", verdict.overall.label());
    println!("  Total blocks    : {}", summary.total_blocks);
    println!("  Mean peg dev    : {:.6}", summary.mean_peg_deviation);
    println!("  Max peg dev     : {:.6}", summary.max_peg_deviation);
    println!("  Total liqs      : {}", summary.total_liquidations);
    println!("  Total bad debt  : {:.2}", summary.total_bad_debt);
    println!("  AMM price range : {:.4} — {:.4}", summary.min_amm_price, summary.max_amm_price);
    println!("  Final ext price : {:.4}", scenario.metrics.last().unwrap().external_price);
    println!("  Final spot price: {:.4}", summary.final_amm_price);
    println!("  Final redemption: {:.6}", summary.final_redemption_price);
    println!("  Final peg dev   : {:.6}", summary.final_peg_deviation);
    println!("  Halt blocks     : {}", summary.halt_blocks);
    println!("  Breaker triggers: {}", summary.breaker_triggers);
    println!("  Criteria:");
    for c in &verdict.criteria {
        println!(
            "    [{}] {} — {}",
            if c.passed { "PASS" } else { c.severity.label() },
            c.name,
            c.details
        );
    }
    println!("  Report saved to : reports/black_thursday_default.html");
    println!("═══════════════════════════════════════════════════════════════\n");

    // Basic sanity
    assert!(html_path.exists(), "Report file should exist");
    assert!(scenario.metrics.len() == blocks, "Should have {blocks} blocks");
}

#[test]
fn integration_oracle_comparison() {
    let config = custom_config();
    let target = config.initial_redemption_price;
    let blocks = 1000;
    let seed = 42;

    let scenario = run_stress(ScenarioId::OracleComparison, &config, blocks, seed);

    // Generate HTML report
    let html =
        report::generate_report(&scenario.metrics, &config, "oracle_comparison", target);
    let html_path = PathBuf::from("reports/black_thursday_oracle_comparison.html");
    report::save_report(&html, &html_path).expect("save report");

    // Evaluate
    let verdict = report::evaluate_pass_fail(&scenario.metrics, target);
    let summary = output::compute_summary(&scenario.metrics, target);

    println!("\n═══════════════════════════════════════════════════════════════");
    println!("  ORACLE COMPARISON — Same Params (Tick controller, 200% CR)");
    println!("═══════════════════════════════════════════════════════════════");
    println!("  Overall verdict : {}", verdict.overall.label());
    println!("  Total blocks    : {}", summary.total_blocks);
    println!("  Mean peg dev    : {:.6}", summary.mean_peg_deviation);
    println!("  Max peg dev     : {:.6}", summary.max_peg_deviation);
    println!("  Total liqs      : {}", summary.total_liquidations);
    println!("  Total bad debt  : {:.2}", summary.total_bad_debt);
    println!("  AMM price range : {:.4} — {:.4}", summary.min_amm_price, summary.max_amm_price);
    println!("  Final ext price : {:.4}", scenario.metrics.last().unwrap().external_price);
    println!("  Final spot price: {:.4}", summary.final_amm_price);
    println!("  Final redemption: {:.6}", summary.final_redemption_price);
    println!("  Final peg dev   : {:.6}", summary.final_peg_deviation);
    println!("  Halt blocks     : {}", summary.halt_blocks);
    println!("  Breaker triggers: {}", summary.breaker_triggers);
    println!("  Criteria:");
    for c in &verdict.criteria {
        println!(
            "    [{}] {} — {}",
            if c.passed { "PASS" } else { c.severity.label() },
            c.name,
            c.details
        );
    }
    println!("  Report saved to : reports/black_thursday_oracle_comparison.html");
    println!("═══════════════════════════════════════════════════════════════\n");

    // Basic sanity
    assert!(html_path.exists(), "Report file should exist");
    assert!(scenario.metrics.len() == blocks, "Should have {blocks} blocks");
}
