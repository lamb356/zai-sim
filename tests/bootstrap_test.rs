//! F-045: Bootstrap Liquidity Path Test
//!
//! Can the system operate during a bootstrap phase with lower liquidity
//! and tighter parameters? What's the minimum safe liquidity at each phase?
//!
//! Phase 1: 6 bootstrap configs × 2 crash scenarios (BT, FC) = 12 static runs
//! Phase 2: Growing liquidity ($500K→$5M) with BT crash at 3 different points = 3 runs
//! Total: 15 runs

use std::path::PathBuf;

use zai_sim::controller::ControllerConfig;
use zai_sim::output;
use zai_sim::report;
use zai_sim::scenario::{Scenario, ScenarioConfig};
use zai_sim::scenarios::{add_agents, generate_prices, ScenarioId};

const BLOCKS: usize = 1000;
const SEED: u64 = 42;
const TARGET_PRICE: f64 = 50.0;
const NUM_VAULTS: usize = 10;
const VAULT_DEBT: f64 = 500.0;

// ═══════════════════════════════════════════════════════════════════════
// Bootstrap configurations
// ═══════════════════════════════════════════════════════════════════════

struct BootConfig {
    name: &'static str,
    amm_zec: f64,
    amm_zai: f64,
    min_ratio: f64,
    twap_window: u64,
    label: &'static str,
}

fn bootstrap_configs() -> Vec<BootConfig> {
    vec![
        BootConfig {
            name: "boot_100k",
            amm_zec: 2_000.0,
            amm_zai: 100_000.0,
            min_ratio: 3.0,
            twap_window: 48,
            label: "$100K / 300% CR / 1h TWAP",
        },
        BootConfig {
            name: "boot_250k",
            amm_zec: 5_000.0,
            amm_zai: 250_000.0,
            min_ratio: 3.0,
            twap_window: 48,
            label: "$250K / 300% CR / 1h TWAP",
        },
        BootConfig {
            name: "boot_500k",
            amm_zec: 10_000.0,
            amm_zai: 500_000.0,
            min_ratio: 2.5,
            twap_window: 120,
            label: "$500K / 250% CR / 2.5h TWAP",
        },
        BootConfig {
            name: "boot_1m",
            amm_zec: 20_000.0,
            amm_zai: 1_000_000.0,
            min_ratio: 2.0,
            twap_window: 240,
            label: "$1M / 200% CR / 5h TWAP",
        },
        BootConfig {
            name: "boot_2_5m",
            amm_zec: 50_000.0,
            amm_zai: 2_500_000.0,
            min_ratio: 2.0,
            twap_window: 240,
            label: "$2.5M / 200% CR / 5h TWAP",
        },
        BootConfig {
            name: "boot_5m",
            amm_zec: 100_000.0,
            amm_zai: 5_000_000.0,
            min_ratio: 2.0,
            twap_window: 240,
            label: "$5M / 200% CR / 5h TWAP",
        },
    ]
}

fn make_config(bc: &BootConfig) -> ScenarioConfig {
    let mut config = ScenarioConfig::default();
    config.amm_initial_zec = bc.amm_zec;
    config.amm_initial_zai = bc.amm_zai;
    config.cdp_config.min_ratio = bc.min_ratio;
    config.cdp_config.twap_window = bc.twap_window;
    config.controller_config = ControllerConfig::default_tick();
    config.liquidation_config.max_liquidations_per_block = 50;
    config
}

// ═══════════════════════════════════════════════════════════════════════
// Shifted Black Thursday for Phase 2
// ═══════════════════════════════════════════════════════════════════════

fn shifted_black_thursday(blocks: usize, crash_start: usize) -> Vec<f64> {
    let crash_dur = 100;
    let recovery_dur = 250;
    let crash_end = crash_start + crash_dur;
    let recovery_end = crash_end + recovery_dur;
    (0..blocks)
        .map(|i| {
            if i < crash_start {
                50.0
            } else if i < crash_end {
                let t = (i - crash_start) as f64 / crash_dur as f64;
                50.0 - 30.0 * t // $50 → $20
            } else if i < recovery_end {
                let t = (i - crash_end) as f64 / recovery_dur as f64;
                20.0 + 15.0 * t // $20 → $35
            } else {
                35.0
            }
        })
        .collect()
}

