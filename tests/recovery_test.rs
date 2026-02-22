/// Recovery Scenario — F-029
///
/// Tests whether the AMM re-converges and zombie vaults resolve during price recovery.
///
/// Price path:
///   Phase 1: $50 → $25 over 500 blocks (linear decline, ~10.4 hours)
///   Phase 2: Hold $25 for 5,000 blocks (~4.3 days)
///   Phase 3: $25 → $50 over 5,000 blocks (linear recovery, ~4.3 days)
///   Total: 10,500 blocks (~9.1 days)
///
/// Config: $5M AMM, 200% CR, Tick controller, 240-block TWAP
use zai_sim::agents::*;
use zai_sim::controller::ControllerConfig;
use zai_sim::output;
use zai_sim::report;
use zai_sim::scenario::{Scenario, ScenarioConfig};

const TOTAL_BLOCKS: usize = 10_500;

fn recovery_prices(blocks: usize) -> Vec<f64> {
    let decline_end = 500;
    let hold_end = 5500;
    (0..blocks)
        .map(|i| {
            if i < decline_end {
                50.0 - 25.0 * (i as f64 / decline_end as f64)
            } else if i < hold_end {
                25.0
            } else {
                25.0 + 25.0 * ((i - hold_end) as f64 / (blocks - hold_end) as f64)
            }
        })
        .collect()
}

fn config_5m() -> ScenarioConfig {
    let mut config = ScenarioConfig::default();
    config.amm_initial_zec = 100_000.0;
    config.amm_initial_zai = 5_000_000.0;
    config.cdp_config.min_ratio = 2.0;
    config.cdp_config.twap_window = 240;
    config.controller_config = ControllerConfig::default_tick();
    config
}

#[test]
fn recovery_scenario() {
    println!(
        "\n═══════════════════════════════════════════════════════════════════════════════════════════════════"
    );
    println!("  RECOVERY SCENARIO — F-029");
    println!("  Does the system self-heal when price recovers?");
    println!("  Price: $50 → $25 (500 blocks) → hold $25 (5000 blocks) → $25 → $50 (5000 blocks)");
    println!(
        "═══════════════════════════════════════════════════════════════════════════════════════════════════\n"
    );

    let config = config_5m();
    let target = config.initial_redemption_price;
    let prices = recovery_prices(TOTAL_BLOCKS);
    let mut scenario = Scenario::new(&config);

    // Standard arber + miner
    scenario
        .arbers
        .push(Arbitrageur::new(ArbitrageurConfig::default()));
    scenario
        .miners
        .push(MinerAgent::new(MinerAgentConfig::default()));

    // 5 CDP holders
    for _ in 0..5 {
        scenario.cdp_holders.push(CdpHolder::new(CdpHolderConfig {
            target_ratio: 2.5,
            action_threshold_ratio: 1.8,
            reserve_zec: 100.0,
            initial_collateral: 50.0,
            initial_debt: 1000.0,
        }));
    }

    scenario.run(&prices);

    let summary = output::compute_summary(&scenario.metrics, target);
    let verdict = report::evaluate_pass_fail(&scenario.metrics, target);

    // Phase analysis
    let phase1_end = 500;
    let phase2_end = 5500;

    // End of phase 1 (crash bottom)
    let p1_metrics = &scenario.metrics[phase1_end - 1];
    let p1_spot = p1_metrics.amm_spot_price;
    let p1_ext = p1_metrics.external_price;
    let p1_gap_pct = ((p1_spot - p1_ext) / p1_ext).abs() * 100.0;

    // End of phase 2 (held at bottom)
    let p2_metrics = &scenario.metrics[phase2_end - 1];
    let p2_spot = p2_metrics.amm_spot_price;
    let p2_ext = p2_metrics.external_price;
    let p2_gap_pct = ((p2_spot - p2_ext) / p2_ext).abs() * 100.0;
    let p2_zombies = p2_metrics.zombie_vault_count;

    // End of phase 3 (recovery complete)
    let p3_metrics = scenario.metrics.last().unwrap();
    let p3_spot = p3_metrics.amm_spot_price;
    let p3_ext = p3_metrics.external_price;
    let p3_gap_pct = ((p3_spot - p3_ext) / p3_ext).abs() * 100.0;
    let p3_zombies = p3_metrics.zombie_vault_count;
    let p3_cr_gap = p3_metrics.mean_collateral_ratio_twap - p3_metrics.mean_collateral_ratio_ext;

    // Max zombies during phases
    let max_zombies_p2 = scenario.metrics[phase1_end..phase2_end]
        .iter()
        .map(|m| m.zombie_vault_count)
        .max()
        .unwrap_or(0);

    let max_zombies_p3 = scenario.metrics[phase2_end..]
        .iter()
        .map(|m| m.zombie_vault_count)
        .max()
        .unwrap_or(0);

    // Mean peg across all phases
    let mean_dev_all = summary.mean_peg_deviation * 100.0;

    println!("  ── Overall ──");
    println!(
        "  Verdict: {}  |  Mean Peg: {:.2}%  |  Max Peg: {:.2}%",
        verdict.overall.label(),
        mean_dev_all,
        summary.max_peg_deviation * 100.0,
    );

    println!("\n  ── Phase Analysis ──");
    println!(
        "  {:<25} {:>10} {:>10} {:>8} {:>8} {:>8}",
        "Phase", "AMM Spot", "External", "Gap %", "Zombies", "CR Gap"
    );
    println!("  {}", "─".repeat(75));
    println!(
        "  {:<25} {:>10.2} {:>10.2} {:>7.2}% {:>8} {:>8}",
        "Phase 1 end (crash)", p1_spot, p1_ext, p1_gap_pct, p1_metrics.zombie_vault_count, ""
    );
    println!(
        "  {:<25} {:>10.2} {:>10.2} {:>7.2}% {:>8} {:>8}",
        "Phase 2 end (hold)", p2_spot, p2_ext, p2_gap_pct, p2_zombies, ""
    );
    println!(
        "  {:<25} {:>10.2} {:>10.2} {:>7.2}% {:>8} {:>7.2}",
        "Phase 3 end (recovery)", p3_spot, p3_ext, p3_gap_pct, p3_zombies, p3_cr_gap
    );

    println!("\n  ── Recovery Indicators ──");
    println!(
        "  AMM re-converged to external? {:.2}% gap at final block (< 5% = yes)",
        p3_gap_pct
    );
    println!(
        "  Max zombies during phase 2 (crash+hold): {}",
        max_zombies_p2
    );
    println!(
        "  Max zombies during phase 3 (recovery): {}",
        max_zombies_p3
    );
    println!(
        "  Zombies resolved at end? {} remaining (0 = fully resolved)",
        p3_zombies
    );
    println!(
        "  Final CR gap (TWAP - ext): {:.4}",
        p3_cr_gap
    );

    let converged = p3_gap_pct < 5.0;
    let zombies_resolved = p3_zombies == 0;
    println!("\n  ── Conclusion ──");
    if converged && zombies_resolved {
        println!("  SYSTEM SELF-HEALS: AMM re-converges and zombie vaults resolve during recovery");
    } else if converged {
        println!("  PARTIAL HEAL: AMM re-converges but {} zombie vaults persist", p3_zombies);
    } else {
        println!(
            "  NO SELF-HEAL: AMM still diverged by {:.2}% with {} zombies at end of recovery",
            p3_gap_pct, p3_zombies
        );
    }

    println!(
        "\n═══════════════════════════════════════════════════════════════════════════════════════════════════\n"
    );
}
