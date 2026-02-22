/// Task 7: Zombie Vault Mitigation — Early Detection vs Cascade Risk
///
/// Tests the `zombie_detector` option: when the gap between TWAP-based CR and
/// spot-based CR exceeds a threshold (default 0.5), the vault is flagged and
/// liquidated using spot price instead of TWAP.
///
/// The key question: does catching zombies early prevent bad debt, or does it
/// trigger unnecessary liquidation cascades during flash crashes?
///
/// Compares:
///   - Unmitigated: standard TWAP-only liquidation
///   - Zombie detector (threshold=0.5): moderate sensitivity
///   - Zombie detector (threshold=0.3): aggressive detection
///   - Zombie detector (threshold=1.0): conservative detection
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

fn run_with_zombie_config(
    sid: ScenarioId,
    zombie_detector: bool,
    gap_threshold: f64,
    num_holders: usize,
) -> Scenario {
    let mut config = config_5m();
    config.zombie_detector = zombie_detector;
    config.zombie_gap_threshold = gap_threshold;

    let prices = generate_prices(sid, BLOCKS, SEED);
    let mut scenario = Scenario::new(&config);

    scenario
        .arbers
        .push(Arbitrageur::new(ArbitrageurConfig::default()));
    scenario
        .miners
        .push(MinerAgent::new(MinerAgentConfig::default()));

    // CDP holders: mix of conservative and aggressive ratios
    for i in 0..num_holders {
        let target = 2.0 + (i as f64) * 0.5; // 2.0, 2.5, 3.0, 3.5, 4.0
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

struct ZombieResult {
    label: String,
    verdict: String,
    mean_peg: f64,
    max_peg: f64,
    total_liquidations: u32,
    total_bad_debt: f64,
    _breaker_triggers: u32,
    max_zombie_count: u32,
    zombie_duration: u64,
    max_zombie_gap: f64,
    _final_vault_count: u64,
}

fn analyze(scenario: &Scenario, label: &str) -> ZombieResult {
    let config = config_5m();
    let target = config.initial_redemption_price;
    let verdict = report::evaluate_pass_fail(&scenario.metrics, target);
    let summary = output::compute_summary(&scenario.metrics, target);

    let mut max_zombie_count = 0u32;
    let mut zombie_duration = 0u64;
    let mut max_zombie_gap = 0.0f64;

    for m in &scenario.metrics {
        if m.zombie_vault_count > max_zombie_count {
            max_zombie_count = m.zombie_vault_count;
        }
        if m.zombie_vault_count > 0 {
            zombie_duration += 1;
        }
        if m.max_zombie_gap > max_zombie_gap {
            max_zombie_gap = m.max_zombie_gap;
        }
    }

    ZombieResult {
        label: label.to_string(),
        verdict: verdict.overall.label().to_string(),
        mean_peg: summary.mean_peg_deviation,
        max_peg: summary.max_peg_deviation,
        total_liquidations: summary.total_liquidations,
        total_bad_debt: summary.total_bad_debt,
        _breaker_triggers: summary.breaker_triggers,
        max_zombie_count,
        zombie_duration,
        max_zombie_gap,
        _final_vault_count: scenario.metrics.last().unwrap().vault_count,
    }
}

#[test]
fn zombie_mitigation_comparison() {
    let report_dir = PathBuf::from("reports/zombie_mitigation");
    let _ = std::fs::create_dir_all(&report_dir);

    let scenarios = [ScenarioId::SustainedBear, ScenarioId::BlackThursday];
    let thresholds = [
        ("unmitigated", false, 0.0),
        ("threshold_0.3", true, 0.3),
        ("threshold_0.5", true, 0.5),
        ("threshold_1.0", true, 1.0),
    ];
    let num_holders = 5;

    println!(
        "\n═══════════════════════════════════════════════════════════════════════════════════════════════════"
    );
    println!("  ZOMBIE VAULT MITIGATION — Early Detection vs Cascade Risk");
    println!(
        "  Config: $5M AMM, 200% CR, Tick, 240-block TWAP, {} CDP holders",
        num_holders
    );
    println!("  Thresholds: unmitigated, 0.3 (aggressive), 0.5 (default), 1.0 (conservative)");
    println!(
        "═══════════════════════════════════════════════════════════════════════════════════════════════════\n"
    );

    for &sid in &scenarios {
        println!(
            "  ┌─ {} ──────────────────────────────────────────────────────────",
            sid.name()
        );
        println!(
            "  │ {:<16} {:>8} {:>8} {:>8} {:>8} {:>10} {:>8} {:>8} {:>8}",
            "Config", "Verdict", "MeanPeg", "MaxPeg", "Liqs", "BadDebt", "Zombies", "ZDur", "ZGap"
        );
        println!("  │ {}", "─".repeat(90));

        let mut results: Vec<ZombieResult> = Vec::new();

        for &(label, enabled, threshold) in &thresholds {
            let scenario = run_with_zombie_config(sid, enabled, threshold, num_holders);

            // Save report
            let config = config_5m();
            let target = config.initial_redemption_price;
            let html = report::generate_report(
                &scenario.metrics,
                &config,
                &format!("{}_{}", sid.name(), label),
                target,
            );
            let _ = report::save_report(
                &html,
                &report_dir.join(format!("{}_{}.html", sid.name(), label)),
            );

            let r = analyze(&scenario, label);

            println!(
                "  │ {:<16} {:>8} {:>7.4}% {:>7.4}% {:>8} {:>10.2} {:>8} {:>7}b {:>7.3}",
                r.label,
                r.verdict,
                r.mean_peg * 100.0,
                r.max_peg * 100.0,
                r.total_liquidations,
                r.total_bad_debt,
                r.max_zombie_count,
                r.zombie_duration,
                r.max_zombie_gap,
            );

            results.push(r);
        }

        // Analysis: compare unmitigated vs best mitigated
        let unmitigated = &results[0];
        println!("  │");
        println!("  │ Comparison to unmitigated:");

        for r in results.iter().skip(1) {
            let liq_delta = r.total_liquidations as i64 - unmitigated.total_liquidations as i64;
            let debt_delta = r.total_bad_debt - unmitigated.total_bad_debt;
            let zombie_delta = r.max_zombie_count as i64 - unmitigated.max_zombie_count as i64;
            let dur_delta = r.zombie_duration as i64 - unmitigated.zombie_duration as i64;

            println!(
                "  │   {} → liqs {:+}, bad_debt {:+.2}, zombies {:+}, duration {:+}b",
                r.label, liq_delta, debt_delta, zombie_delta, dur_delta,
            );

            // Determine if mitigation helped or hurt
            let helped = r.total_bad_debt <= unmitigated.total_bad_debt
                && r.max_zombie_count <= unmitigated.max_zombie_count;
            let cascade_risk = r.total_liquidations > unmitigated.total_liquidations + 2;

            if helped && !cascade_risk {
                println!("  │     → HELPED: reduced zombie risk without cascade");
            } else if cascade_risk {
                println!("  │     → CASCADE RISK: triggered {} extra liquidations", liq_delta);
            } else {
                println!("  │     → NEUTRAL: no significant change");
            }
        }

        println!(
            "  └──────────────────────────────────────────────────────────────────\n"
        );
    }

    // Final summary table
    println!("  ZOMBIE MITIGATION SUMMARY");
    println!(
        "  {:<20} {:<16} {:>8} {:>10} {:>8} {:>8}",
        "Scenario", "Config", "Liqs", "Bad Debt", "Zombies", "Verdict"
    );
    println!("  {}", "─".repeat(76));

    for &sid in &scenarios {
        for &(label, enabled, threshold) in &thresholds {
            let scenario = run_with_zombie_config(sid, enabled, threshold, num_holders);
            let r = analyze(&scenario, label);
            println!(
                "  {:<20} {:<16} {:>8} {:>10.2} {:>8} {:>8}",
                if label == "unmitigated" {
                    sid.name()
                } else {
                    ""
                },
                r.label,
                r.total_liquidations,
                r.total_bad_debt,
                r.max_zombie_count,
                r.verdict,
            );
        }
    }
    println!("  {}", "─".repeat(76));

    // Key question assertion: zombie detector should not make things worse
    // (it should either reduce zombies or at worst be neutral)
    for &sid in &scenarios {
        let unmitigated = run_with_zombie_config(sid, false, 0.0, num_holders);
        let mitigated = run_with_zombie_config(sid, true, 0.5, num_holders);
        let r_u = analyze(&unmitigated, "unmitigated");
        let r_m = analyze(&mitigated, "mitigated");

        // Bad debt should not increase dramatically (allow 10% tolerance)
        assert!(
            r_m.total_bad_debt <= r_u.total_bad_debt * 1.1 + 1.0,
            "{}: zombie detector increased bad debt from {:.2} to {:.2}",
            sid.name(),
            r_u.total_bad_debt,
            r_m.total_bad_debt,
        );
    }
}