// ═══════════════════════════════════════════════════════════════════════
// Result tracking
// ═══════════════════════════════════════════════════════════════════════

struct BootstrapRow {
    config_name: String,
    #[allow(dead_code)]
    scenario_name: String,
    verdict: String,
    mean_peg: f64,
    max_peg: f64,
    total_liqs: u32,
    bad_debt: f64,
    min_amm_depth: f64,
    final_amm_depth: f64,
}

// ═══════════════════════════════════════════════════════════════════════
// Phase 1: Static config runs
// ═══════════════════════════════════════════════════════════════════════

fn run_static(bc: &BootConfig, scenario_id: ScenarioId, scenario_label: &str) -> (BootstrapRow, Scenario) {
    let config = make_config(bc);
    let prices = generate_prices(scenario_id, BLOCKS, SEED);

    let mut scenario = Scenario::new_with_seed(&config, SEED);
    add_agents(scenario_id, &mut scenario);

    // Open 10 vaults at CR spread from min_ratio+0.10 to min_ratio+0.50
    let base_cr = config.cdp_config.min_ratio + 0.10;
    for i in 0..NUM_VAULTS {
        let cr = base_cr + (i as f64) * 0.40 / (NUM_VAULTS - 1) as f64;
        let collateral = cr * VAULT_DEBT / TARGET_PRICE;
        let owner = format!("vault_{}", i);
        scenario
            .registry
            .open_vault(&owner, collateral, VAULT_DEBT, 0, &scenario.amm)
            .unwrap();
    }

    scenario.run(&prices);

    let summary = output::compute_summary(&scenario.metrics, TARGET_PRICE);
    let verdict = report::evaluate_pass_fail(&scenario.metrics, TARGET_PRICE);
    let total_liqs: u32 = scenario.metrics.iter().map(|m| m.liquidation_count).sum();
    let bad_debt = scenario.metrics.last().map(|m| m.bad_debt).unwrap_or(0.0);

    // Track min/final AMM depth (reserve_zec * ext_price + reserve_zai)
    let min_depth = scenario
        .metrics
        .iter()
        .map(|m| m.amm_reserve_zec * m.external_price + m.amm_reserve_zai)
        .fold(f64::MAX, f64::min);
    let final_depth = scenario
        .metrics
        .last()
        .map(|m| m.amm_reserve_zec * m.external_price + m.amm_reserve_zai)
        .unwrap_or(0.0);

    let row = BootstrapRow {
        config_name: bc.name.to_string(),
        scenario_name: scenario_label.to_string(),
        verdict: verdict.overall.label().to_string(),
        mean_peg: summary.mean_peg_deviation,
        max_peg: summary.max_peg_deviation,
        total_liqs,
        bad_debt,
        min_amm_depth: min_depth,
        final_amm_depth: final_depth,
    };

    (row, scenario)
}

// ═══════════════════════════════════════════════════════════════════════
// Phase 2: Growing liquidity with crash
// ═══════════════════════════════════════════════════════════════════════

