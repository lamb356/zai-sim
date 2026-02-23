/// Collateral Ratio Sensitivity Sweep (F-039)
///
/// Finds the minimum collateral ratio where the system survives crash scenarios.
/// Vaults are created at CRs relative to min_ratio (105%-140% of minimum),
/// simulating capital-efficient users opening near the threshold.
///
/// Sweep: 9 CR levels × 4 scenarios = 36 runs.
use zai_sim::agents::*;
use zai_sim::controller::ControllerConfig;
use zai_sim::output;
use zai_sim::report;
use zai_sim::scenario::{Scenario, ScenarioConfig};
use zai_sim::scenarios::{generate_prices, ScenarioId};

use std::path::PathBuf;

const BLOCKS: usize = 1000;
const SEED: u64 = 42;

fn config_with_cr(min_ratio: f64) -> ScenarioConfig {
    let mut c = ScenarioConfig::default();
    c.amm_initial_zec = 100_000.0;
    c.amm_initial_zai = 5_000_000.0;
    c.cdp_config.min_ratio = min_ratio;
    c.cdp_config.twap_window = 240;
    c.controller_config = ControllerConfig::default_tick();
    c.liquidation_config.max_liquidations_per_block = 50;
    c
}

/// Create 25 vaults at CRs relative to min_ratio.
/// 5 tiers × 5 vaults = 25 total.
/// Tiers: 105%, 110%, 115%, 125%, 140% of min_ratio.
/// Each vault: 2000 ZAI debt, collateral scaled to target CR at $50.
fn add_vaults_for_cr(scenario: &mut Scenario, min_ratio: f64) {
    let tiers = [1.05, 1.10, 1.15, 1.25, 1.40];
    let debt = 2000.0;
    let price = 50.0;
    for &tier in &tiers {
        let cr = min_ratio * tier;
        let collateral = cr * debt / price;
        for _ in 0..5 {
            scenario.cdp_holders.push(CdpHolder::new(CdpHolderConfig {
                target_ratio: cr,
                action_threshold_ratio: min_ratio * 0.95,
                reserve_zec: collateral * 0.1,
                initial_collateral: collateral,
                initial_debt: debt,
            }));
        }
    }
}

fn add_agents(scenario: &mut Scenario, min_ratio: f64) {
    scenario
        .arbers
        .push(Arbitrageur::new(ArbitrageurConfig::default()));
    scenario
        .miners
        .push(MinerAgent::new(MinerAgentConfig::default()));
    scenario
        .demand_agents
        .push(DemandAgent::new(DemandAgentConfig::default()));
    add_vaults_for_cr(scenario, min_ratio);
}

struct CrRow {
    min_ratio: f64,
    scenario_name: String,
    verdict: String,
    mean_peg: f64,
    max_peg: f64,
    liqs: u32,
    bad_debt: f64,
    max_zombie_count: u32,
    zombie_duration: u64,
    solvent: bool,
    final_solvency_ratio: f64,
}

fn run_single(min_ratio: f64, sid: ScenarioId, scenario_name: &str) -> (CrRow, Scenario) {
    let config = config_with_cr(min_ratio);
    let prices = generate_prices(sid, BLOCKS, SEED);
    let target = 50.0;

    let mut scenario = Scenario::new_with_seed(&config, SEED);
    add_agents(&mut scenario, min_ratio);
    scenario.run(&prices);

    let summary = output::compute_summary(&scenario.metrics, target);
    let verdict = report::evaluate_pass_fail(&scenario.metrics, target);

    let max_zombie_count = scenario
        .metrics
        .iter()
        .map(|m| m.zombie_vault_count)
        .max()
        .unwrap_or(0);
    let zombie_duration = scenario
        .metrics
        .iter()
        .filter(|m| m.zombie_vault_count > 0)
        .count() as u64;

    let last = scenario.metrics.last().unwrap();
    let collateral_value = last.total_collateral * last.twap_price;
    let final_solvency_ratio = if last.total_debt > 0.0 {
        collateral_value / last.total_debt
    } else {
        f64::INFINITY
    };
    let solvent = final_solvency_ratio >= 1.0;

    let row = CrRow {
        min_ratio,
        scenario_name: scenario_name.to_string(),
        verdict: verdict.overall.label().to_string(),
        mean_peg: summary.mean_peg_deviation,
        max_peg: summary.max_peg_deviation,
        liqs: summary.total_liquidations,
        bad_debt: summary.total_bad_debt,
        max_zombie_count,
        zombie_duration,
        solvent,
        final_solvency_ratio,
    };

    (row, scenario)
}

