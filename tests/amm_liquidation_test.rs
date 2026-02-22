/// Task 6: AMM Liquidation Feedback — Death Spiral Modeling
///
/// Tests the `use_amm_liquidation` mode where seized collateral is sold through
/// the AMM swap function using spot price for liquidation checks. This creates
/// the death spiral feedback loop:
///
///   liquidation → ZEC dump on AMM → spot price drops → more liquidations
///
/// This is the dynamic that killed MakerDAO vaults on Black Thursday 2020.
/// Compares bypass mode (TWAP-based) vs AMM mode (spot-based cascading).
use zai_sim::agents::*;
use zai_sim::controller::ControllerConfig;
use zai_sim::output;
use zai_sim::report;
use zai_sim::scenario::{Scenario, ScenarioConfig};
use zai_sim::scenarios::{generate_prices, ScenarioId};

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

/// Build scenario with CDP holders and configurable liquidation mode.
fn run_with_mode(
    sid: ScenarioId,
    use_amm_liquidation: bool,
    num_holders: usize,
) -> Scenario {
    let mut config = config_5m();
    config.use_amm_liquidation = use_amm_liquidation;

    let prices = generate_prices(sid, BLOCKS, SEED);
    let mut scenario = Scenario::new(&config);

    // Standard agents
    scenario
        .arbers
        .push(Arbitrageur::new(ArbitrageurConfig::default()));
    scenario
        .miners
        .push(MinerAgent::new(MinerAgentConfig::default()));

    // CDP holders with varying collateral ratios
    for i in 0..num_holders {
        let target = 2.0 + (i as f64) * 0.3; // 2.0, 2.3, 2.6, 2.9, 3.2
        scenario.cdp_holders.push(CdpHolder::new(CdpHolderConfig {
            target_ratio: target,
            action_threshold_ratio: target - 0.3,
            reserve_zec: 200.0,
            initial_collateral: 100.0,
            initial_debt: 2000.0,
        }));
    }

    scenario.run(&prices);
    scenario
}

struct ModeResult {
    mode_name: String,
    verdict: String,
    mean_peg: f64,
    max_peg: f64,
    total_liquidations: u32,
    total_bad_debt: f64,
    breaker_triggers: u32,
    final_vault_count: u64,
    cascade_blocks: u32, // blocks with >1 liquidation
}

fn analyze(scenario: &Scenario, mode_name: &str) -> ModeResult {
    let config = config_5m();
    let target = config.initial_redemption_price;
    let verdict = report::evaluate_pass_fail(&scenario.metrics, target);
    let summary = output::compute_summary(&scenario.metrics, target);

    let cascade_blocks = scenario
        .metrics
        .iter()
        .filter(|m| m.liquidation_count > 1)
        .count() as u32;

    ModeResult {
        mode_name: mode_name.to_string(),
        verdict: verdict.overall.label().to_string(),
        mean_peg: summary.mean_peg_deviation,
        max_peg: summary.max_peg_deviation,
        total_liquidations: summary.total_liquidations,
        total_bad_debt: summary.total_bad_debt,
        breaker_triggers: summary.breaker_triggers,
        final_vault_count: scenario.metrics.last().unwrap().vault_count,
        cascade_blocks,
    }
}