fn run_growing(crash_start: usize, run_name: &str) -> (BootstrapRow, Scenario) {
    // Start at $500K config
    let mut config = ScenarioConfig::default();
    config.amm_initial_zec = 10_000.0;
    config.amm_initial_zai = 500_000.0;
    config.cdp_config.min_ratio = 2.5;
    config.cdp_config.twap_window = 120;
    config.controller_config = ControllerConfig::default_tick();
    config.liquidation_config.max_liquidations_per_block = 50;

    let prices = shifted_black_thursday(BLOCKS, crash_start);

    let mut scenario = Scenario::new_with_seed(&config, SEED);
    add_agents(ScenarioId::BlackThursday, &mut scenario);

    // Open 10 vaults at 260-300% CR
    for i in 0..NUM_VAULTS {
        let cr = 2.60 + (i as f64) * 0.40 / (NUM_VAULTS - 1) as f64;
        let collateral = cr * VAULT_DEBT / TARGET_PRICE;
        let owner = format!("vault_{}", i);
        scenario
            .registry
            .open_vault(&owner, collateral, VAULT_DEBT, 0, &scenario.amm)
            .unwrap();
    }

    // Initialize LP and CDP holder agents (normally done by scenario.run())
    for lp in &mut scenario.lp_agents {
        lp.provide_liquidity(&mut scenario.amm);
    }
    for lp in &mut scenario.il_aware_lps {
        lp.provide_liquidity(&mut scenario.amm);
    }

    let growth_zec_per_injection = 9_000.0; // (100K - 10K) / 10
    let mut min_depth = f64::MAX;

    // Manual step loop with liquidity injection
    for (i, &ext_price) in prices.iter().enumerate() {
        let block = i as u64 + 1;

        // Inject liquidity every 50 blocks during growth phase (blocks 50-500)
        if block <= 500 && block % 50 == 0 {
            let ratio = scenario.amm.reserve_zai / scenario.amm.reserve_zec;
            let growth_zai = growth_zec_per_injection * ratio;
            scenario
                .amm
                .add_liquidity(growth_zec_per_injection, growth_zai, "bootstrap_lp")
                .unwrap();
        }

        scenario.step(block, ext_price);

        // Track min AMM depth
        let depth =
            scenario.amm.reserve_zec * ext_price + scenario.amm.reserve_zai;
        min_depth = min_depth.min(depth);
    }

    let summary = output::compute_summary(&scenario.metrics, TARGET_PRICE);
    let verdict = report::evaluate_pass_fail(&scenario.metrics, TARGET_PRICE);
    let total_liqs: u32 = scenario.metrics.iter().map(|m| m.liquidation_count).sum();
    let bad_debt = scenario.metrics.last().map(|m| m.bad_debt).unwrap_or(0.0);
    let final_depth = scenario
        .metrics
        .last()
        .map(|m| m.amm_reserve_zec * m.external_price + m.amm_reserve_zai)
        .unwrap_or(0.0);

    let row = BootstrapRow {
        config_name: run_name.to_string(),
        scenario_name: format!("BT@blk{}", crash_start),
        verdict: verdict.overall.label().to_string(),
        mean_peg: summary.mean_peg_deviation,
        max_peg: summary.max_peg_deviation,
        total_liqs,
        bad_debt,
        min_amm_depth: min_depth,
        final_amm_depth: final_depth,
    };

    (row, scenario)
}

