use zai_sim::agents::*;
use zai_sim::controller::ControllerConfig;
use zai_sim::output;
use zai_sim::report;
use zai_sim::scenario::{Scenario, ScenarioConfig};
use zai_sim::scenarios::{generate_prices, ScenarioId};

use std::path::PathBuf;

const BLOCKS: usize = 1000;
const SEED: u64 = 42;

fn config_5m_200cr() -> ScenarioConfig {
    let mut config = ScenarioConfig::default();
    config.amm_initial_zec = 100_000.0;
    config.amm_initial_zai = 5_000_000.0;
    config.cdp_config.min_ratio = 2.0;
    config.cdp_config.twap_window = 240;
    config.controller_config = ControllerConfig::default_tick();
    // Raise velocity limit so cascading liquidations can actually happen
    config.liquidation_config.max_liquidations_per_block = 50;
    config
}

fn config_oracle_based() -> ScenarioConfig {
    let mut config = config_5m_200cr();
    config.use_external_oracle_for_liquidation = true;
    config.use_amm_liquidation = true; // sell through AMM to create feedback loop
    config
}

/// Add agents including many CDP holders at various collateral ratios.
/// At $50/ZEC and 200% min ratio, a vault with collateral C and debt D
/// has CR = (C * 50) / D. We create vaults at 210%-280% CR so they
/// become liquidatable at different price levels during crashes.
///
/// Total vault exposure: ~5000 ZEC collateral, ~$100K+ debt
fn add_heavy_vault_agents(scenario: &mut Scenario) {
    // Arbitrageur and miner (standard)
    scenario
        .arbers
        .push(Arbitrageur::new(ArbitrageurConfig::default()));
    scenario
        .miners
        .push(MinerAgent::new(MinerAgentConfig::default()));

    // Demand agent
    scenario
        .demand_agents
        .push(DemandAgent::new(DemandAgentConfig::default()));

    // Many CDP holders at varying collateral ratios
    // At $50/ZEC: CR = (collateral * 50) / debt
    // Vault becomes liquidatable when price drops to: (debt * min_ratio) / collateral
    // With min_ratio=2.0:
    //   CR 210% → liquidatable at $47.62 (4.8% drop)
    //   CR 220% → liquidatable at $45.45 (9.1% drop)
    //   CR 230% → liquidatable at $43.48 (13.0% drop)
    //   CR 250% → liquidatable at $40.00 (20.0% drop)
    //   CR 280% → liquidatable at $35.71 (28.6% drop)

    let vault_configs: Vec<(f64, f64, f64)> = vec![
        // (collateral_zec, debt_zai, reserve_zec)
        // ~210% CR vaults — first to liquidate (small price drop)
        (500.0, 11_900.0, 50.0),
        (500.0, 11_900.0, 50.0),
        (500.0, 11_900.0, 50.0),
        (500.0, 11_900.0, 50.0),
        (500.0, 11_900.0, 50.0),
        // ~220% CR vaults — liquidate at ~9% drop
        (400.0, 9_000.0, 40.0),
        (400.0, 9_000.0, 40.0),
        (400.0, 9_000.0, 40.0),
        (400.0, 9_000.0, 40.0),
        (400.0, 9_000.0, 40.0),
        // ~230% CR vaults — liquidate at ~13% drop
        (300.0, 6_500.0, 30.0),
        (300.0, 6_500.0, 30.0),
        (300.0, 6_500.0, 30.0),
        (300.0, 6_500.0, 30.0),
        (300.0, 6_500.0, 30.0),
        // ~250% CR vaults — liquidate at ~20% drop
        (200.0, 4_000.0, 20.0),
        (200.0, 4_000.0, 20.0),
        (200.0, 4_000.0, 20.0),
        (200.0, 4_000.0, 20.0),
        (200.0, 4_000.0, 20.0),
        // ~280% CR vaults — only liquidate in severe crash
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

struct ComparisonRow {
    name: String,
    // Oracle-free (TWAP-based)
    of_verdict: String,
    of_mean_peg: f64,
    of_liqs: u32,
    of_bad_debt: f64,
    // Oracle-based
    ob_verdict: String,
    ob_mean_peg: f64,
    ob_liqs: u32,
    ob_bad_debt: f64,
}

#[test]
fn oracle_comparison_suite() {
    let test_scenarios: Vec<(ScenarioId, &str)> = vec![
        (ScenarioId::BlackThursday, "black_thursday"),
        (ScenarioId::SustainedBear, "sustained_bear"),
        (ScenarioId::FlashCrash, "flash_crash"),
        (ScenarioId::BankRun, "bank_run"),
        (ScenarioId::DemandShock, "demand_shock"),
    ];

    let report_dir = PathBuf::from("reports/oracle_comparison");
    let _ = std::fs::create_dir_all(&report_dir);

    let mut rows: Vec<ComparisonRow> = Vec::new();
    let mut entries = Vec::new();
    let target = 50.0;

    println!("\n  Running oracle comparison test suite ({} blocks, seed={})...", BLOCKS, SEED);
    println!("  Config: $5M AMM, 200% CR, Tick controller, 240-block TWAP");
    println!("  25 vaults at 210-280% CR (~8500 ZEC collateral, ~$195K debt)");
    println!("  Oracle-free: TWAP-based liquidation (default)");
    println!("  Oracle-based: external price liquidation + AMM sell (death spiral mode)\n");

    for (sid, name) in &test_scenarios {
        let prices = generate_prices(*sid, BLOCKS, SEED);

        // ---- Oracle-free mode ----
        let of_config = config_5m_200cr();
        let mut of_scenario = Scenario::new_with_seed(&of_config, SEED);
        add_heavy_vault_agents(&mut of_scenario);
        of_scenario.run(&prices);

        let of_verdict = report::evaluate_pass_fail(&of_scenario.metrics, target);
        let of_summary = output::compute_summary(&of_scenario.metrics, target);

        let of_html = report::generate_report(
            &of_scenario.metrics,
            &of_config,
            &format!("{}_oracle_free", name),
            target,
        );
        let of_path = report_dir.join(format!("{}_oracle_free.html", name));
        report::save_report(&of_html, &of_path).expect("save oracle-free report");

        // ---- Oracle-based mode ----
        let ob_config = config_oracle_based();
        let mut ob_scenario = Scenario::new_with_seed(&ob_config, SEED);
        add_heavy_vault_agents(&mut ob_scenario);
        ob_scenario.run(&prices);

        let ob_verdict = report::evaluate_pass_fail(&ob_scenario.metrics, target);
        let ob_summary = output::compute_summary(&ob_scenario.metrics, target);

        let ob_html = report::generate_report(
            &ob_scenario.metrics,
            &ob_config,
            &format!("{}_oracle_based", name),
            target,
        );
        let ob_path = report_dir.join(format!("{}_oracle_based.html", name));
        report::save_report(&ob_html, &ob_path).expect("save oracle-based report");

        rows.push(ComparisonRow {
            name: name.to_string(),
            of_verdict: of_verdict.overall.label().to_string(),
            of_mean_peg: of_summary.mean_peg_deviation,
            of_liqs: of_summary.total_liquidations,
            of_bad_debt: of_summary.total_bad_debt,
            ob_verdict: ob_verdict.overall.label().to_string(),
            ob_mean_peg: ob_summary.mean_peg_deviation,
            ob_liqs: ob_summary.total_liquidations,
            ob_bad_debt: ob_summary.total_bad_debt,
        });

        entries.push((format!("{}_oracle_free", name), of_verdict, of_summary));
        entries.push((format!("{}_oracle_based", name), ob_verdict, ob_summary));
    }

    // Generate master index
    let master_html = report::generate_master_summary(&entries);
    let master_path = report_dir.join("index.html");
    report::save_report(&master_html, &master_path).expect("save master summary");

    // Print comparison table
    println!("\n═══════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════");
    println!("  ZAI SIMULATOR — ORACLE COMPARISON: Oracle-Free (TWAP) vs Oracle-Based (External Price)");
    println!("═══════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════");
    println!(
        "  {:<20} │ {:>10} {:>8} {:>6} {:>12} │ {:>10} {:>8} {:>6} {:>12} │",
        "Scenario",
        "OF Verdict", "OF Peg", "OF Liq", "OF BadDebt",
        "OB Verdict", "OB Peg", "OB Liq", "OB BadDebt",
    );
    println!("  {}", "─".repeat(115));

    let mut of_better = 0u32;
    let mut ob_better = 0u32;

    for r in &rows {
        let of_wins = r.of_bad_debt <= r.ob_bad_debt && r.of_liqs <= r.ob_liqs;
        if of_wins {
            of_better += 1;
        } else {
            ob_better += 1;
        }

        println!(
            "  {:<20} │ {:>10} {:>7.2}% {:>6} {:>12.2} │ {:>10} {:>7.2}% {:>6} {:>12.2} │ {}",
            r.name,
            r.of_verdict,
            r.of_mean_peg * 100.0,
            r.of_liqs,
            r.of_bad_debt,
            r.ob_verdict,
            r.ob_mean_peg * 100.0,
            r.ob_liqs,
            r.ob_bad_debt,
            if of_wins {
                "<-- oracle-free wins"
            } else {
                "<-- oracle-based wins"
            },
        );
    }

    println!("  {}", "─".repeat(115));
    println!(
        "  SCORE: Oracle-free wins {}/{} scenarios, Oracle-based wins {}/{}",
        of_better,
        rows.len(),
        ob_better,
        rows.len()
    );
    println!("═══════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════");
    println!("\n  Reports saved to: reports/oracle_comparison/");
    println!("  Master summary:   reports/oracle_comparison/index.html\n");

    // Verify all reports exist
    for (_, name) in &test_scenarios {
        let of_path = report_dir.join(format!("{}_oracle_free.html", name));
        let ob_path = report_dir.join(format!("{}_oracle_based.html", name));
        assert!(
            of_path.exists(),
            "Oracle-free report should exist for {}",
            name
        );
        assert!(
            ob_path.exists(),
            "Oracle-based report should exist for {}",
            name
        );
    }
    assert!(master_path.exists(), "Master summary should exist");
}
