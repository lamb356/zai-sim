/// LP Incentive Parameter Sweep (F-034)
///
/// Tests whether any fee/incentive configuration makes private LPs
/// self-sustaining through crashes without protocol subsidies.
///
/// Sweep: 4 fee rates × 4 penalty LP shares × 4 scenarios = 64 runs.
use zai_sim::agents::*;
use zai_sim::controller::ControllerConfig;
use zai_sim::output;
use zai_sim::report;
use zai_sim::scenario::{Scenario, ScenarioConfig};
use zai_sim::scenarios::{generate_prices, ScenarioId};

use std::path::PathBuf;

const BLOCKS: usize = 1000;
const SEED: u64 = 42;

fn config_lp_sweep(fee_rate: f64, penalty_lp_pct: f64) -> ScenarioConfig {
    let mut c = ScenarioConfig::default();
    c.amm_initial_zec = 100_000.0;
    c.amm_initial_zai = 5_000_000.0;
    c.cdp_config.min_ratio = 2.0;
    c.cdp_config.twap_window = 240;
    c.controller_config = ControllerConfig::default_tick();
    c.stability_fee_to_lps = true;
    c.cdp_config.stability_fee_rate = fee_rate;
    c.liquidation_config.liquidation_penalty_to_lps_pct = penalty_lp_pct;
    c.liquidation_config.max_liquidations_per_block = 50;
    c
}

/// Add agents: arber + miner + demand + 25 CDP holders at 210-280% CR.
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

    // 25 vaults at varying CRs (same as oracle_comparison_test)
    let vault_configs: Vec<(f64, f64, f64)> = vec![
        // ~210% CR (5 vaults)
        (500.0, 11_900.0, 50.0),
        (500.0, 11_900.0, 50.0),
        (500.0, 11_900.0, 50.0),
        (500.0, 11_900.0, 50.0),
        (500.0, 11_900.0, 50.0),
        // ~220% CR (5 vaults)
        (400.0, 9_000.0, 40.0),
        (400.0, 9_000.0, 40.0),
        (400.0, 9_000.0, 40.0),
        (400.0, 9_000.0, 40.0),
        (400.0, 9_000.0, 40.0),
        // ~230% CR (5 vaults)
        (300.0, 6_500.0, 30.0),
        (300.0, 6_500.0, 30.0),
        (300.0, 6_500.0, 30.0),
        (300.0, 6_500.0, 30.0),
        (300.0, 6_500.0, 30.0),
        // ~250% CR (5 vaults)
        (200.0, 4_000.0, 20.0),
        (200.0, 4_000.0, 20.0),
        (200.0, 4_000.0, 20.0),
        (200.0, 4_000.0, 20.0),
        (200.0, 4_000.0, 20.0),
        // ~280% CR (5 vaults)
        (200.0, 3_570.0, 20.0),
        (200.0, 3_570.0, 20.0),
        (200.0, 3_570.0, 20.0),
        (200.0, 3_570.0, 20.0),
        (200.0, 3_570.0, 20.0),
    ];

    for (collateral, debt, reserve) in vault_configs {
        scenario.cdp_holders.push(CdpHolder::new(CdpHolderConfig {
            target_ratio: 2.5,
            action_threshold_ratio: 1.8,
            reserve_zec: reserve,
            initial_collateral: collateral,
            initial_debt: debt,
        }));
    }
}

struct SweepRow {
    fee_rate: f64,
    penalty_lp_pct: f64,
    scenario_name: String,
    entry_value_zai: f64,
    end_value_zai: f64,
    cumulative_fees: f64,
    il_pct: f64,
    net_pnl: f64,
    profitable: bool,
    mean_peg: f64,
    verdict: String,
}