// ═══════════════════════════════════════════════════════════════════════
// Test
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn bootstrap_liquidity_path() {
    println!(
        "\n{}",
        "═".repeat(140)
    );
    println!(
        "  F-045: BOOTSTRAP LIQUIDITY PATH TEST"
    );
    println!(
        "  Phase 1: 6 configs × 2 scenarios | Phase 2: Growing $500K→$5M with shifted BT crash"
    );
    println!(
        "{}\n",
        "═".repeat(140)
    );

    let configs = bootstrap_configs();
    let mut all_rows: Vec<BootstrapRow> = Vec::new();
    let mut report_entries: Vec<(String, report::PassFailResult, output::SummaryMetrics)> =
        Vec::new();

    // ─── Phase 1: Static configs ───
    println!("  PHASE 1: Static Bootstrap Configurations");
    println!("  {}", "─".repeat(60));

    let scenarios: Vec<(ScenarioId, &str)> = vec![
        (ScenarioId::BlackThursday, "BT"),
        (ScenarioId::FlashCrash, "FC"),
    ];

    for bc in &configs {
        for &(scenario_id, scenario_label) in &scenarios {
            let (row, scenario) = run_static(bc, scenario_id, scenario_label);
            let metrics = &scenario.metrics;

            let summary = output::compute_summary(metrics, TARGET_PRICE);
            let verdict = report::evaluate_pass_fail(metrics, TARGET_PRICE);

            let label = format!("boot_{}_{}", bc.name, scenario_label.to_lowercase());
            let html = report::generate_report(metrics, &make_config(bc), &label, TARGET_PRICE);
            let html_path = PathBuf::from(format!("reports/bootstrap/{}.html", label));
            report::save_report(&html, &html_path).expect("save report");

            report_entries.push((label, verdict, summary));

            println!(
                "    {} × {}: {} | mean {:.2}% | max {:.2}% | liqs {} | bad_debt ${:.2} | min_depth ${:.0}",
                bc.name,
                scenario_label,
                row.verdict,
                row.mean_peg * 100.0,
                row.max_peg * 100.0,
                row.total_liqs,
                row.bad_debt,
                row.min_amm_depth,
            );

            all_rows.push(row);
        }
    }

    // ─── Phase 2: Growing liquidity ───
    println!("\n  PHASE 2: Growing Liquidity ($500K → $5M) with Shifted BT Crash");
    println!("  {}", "─".repeat(60));

    let growth_runs = vec![
        (100, "grow_crash_100"),
        (250, "grow_crash_250"),
        (400, "grow_crash_400"),
    ];

    for &(crash_start, name) in &growth_runs {
        let config_for_report = {
            let mut c = ScenarioConfig::default();
            c.amm_initial_zec = 10_000.0;
            c.amm_initial_zai = 500_000.0;
            c.cdp_config.min_ratio = 2.5;
            c.cdp_config.twap_window = 120;
            c.controller_config = ControllerConfig::default_tick();
            c.liquidation_config.max_liquidations_per_block = 50;
            c
        };

        let (row, scenario) = run_growing(crash_start, name);
        let metrics = &scenario.metrics;

        let summary = output::compute_summary(metrics, TARGET_PRICE);
        let verdict = report::evaluate_pass_fail(metrics, TARGET_PRICE);

        let label = format!("boot_{}", name);
        let html = report::generate_report(metrics, &config_for_report, &label, TARGET_PRICE);
        let html_path = PathBuf::from(format!("reports/bootstrap/{}.html", label));
        report::save_report(&html, &html_path).expect("save report");

        report_entries.push((label, verdict, summary));

        // Estimate AMM depth at crash start (injections done × $450K + $500K base)
        let injections_done = crash_start / 50;
        let est_depth_at_crash = 500_000.0 + injections_done as f64 * 450_000.0;

        println!(
            "    {} (crash@blk{}, ~${:.0}K depth): {} | mean {:.2}% | max {:.2}% | liqs {} | bad_debt ${:.2} | min_depth ${:.0}",
            name,
            crash_start,
            est_depth_at_crash / 1000.0,
            row.verdict,
            row.mean_peg * 100.0,
            row.max_peg * 100.0,
            row.total_liqs,
            row.bad_debt,
            row.min_amm_depth,
        );

        all_rows.push(row);
    }

    // ─── Phase 1 Summary Table (pivoted: BT vs FC side by side) ───
    println!(
        "\n{}",
        "═".repeat(140)
    );
    println!("  PHASE 1 SUMMARY: Bootstrap Config × Crash Scenario");
    println!(
        "{}",
        "═".repeat(140)
    );
    println!(
        "  {:<12} {:<30} │ {:<10} {:>8} {:>8} {:>5} {:>9} │ {:<10} {:>8} {:>8} {:>5} {:>9}",
        "Config", "Parameters",
        "BT Verdict", "Mean%", "Max%", "Liqs", "Bad Debt",
        "FC Verdict", "Mean%", "Max%", "Liqs", "Bad Debt",
    );
    println!("  {}", "─".repeat(135));

    for (idx, bc) in configs.iter().enumerate() {
        let bt = &all_rows[idx * 2];
        let fc = &all_rows[idx * 2 + 1];
        println!(
            "  {:<12} {:<30} │ {:<10} {:>7.2}% {:>7.2}% {:>5} {:>9.2} │ {:<10} {:>7.2}% {:>7.2}% {:>5} {:>9.2}",
            bc.name,
            bc.label,
            bt.verdict,
            bt.mean_peg * 100.0,
            bt.max_peg * 100.0,
            bt.total_liqs,
            bt.bad_debt,
            fc.verdict,
            fc.mean_peg * 100.0,
            fc.max_peg * 100.0,
            fc.total_liqs,
            fc.bad_debt,
        );
    }
    println!(
        "{}",
        "═".repeat(140)
    );

    // ─── Phase 2 Summary Table ───
    println!(
        "\n{}",
        "═".repeat(120)
    );
    println!("  PHASE 2 SUMMARY: Growing $500K → $5M with Shifted Black Thursday");
    println!(
        "{}",
        "═".repeat(120)
    );
    println!(
        "  {:<20} {:<14} {:<10} {:>8} {:>8} {:>5} {:>9} {:>14} {:>14}",
        "Run", "Crash At", "Verdict", "Mean%", "Max%", "Liqs", "Bad Debt", "Min Depth", "Final Depth"
    );
    println!("  {}", "─".repeat(115));

    let phase2_start = configs.len() * 2;
    for (i, &(crash_start, name)) in growth_runs.iter().enumerate() {
        let r = &all_rows[phase2_start + i];
        println!(
            "  {:<20} block {:<7} {:<10} {:>7.2}% {:>7.2}% {:>5} {:>9.2} {:>14.0} {:>14.0}",
            name,
            crash_start,
            r.verdict,
            r.mean_peg * 100.0,
            r.max_peg * 100.0,
            r.total_liqs,
            r.bad_debt,
            r.min_amm_depth,
            r.final_amm_depth,
        );
    }
    println!(
        "{}",
        "═".repeat(120)
    );

    // ─── Bootstrap Roadmap ───
    println!("\n  BOOTSTRAP ROADMAP (derived from results)");
    println!("  {}", "─".repeat(80));

    // Find first config that passes BT
    let first_bt_pass = configs.iter().enumerate().find(|(idx, _)| {
        all_rows[idx * 2].verdict == "PASS"
    });
    // Find first config that passes FC
    let first_fc_pass = configs.iter().enumerate().find(|(idx, _)| {
        all_rows[idx * 2 + 1].verdict == "PASS"
    });

    if let Some((_, bc)) = first_bt_pass {
        println!("  Min safe for Black Thursday:  {} ({})", bc.name, bc.label);
    } else {
        println!("  Min safe for Black Thursday:  NONE pass at tested configs");
    }
    if let Some((_, bc)) = first_fc_pass {
        println!("  Min safe for Flash Crash:     {} ({})", bc.name, bc.label);
    } else {
        println!("  Min safe for Flash Crash:     NONE pass at tested configs");
    }

    // Check Phase 2: can you grow through a crash?
    let grow_passes: Vec<&str> = growth_runs
        .iter()
        .enumerate()
        .filter(|(i, _)| all_rows[phase2_start + i].verdict == "PASS")
        .map(|(_, &(_, name))| name)
        .collect();

    if grow_passes.is_empty() {
        println!("  Growing through BT crash:     No growth runs PASS — delay CDPs until $5M");
    } else {
        println!(
            "  Growing through BT crash:     {} of 3 pass ({})",
            grow_passes.len(),
            grow_passes.join(", ")
        );
    }

    // Print suggested roadmap
    println!("\n  SUGGESTED DEPLOYMENT PHASES:");
    println!("  {}", "─".repeat(80));

    for (idx, bc) in configs.iter().enumerate() {
        let bt_v = &all_rows[idx * 2].verdict;
        let fc_v = &all_rows[idx * 2 + 1].verdict;
        let status = if bt_v == "PASS" && fc_v == "PASS" {
            "CDPs ENABLED — full operation"
        } else if fc_v == "PASS" {
            "CDPs with caution — BT vulnerable"
        } else if bt_v == "SOFT FAIL" || fc_v == "SOFT FAIL" {
            "AMM-only trading — no CDPs"
        } else {
            "AMM-only trading — high risk"
        };
        println!(
            "    {} → {} (BT:{} FC:{})",
            bc.label, status, bt_v, fc_v
        );
    }

    println!(
        "\n{}",
        "═".repeat(140)
    );

    // ─── Generate master index HTML ───
    let master_html = report::generate_master_summary(&report_entries);
    let master_path = PathBuf::from("reports/bootstrap/index.html");
    report::save_report(&master_html, &master_path).expect("save master index");
    println!(
        "  Reports saved to reports/bootstrap/ (15 individual + index.html)\n"
    );

    // ─── Assertions ───
    assert_eq!(all_rows.len(), 15, "Should have 15 total runs");
    assert!(master_path.exists(), "Master index should exist");

    // Production baseline should pass both scenarios
    let prod_bt = &all_rows[10]; // boot_5m × BT
    let prod_fc = &all_rows[11]; // boot_5m × FC
    assert_eq!(prod_bt.config_name, "boot_5m");
    assert_eq!(prod_fc.config_name, "boot_5m");
    assert_eq!(
        prod_bt.verdict, "PASS",
        "Production $5M should pass Black Thursday"
    );
    assert_eq!(
        prod_fc.verdict, "PASS",
        "Production $5M should pass Flash Crash"
    );

    // Zero bad debt for production baseline
    assert_eq!(prod_bt.bad_debt, 0.0, "$5M BT should have zero bad debt");
    assert_eq!(prod_fc.bad_debt, 0.0, "$5M FC should have zero bad debt");
}
