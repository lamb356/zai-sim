/// AMM Fee Sensitivity Sweep (F-038)
///
/// Tests how swap fee level affects system stability. Higher fees slow arber
/// repricing (potentially good per F-028) but may reduce trading volume and
/// worsen peg tracking during normal operation.
///
/// Sweep: 6 fee levels × 4 scenarios = 24 runs.
use zai_sim::agents::*;
use zai_sim::controller::ControllerConfig;
use zai_sim::output;
use zai_sim::report;
use zai_sim::scenario::{Scenario, ScenarioConfig};
use zai_sim::scenarios::{generate_prices, ScenarioId};

use std::path::PathBuf;

const BLOCKS: usize = 1000;
const SEED: u64 = 42;

fn config_with_fee(fee: f64) -> ScenarioConfig {
    let mut c = ScenarioConfig::default();
    c.amm_initial_zec = 100_000.0;
    c.amm_initial_zai = 5_000_000.0;
    c.amm_swap_fee = fee;
    c.cdp_config.min_ratio = 2.0;
    c.cdp_config.twap_window = 240;
    c.controller_config = ControllerConfig::default_tick();
    c.liquidation_config.max_liquidations_per_block = 50;
    c
}

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
    scenario
        .cdp_holders
        .push(CdpHolder::new(CdpHolderConfig::default()));
}

struct FeeRow {
    fee: f64,
    scenario_name: String,
    verdict: String,
    mean_peg: f64,
    max_peg: f64,
    exhaustion_block: Option<u64>,
    fees_generated: f64,
    liqs: u32,
    bad_debt: f64,
}

fn run_single(fee: f64, sid: ScenarioId, scenario_name: &str) -> (FeeRow, Scenario) {
    let config = config_with_fee(fee);
    let prices = generate_prices(sid, BLOCKS, SEED);
    let target = 50.0;

    let mut scenario = Scenario::new_with_seed(&config, SEED);
    add_agents(&mut scenario);
    scenario.run(&prices);

    let summary = output::compute_summary(&scenario.metrics, target);
    let verdict = report::evaluate_pass_fail(&scenario.metrics, target);

    let exhaustion_block = scenario
        .metrics
        .iter()
        .position(|m| m.arber_zec_total < 1.0 && m.arber_zai_total < 1.0)
        .map(|i| i as u64 + 1);

    let fees_generated = scenario.metrics.last().unwrap().cumulative_fees_zai;

    let row = FeeRow {
        fee,
        scenario_name: scenario_name.to_string(),
        verdict: verdict.overall.label().to_string(),
        mean_peg: summary.mean_peg_deviation,
        max_peg: summary.max_peg_deviation,
        exhaustion_block,
        fees_generated,
        liqs: summary.total_liquidations,
        bad_debt: summary.total_bad_debt,
    };

    (row, scenario)
}