fn run_single(
    fee_rate: f64,
    penalty_lp_pct: f64,
    sid: ScenarioId,
) -> SweepRow {
    let config = config_lp_sweep(fee_rate, penalty_lp_pct);
    let initial_price = config.amm_initial_zai / config.amm_initial_zec; // $50
    let entry_value_zai = config.amm_initial_zec * initial_price + config.amm_initial_zai;
    let target = config.initial_redemption_price;

    let prices = generate_prices(sid, BLOCKS, SEED);
    let mut scenario = Scenario::new_with_seed(&config, SEED);
    add_agents(&mut scenario);
    scenario.run(&prices);

    let last = scenario.metrics.last().unwrap();

    // Genesis LP owns all shares initially. Since no LpAgent or IlAwareLpAgent
    // is added, genesis fraction stays 1.0 throughout.
    let genesis_shares = scenario.amm.lp_shares.get("genesis").copied().unwrap_or(0.0);
    let genesis_fraction = if scenario.amm.total_lp_shares > 0.0 {
        genesis_shares / scenario.amm.total_lp_shares
    } else {
        0.0
    };

    let end_value_zai =
        (last.amm_reserve_zec * last.external_price + last.amm_reserve_zai) * genesis_fraction;
    let cumulative_fees = last.cumulative_fees_zai * genesis_fraction;
    let il_pct = last.cumulative_il_pct;
    let net_pnl = end_value_zai - entry_value_zai;

    let summary = output::compute_summary(&scenario.metrics, target);
    let verdict_result = report::evaluate_pass_fail(&scenario.metrics, target);

    SweepRow {
        fee_rate,
        penalty_lp_pct,
        scenario_name: sid.name().to_string(),
        entry_value_zai,
        end_value_zai,
        cumulative_fees,
        il_pct,
        net_pnl,
        profitable: net_pnl > 0.0,
        mean_peg: summary.mean_peg_deviation,
        verdict: verdict_result.overall.label().to_string(),
    }
}