#[test]
fn cr_sensitivity_sweep() {
    let crs = [1.25, 1.5, 1.6, 1.75, 1.8, 1.9, 2.0, 2.5, 3.0];
    let scenarios: Vec<(&str, ScenarioId)> = vec![
        ("black_thursday", ScenarioId::BlackThursday),
        ("sustained_bear", ScenarioId::SustainedBear),
        ("flash_crash", ScenarioId::FlashCrash),
        ("bank_run", ScenarioId::BankRun),
    ];

    let report_dir = PathBuf::from("reports/cr_sensitivity");
    let _ = std::fs::create_dir_all(&report_dir);

    let mut rows: Vec<CrRow> = Vec::new();
    let mut entries = Vec::new();
    let target = 50.0;

    println!("\n  Running CR sensitivity sweep...");
    println!("  Config: $5M AMM, Tick controller, 240-block TWAP, 25 vaults per run");
    println!("  Vaults: 5 tiers at 105-140% of min_ratio, 2000 ZAI debt each");
    println!("  Sweep: 9 CRs × 4 scenarios = 36 runs\n");

    for &cr in &crs {
        for &(scenario_name, sid) in &scenarios {
            let run_name = format!("cr_{:.0}pct_{}", cr * 100.0, scenario_name);
            println!("  Running {}...", run_name);

            let (row, scenario) = run_single(cr, sid, scenario_name);

            let config = config_with_cr(cr);
            let verdict = report::evaluate_pass_fail(&scenario.metrics, target);
            let summary = output::compute_summary(&scenario.metrics, target);

            let html = report::generate_report(&scenario.metrics, &config, &run_name, target);
            let html_path = report_dir.join(format!("{}.html", run_name));
            report::save_report(&html, &html_path).expect("save report");

            entries.push((run_name, verdict, summary));
            rows.push(row);
        }
    }

    // Generate master index
    let master_html = report::generate_master_summary(&entries);
    let master_path = report_dir.join("index.html");
    report::save_report(&master_html, &master_path).expect("save master summary");

    // Print full results table
    println!("\n═══════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════");
    println!("  ZAI SIMULATOR — CR SENSITIVITY SWEEP (F-039)");
    println!("═══════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════");
    println!(
        "  {:<8} {:<16} {:>10} {:>10} {:>10} {:>6} {:>10} {:>8} {:>10} {:>8} {:>10}",
        "Min CR", "Scenario", "Verdict", "Mean Peg", "Max Peg", "Liqs", "Bad Debt", "Zombies", "Zmb Dur", "Solvent", "Solvency"
    );
    println!("  {}", "─".repeat(110));

    for r in &rows {
        println!(
            "  {:<8} {:<16} {:>10} {:>9.4}% {:>9.4}% {:>6} {:>10.2} {:>8} {:>10} {:>8} {:>9.2}x",
            format!("{:.0}%", r.min_ratio * 100.0),
            r.scenario_name,
            r.verdict,
            r.mean_peg * 100.0,
            r.max_peg * 100.0,
            r.liqs,
            r.bad_debt,
            r.max_zombie_count,
            r.zombie_duration,
            if r.solvent { "YES" } else { "NO" },
            r.final_solvency_ratio,
        );
    }

    println!("  {}", "─".repeat(110));

    // Per-scenario summary: CR vs bad debt
    println!("\n  Per-scenario: CR vs bad debt");
    println!(
        "  {:<16} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8}",
        "Scenario", "125%", "150%", "160%", "175%", "180%", "190%", "200%", "250%", "300%"
    );
    println!("  {}", "─".repeat(92));

    for &(scenario_name, _) in &scenarios {
        let debts: Vec<String> = crs
            .iter()
            .map(|&cr| {
                rows.iter()
                    .find(|r| r.min_ratio == cr && r.scenario_name == scenario_name)
                    .map(|r| {
                        if r.bad_debt == 0.0 {
                            "$0".to_string()
                        } else {
                            format!("${:.0}", r.bad_debt)
                        }
                    })
                    .unwrap_or_default()
            })
            .collect();
        println!(
            "  {:<16} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8}",
            scenario_name, debts[0], debts[1], debts[2], debts[3], debts[4], debts[5], debts[6], debts[7], debts[8]
        );
    }

    // Per-scenario summary: CR vs verdict
    println!("\n  Per-scenario: CR vs verdict");
    println!(
        "  {:<16} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8}",
        "Scenario", "125%", "150%", "160%", "175%", "180%", "190%", "200%", "250%", "300%"
    );
    println!("  {}", "─".repeat(92));

    for &(scenario_name, _) in &scenarios {
        let verdicts: Vec<String> = crs
            .iter()
            .map(|&cr| {
                rows.iter()
                    .find(|r| r.min_ratio == cr && r.scenario_name == scenario_name)
                    .map(|r| r.verdict.clone())
                    .unwrap_or_default()
            })
            .collect();
        println!(
            "  {:<16} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8}",
            scenario_name, verdicts[0], verdicts[1], verdicts[2], verdicts[3], verdicts[4], verdicts[5], verdicts[6], verdicts[7], verdicts[8]
        );
    }

    // Per-scenario summary: CR vs liquidations
    println!("\n  Per-scenario: CR vs liquidations");
    println!(
        "  {:<16} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8}",
        "Scenario", "125%", "150%", "160%", "175%", "180%", "190%", "200%", "250%", "300%"
    );
    println!("  {}", "─".repeat(92));

    for &(scenario_name, _) in &scenarios {
        let liqs: Vec<String> = crs
            .iter()
            .map(|&cr| {
                rows.iter()
                    .find(|r| r.min_ratio == cr && r.scenario_name == scenario_name)
                    .map(|r| format!("{}", r.liqs))
                    .unwrap_or_default()
            })
            .collect();
        println!(
            "  {:<16} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8}",
            scenario_name, liqs[0], liqs[1], liqs[2], liqs[3], liqs[4], liqs[5], liqs[6], liqs[7], liqs[8]
        );
    }

    println!("\n═══════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════");
    println!("  Reports saved to: reports/cr_sensitivity/");
    println!("  Master summary:   reports/cr_sensitivity/index.html\n");

    // Verify all reports exist
    assert!(master_path.exists(), "Master summary should exist");
    for &cr in &crs {
        for &(scenario_name, _) in &scenarios {
            let run_name = format!("cr_{:.0}pct_{}", cr * 100.0, scenario_name);
            let path = report_dir.join(format!("{}.html", run_name));
            assert!(path.exists(), "Report should exist for {}", run_name);
        }
    }
}