#[test]
fn amm_liquidation_feedback() {
    let report_dir = PathBuf::from("reports/amm_liquidation");
    let _ = std::fs::create_dir_all(&report_dir);

    let scenarios = [ScenarioId::BlackThursday, ScenarioId::SustainedBear];
    let num_holders = 5;

    println!(
        "\n═══════════════════════════════════════════════════════════════════════════════════════════════════"
    );
    println!("  AMM LIQUIDATION FEEDBACK — Death Spiral Analysis");
    println!(
        "  Config: $5M AMM, 200% CR, Tick, 240-block TWAP, {} CDP holders",
        num_holders
    );
    println!("  Comparing: TWAP-based (bypass) vs Spot-based (AMM cascading)");
    println!(
        "═══════════════════════════════════════════════════════════════════════════════════════════════════\n"
    );

    for &sid in &scenarios {
        // Run both modes
        let bypass = run_with_mode(sid, false, num_holders);
        let amm_mode = run_with_mode(sid, true, num_holders);

        let config = config_5m();
        let target = config.initial_redemption_price;

        // Save reports
        let h_bypass = report::generate_report(
            &bypass.metrics,
            &config,
            &format!("{}_bypass", sid.name()),
            target,
        );
        let h_amm = report::generate_report(
            &amm_mode.metrics,
            &config,
            &format!("{}_amm_liquidation", sid.name()),
            target,
        );
        let _ = report::save_report(
            &h_bypass,
            &report_dir.join(format!("{}_bypass.html", sid.name())),
        );
        let _ = report::save_report(
            &h_amm,
            &report_dir.join(format!("{}_amm.html", sid.name())),
        );

        let r_bypass = analyze(&bypass, "TWAP bypass");
        let r_amm = analyze(&amm_mode, "AMM cascading");

        println!("  ┌─ {} ───────────────────────────────────────────", sid.name());
        println!(
            "  │ {:<18} {:>10} {:>10} {:>8} {:>10} {:>8} {:>8} {:>10}",
            "Mode", "Verdict", "MeanPeg", "MaxPeg", "Liqs", "BadDebt", "Breaker", "Cascades"
        );
        println!("  │ {}", "─".repeat(80));
        for r in [&r_bypass, &r_amm] {
            println!(
                "  │ {:<18} {:>10} {:>9.4}% {:>7.4}% {:>8} {:>10.2} {:>8} {:>10}",
                r.mode_name,
                r.verdict,
                r.mean_peg * 100.0,
                r.max_peg * 100.0,
                r.total_liquidations,
                r.total_bad_debt,
                r.breaker_triggers,
                r.cascade_blocks,
            );
        }

        // Delta analysis
        let liq_delta = r_amm.total_liquidations as i64 - r_bypass.total_liquidations as i64;
        let debt_delta = r_amm.total_bad_debt - r_bypass.total_bad_debt;
        let peg_delta = r_amm.mean_peg - r_bypass.mean_peg;

        println!("  │");
        println!("  │ Delta (AMM - bypass):");
        println!("  │   Liquidations: {:+}", liq_delta);
        println!("  │   Bad debt:     {:+.2}", debt_delta);
        println!("  │   Mean peg dev: {:+.6}", peg_delta);
        println!("  │   Surviving vaults: {} (bypass) vs {} (AMM)",
            r_bypass.final_vault_count, r_amm.final_vault_count);

        // Death spiral detection
        let has_death_spiral = r_amm.total_liquidations > r_bypass.total_liquidations
            && r_amm.cascade_blocks > 0;
        if has_death_spiral {
            println!("  │");
            println!(
                "  │ ⚠ DEATH SPIRAL DETECTED: {} cascade blocks, {} extra liquidations",
                r_amm.cascade_blocks, liq_delta
            );
        }
        println!("  └────────────────────────────────────────────────────────────\n");
    }

    // Summary comparison
    println!("  {:<20} {:<18} {:>8} {:>10} {:>12}",
        "Scenario", "Mode", "Liqs", "Bad Debt", "Cascades");
    println!("  {}", "─".repeat(72));

    for &sid in &scenarios {
        let bypass = run_with_mode(sid, false, num_holders);
        let amm = run_with_mode(sid, true, num_holders);
        let r_b = analyze(&bypass, "bypass");
        let r_a = analyze(&amm, "AMM");

        println!(
            "  {:<20} {:<18} {:>8} {:>10.2} {:>12}",
            sid.name(), "TWAP bypass", r_b.total_liquidations, r_b.total_bad_debt, r_b.cascade_blocks
        );
        println!(
            "  {:<20} {:<18} {:>8} {:>10.2} {:>12}",
            "", "AMM cascading", r_a.total_liquidations, r_a.total_bad_debt, r_a.cascade_blocks
        );
    }
    println!("  {}", "─".repeat(72));
}