#[test]
fn lp_incentive_sweep() {
    let fee_rates = [0.02, 0.05, 0.10, 0.15];
    let penalty_lp_pcts = [0.0, 0.25, 0.50, 1.0];
    let scenarios = [
        ScenarioId::SteadyState,
        ScenarioId::BlackThursday,
        ScenarioId::SustainedBear,
        ScenarioId::FlashCrash,
    ];

    let report_dir = PathBuf::from("reports/lp_incentive");
    let _ = std::fs::create_dir_all(&report_dir);

    let mut rows: Vec<SweepRow> = Vec::new();
    let mut report_entries: Vec<(String, report::PassFailResult, output::SummaryMetrics)> =
        Vec::new();

    println!("\n  Running LP incentive parameter sweep ({} blocks, seed={})...", BLOCKS, SEED);
    println!("  Config: $5M AMM, 200% CR, Tick, 240-TWAP, stability_fee_to_lps=true");
    println!("  25 vaults at 210-280% CR for fee generation + liquidation exposure");
    println!(
        "  Sweep: {} fee rates x {} penalty shares x {} scenarios = {} runs\n",
        fee_rates.len(),
        penalty_lp_pcts.len(),
        scenarios.len(),
        fee_rates.len() * penalty_lp_pcts.len() * scenarios.len()
    );

    for &fee_rate in &fee_rates {
        for &penalty_lp_pct in &penalty_lp_pcts {
            for &sid in &scenarios {
                let row = run_single(fee_rate, penalty_lp_pct, sid);

                // Generate per-run HTML report
                let config = config_lp_sweep(fee_rate, penalty_lp_pct);
                let prices = generate_prices(sid, BLOCKS, SEED);
                let mut scenario = Scenario::new_with_seed(&config, SEED);
                add_agents(&mut scenario);
                scenario.run(&prices);

                let run_name = format!(
                    "fee{:.0}pct_pen{:.0}pct_{}",
                    fee_rate * 100.0,
                    penalty_lp_pct * 100.0,
                    sid.name()
                );
                let target = config.initial_redemption_price;

                let html = report::generate_report(
                    &scenario.metrics,
                    &config,
                    &run_name,
                    target,
                );
                let html_path = report_dir.join(format!("{}.html", run_name));
                report::save_report(&html, &html_path).expect("save HTML report");

                let verdict = report::evaluate_pass_fail(&scenario.metrics, target);
                let summary = output::compute_summary(&scenario.metrics, target);
                report_entries.push((run_name, verdict, summary));

                rows.push(row);
            }
        }
    }

    // Generate master index
    let master_html = report::generate_master_summary(&report_entries);
    let master_path = report_dir.join("index.html");
    report::save_report(&master_html, &master_path).expect("save master index");

    // ── Print summary table ────────────────────────────────────────────────
    println!("═══════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════");
    println!("  ZAI SIMULATOR — LP INCENTIVE PARAMETER SWEEP (F-034)");
    println!("═══════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════");
    println!(
        "  {:<6} {:<6} {:<16} {:>12} {:>12} {:>12} {:>8} {:>10} {:>6} {:>10} {:>10}",
        "Fee%", "Pen%", "Scenario", "EntryVal", "EndVal", "Fees", "IL%", "NetP&L", "Prof", "MeanPeg", "Verdict"
    );
    println!("  {}", "─".repeat(120));

    let mut profitable_count = 0u32;
    let mut total_count = 0u32;

    for r in &rows {
        total_count += 1;
        if r.profitable {
            profitable_count += 1;
        }

        println!(
            "  {:>5.0}% {:>5.0}% {:<16} ${:>10.0} ${:>10.0} ${:>10.2} {:>7.3}% ${:>+9.0} {:>6} {:>9.2}% {:>10}",
            r.fee_rate * 100.0,
            r.penalty_lp_pct * 100.0,
            r.scenario_name,
            r.entry_value_zai,
            r.end_value_zai,
            r.cumulative_fees,
            r.il_pct * 100.0,
            r.net_pnl,
            if r.profitable { "YES" } else { "no" },
            r.mean_peg * 100.0,
            r.verdict,
        );
    }

    println!("  {}", "─".repeat(120));
    println!(
        "  PROFITABLE: {} / {} runs ({:.1}%)",
        profitable_count,
        total_count,
        profitable_count as f64 / total_count as f64 * 100.0
    );

    // Summary by scenario
    println!("\n  ── BY SCENARIO ──────────────────────────────────────────────────");
    for &sid in &scenarios {
        let scenario_rows: Vec<&SweepRow> = rows
            .iter()
            .filter(|r| r.scenario_name == sid.name())
            .collect();
        let prof = scenario_rows.iter().filter(|r| r.profitable).count();
        let best = scenario_rows
            .iter()
            .max_by(|a, b| a.net_pnl.partial_cmp(&b.net_pnl).unwrap())
            .unwrap();
        println!(
            "  {:<16}: {} / {} profitable. Best: fee={:.0}% pen={:.0}% -> ${:+.0} P&L",
            sid.name(),
            prof,
            scenario_rows.len(),
            best.fee_rate * 100.0,
            best.penalty_lp_pct * 100.0,
            best.net_pnl,
        );
    }

    // Summary by config (across scenarios)
    println!("\n  ── BY CONFIG (all scenarios) ────────────────────────────────────");
    for &fee_rate in &fee_rates {
        for &penalty_lp_pct in &penalty_lp_pcts {
            let config_rows: Vec<&SweepRow> = rows
                .iter()
                .filter(|r| {
                    (r.fee_rate - fee_rate).abs() < 1e-6
                        && (r.penalty_lp_pct - penalty_lp_pct).abs() < 1e-6
                })
                .collect();
            let prof = config_rows.iter().filter(|r| r.profitable).count();
            let total_pnl: f64 = config_rows.iter().map(|r| r.net_pnl).sum();
            println!(
                "  fee={:>3.0}% pen={:>3.0}%: {} / {} profitable, total P&L: ${:+.0}",
                fee_rate * 100.0,
                penalty_lp_pct * 100.0,
                prof,
                config_rows.len(),
                total_pnl,
            );
        }
    }

    println!("\n═══════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════");
    println!("  Reports: reports/lp_incentive/");
    println!("  Master:  reports/lp_incentive/index.html");
    println!("═══════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════\n");

    // Verify reports exist
    assert!(master_path.exists(), "Master index should exist");
    assert!(
        rows.len() == 64,
        "Should have 64 runs, got {}",
        rows.len()
    );
}