#[test]
fn amm_fee_sensitivity_sweep() {
    let fees = [0.001, 0.003, 0.005, 0.01, 0.02, 0.05];
    let scenarios: Vec<(&str, ScenarioId)> = vec![
        ("steady_state", ScenarioId::SteadyState),
        ("black_thursday", ScenarioId::BlackThursday),
        ("sustained_bear", ScenarioId::SustainedBear),
        ("flash_crash", ScenarioId::FlashCrash),
    ];

    let report_dir = PathBuf::from("reports/amm_fee_sensitivity");
    let _ = std::fs::create_dir_all(&report_dir);

    let mut rows: Vec<FeeRow> = Vec::new();
    let mut entries = Vec::new();
    let target = 50.0;

    println!("\n  Running AMM fee sensitivity sweep...");
    println!("  Config: $5M AMM, 200% CR, Tick controller, 240-block TWAP");
    println!("  Sweep: 6 fees × 4 scenarios = 24 runs\n");

    for &fee in &fees {
        for &(scenario_name, sid) in &scenarios {
            let run_name = format!("fee_{:.1}pct_{}", fee * 100.0, scenario_name);
            println!("  Running {}...", run_name);

            let (row, scenario) = run_single(fee, sid, scenario_name);

            let config = config_with_fee(fee);
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

    // Print summary table
    println!("\n═══════════════════════════════════════════════════════════════════════════════════════════════════════════════════");
    println!("  ZAI SIMULATOR — AMM FEE SENSITIVITY SWEEP (F-038)");
    println!("═══════════════════════════════════════════════════════════════════════════════════════════════════════════════════");
    println!(
        "  {:<8} {:<16} {:>10} {:>10} {:>10} {:>12} {:>12} {:>6} {:>10}",
        "Fee", "Scenario", "Verdict", "Mean Peg", "Max Peg", "Exhaust Blk", "Fees ($)", "Liqs", "Bad Debt"
    );
    println!("  {}", "─".repeat(100));

    for r in &rows {
        let exhaust_str = match r.exhaustion_block {
            Some(b) => format!("{}", b),
            None => "never".to_string(),
        };
        println!(
            "  {:<8} {:<16} {:>10} {:>9.4}% {:>9.4}% {:>12} {:>11.2} {:>6} {:>10.2}",
            format!("{:.1}%", r.fee * 100.0),
            r.scenario_name,
            r.verdict,
            r.mean_peg * 100.0,
            r.max_peg * 100.0,
            exhaust_str,
            r.fees_generated,
            r.liqs,
            r.bad_debt,
        );
    }

    println!("  {}", "─".repeat(100));

    // Print per-scenario analysis: fee vs mean_peg
    println!("\n  Per-scenario: fee impact on mean peg deviation");
    println!("  {:<16} {:>10} {:>10} {:>10} {:>10} {:>10} {:>10}", "Scenario", "0.1%", "0.3%", "0.5%", "1.0%", "2.0%", "5.0%");
    println!("  {}", "─".repeat(76));

    for &(scenario_name, _) in &scenarios {
        let pegs: Vec<String> = fees
            .iter()
            .map(|&fee| {
                rows.iter()
                    .find(|r| r.fee == fee && r.scenario_name == scenario_name)
                    .map(|r| format!("{:.2}%", r.mean_peg * 100.0))
                    .unwrap_or_default()
            })
            .collect();
        println!(
            "  {:<16} {:>10} {:>10} {:>10} {:>10} {:>10} {:>10}",
            scenario_name, pegs[0], pegs[1], pegs[2], pegs[3], pegs[4], pegs[5]
        );
    }

    // Print per-scenario analysis: fee vs exhaustion block
    println!("\n  Per-scenario: fee impact on arber exhaustion block");
    println!("  {:<16} {:>10} {:>10} {:>10} {:>10} {:>10} {:>10}", "Scenario", "0.1%", "0.3%", "0.5%", "1.0%", "2.0%", "5.0%");
    println!("  {}", "─".repeat(76));

    for &(scenario_name, _) in &scenarios {
        let blocks: Vec<String> = fees
            .iter()
            .map(|&fee| {
                rows.iter()
                    .find(|r| r.fee == fee && r.scenario_name == scenario_name)
                    .map(|r| match r.exhaustion_block {
                        Some(b) => format!("{}", b),
                        None => "never".to_string(),
                    })
                    .unwrap_or_default()
            })
            .collect();
        println!(
            "  {:<16} {:>10} {:>10} {:>10} {:>10} {:>10} {:>10}",
            scenario_name, blocks[0], blocks[1], blocks[2], blocks[3], blocks[4], blocks[5]
        );
    }

    println!("\n═══════════════════════════════════════════════════════════════════════════════════════════════════════════════════");
    println!("  Reports saved to: reports/amm_fee_sensitivity/");
    println!("  Master summary:   reports/amm_fee_sensitivity/index.html\n");

    // Verify all reports exist
    assert!(master_path.exists(), "Master summary should exist");
    for &fee in &fees {
        for &(scenario_name, _) in &scenarios {
            let run_name = format!("fee_{:.1}pct_{}", fee * 100.0, scenario_name);
            let path = report_dir.join(format!("{}.html", run_name));
            assert!(path.exists(), "Report should exist for {}", run_name);
        }
    }
}
