//! F-044: Sustained Bear 50K-Block Survival Test (90% Decline)
//!
//! Tests whether ANY configuration can survive an extreme prolonged crash:
//! $50 → $5 (90% decline) over 50,000 blocks (~43 days).
//!
//! The standard sustained bear (F-024) used $50→$15 (70% decline) and SOFT FAILed
//! at 11.8% mean peg. This test pushes to 90% to find the design limit.
//!
//! 6 configurations tested: standard, deep_pool, high_cr, short_twap, combined, maximum.
//! 25 vaults opened per config to test solvency and liquidation behavior.

use std::path::PathBuf;

use zai_sim::controller::ControllerConfig;
use zai_sim::output;
use zai_sim::report;
use zai_sim::scenario::{Scenario, ScenarioConfig};
use zai_sim::scenarios::{add_agents, ScenarioId};

const BLOCKS: usize = 50_000;
const SEED: u64 = 42;
const TARGET_PRICE: f64 = 50.0;
const NUM_VAULTS: usize = 25;
const VAULT_DEBT: f64 = 1000.0;

// ═══════════════════════════════════════════════════════════════════════
// Price path: $50 → $5 linear decline over 50K blocks (90% decline)
// Per-block decline: $0.0009 (0.0018%)
// ═══════════════════════════════════════════════════════════════════════

fn bear_90pct_prices(blocks: usize) -> Vec<f64> {
    (0..blocks)
        .map(|i| {
            let t = i as f64 / (blocks - 1) as f64;
            50.0 - 45.0 * t // $50 → $5
        })
        .collect()
}

// ═══════════════════════════════════════════════════════════════════════
// 6 configurations
// ═══════════════════════════════════════════════════════════════════════

struct ConfigDef {
    name: &'static str,
    amm_zec: f64,
    amm_zai: f64,
    min_ratio: f64,
    twap_window: u64,
    description: &'static str,
}

fn configs() -> Vec<ConfigDef> {
    vec![
        ConfigDef {
            name: "standard",
            amm_zec: 100_000.0,
            amm_zai: 5_000_000.0,
            min_ratio: 2.0,
            twap_window: 240,
            description: "Current recommendation ($5M, 200% CR, 5h TWAP)",
        },
        ConfigDef {
            name: "deep_pool",
            amm_zec: 200_000.0,
            amm_zai: 10_000_000.0,
            min_ratio: 2.0,
            twap_window: 240,
            description: "Double liquidity ($10M, 200% CR, 5h TWAP)",
        },
        ConfigDef {
            name: "high_cr",
            amm_zec: 100_000.0,
            amm_zai: 5_000_000.0,
            min_ratio: 3.0,
            twap_window: 240,
            description: "Higher collateral buffer ($5M, 300% CR, 5h TWAP)",
        },
        ConfigDef {
            name: "short_twap",
            amm_zec: 100_000.0,
            amm_zai: 5_000_000.0,
            min_ratio: 2.0,
            twap_window: 48,
            description: "Faster TWAP response ($5M, 200% CR, 1h TWAP)",
        },
        ConfigDef {
            name: "combined",
            amm_zec: 200_000.0,
            amm_zai: 10_000_000.0,
            min_ratio: 3.0,
            twap_window: 48,
            description: "Best defensive combo ($10M, 300% CR, 1h TWAP)",
        },
        ConfigDef {
            name: "maximum",
            amm_zec: 400_000.0,
            amm_zai: 20_000_000.0,
            min_ratio: 3.0,
            twap_window: 48,
            description: "Kitchen sink defense ($20M, 300% CR, 1h TWAP)",
        },
    ]
}

fn make_config(def: &ConfigDef) -> ScenarioConfig {
    let mut config = ScenarioConfig::default();
    config.amm_initial_zec = def.amm_zec;
    config.amm_initial_zai = def.amm_zai;
    config.cdp_config.min_ratio = def.min_ratio;
    config.cdp_config.twap_window = def.twap_window;
    config.controller_config = ControllerConfig::default_tick();
    config.liquidation_config.max_liquidations_per_block = 50;
    config
}

// ═══════════════════════════════════════════════════════════════════════
// Result tracking
// ═══════════════════════════════════════════════════════════════════════

struct BearRow {
    config_name: String,
    #[allow(dead_code)]
    description: String,
    verdict: String,
    mean_peg: f64,
    max_peg: f64,
    total_liqs: u32,
    bad_debt: f64,
    final_zombie_count: u32,
    final_solvency_ratio: f64,
    arber_exhaust_block: Option<usize>,
    worst_block: usize,
    worst_peg: f64,
}

// ═══════════════════════════════════════════════════════════════════════
// Core run function
// ═══════════════════════════════════════════════════════════════════════

fn run_bear_config(def: &ConfigDef) -> (BearRow, Scenario) {
    let config = make_config(def);
    let prices = bear_90pct_prices(BLOCKS);

    let mut scenario = Scenario::new_with_seed(&config, SEED);
    add_agents(ScenarioId::SustainedBear, &mut scenario);

    // Open 25 vaults with CR spread from min_ratio+0.10 to min_ratio+0.80
    let base_cr = config.cdp_config.min_ratio + 0.10;
    for i in 0..NUM_VAULTS {
        let cr = base_cr + (i as f64) * 0.70 / (NUM_VAULTS - 1) as f64;
        let collateral = cr * VAULT_DEBT / TARGET_PRICE;
        let owner = format!("vault_{}", i);
        scenario
            .registry
            .open_vault(&owner, collateral, VAULT_DEBT, 0, &scenario.amm)
            .unwrap();
    }

    scenario.run(&prices);

    // Compute metrics
    let summary = output::compute_summary(&scenario.metrics, TARGET_PRICE);
    let verdict = report::evaluate_pass_fail(&scenario.metrics, TARGET_PRICE);

    let total_liqs: u32 = scenario.metrics.iter().map(|m| m.liquidation_count).sum();
    let bad_debt = scenario
        .metrics
        .last()
        .map(|m| m.bad_debt)
        .unwrap_or(0.0);
    let final_zombie_count = scenario
        .metrics
        .last()
        .map(|m| m.zombie_vault_count)
        .unwrap_or(0);

    // Solvency ratio: total_collateral * twap / total_debt
    let final_solvency = scenario.metrics.last().map(|m| {
        if m.total_debt > 0.0 {
            m.total_collateral * m.twap_price / m.total_debt
        } else {
            f64::INFINITY
        }
    }).unwrap_or(f64::INFINITY);

    // Arber exhaustion: first block where arber_zec_total < 1.0
    let arber_exhaust_block = scenario
        .metrics
        .iter()
        .position(|m| m.arber_zec_total < 1.0)
        .map(|i| i + 1);

    // Worst block: highest peg deviation
    let (worst_block, worst_peg) = scenario
        .metrics
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let dev = ((m.amm_spot_price - TARGET_PRICE) / TARGET_PRICE).abs();
            (i + 1, dev)
        })
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
        .unwrap_or((0, 0.0));

    let row = BearRow {
        config_name: def.name.to_string(),
        description: def.description.to_string(),
        verdict: verdict.overall.label().to_string(),
        mean_peg: summary.mean_peg_deviation,
        max_peg: summary.max_peg_deviation,
        total_liqs,
        bad_debt,
        final_zombie_count,
        final_solvency_ratio: final_solvency,
        arber_exhaust_block,
        worst_block,
        worst_peg,
    };

    (row, scenario)
}

// ═══════════════════════════════════════════════════════════════════════
// Test
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn sustained_bear_50k_survival() {
    println!(
        "\n{}", "═".repeat(130)
    );
    println!(
        "  F-044: SUSTAINED BEAR 50K-BLOCK SURVIVAL TEST — 90% DECLINE ($50 → $5)"
    );
    println!(
        "  50,000 blocks (~43 days) | 6 configurations | 25 vaults per config"
    );
    println!(
        "{}\n", "═".repeat(130)
    );

    let defs = configs();
    let mut all_rows: Vec<BearRow> = Vec::new();
    let mut report_entries: Vec<(String, report::PassFailResult, output::SummaryMetrics)> =
        Vec::new();

    for def in &defs {
        println!("  Running: {} — {}", def.name, def.description);

        let config = make_config(def);
        let (row, scenario) = run_bear_config(def);
        let metrics = &scenario.metrics;

        let summary = output::compute_summary(metrics, TARGET_PRICE);
        let verdict = report::evaluate_pass_fail(metrics, TARGET_PRICE);

        // Generate individual HTML report
        let label = format!("bear90_{}", def.name);
        let html = report::generate_report(metrics, &config, &label, TARGET_PRICE);
        let html_path = PathBuf::from(format!("reports/sustained_bear_50k/{}.html", label));
        report::save_report(&html, &html_path).expect("save individual report");

        report_entries.push((label, verdict, summary));

        println!(
            "    → {} | mean {:.2}% | max {:.2}% | liqs {} | bad_debt ${:.2}",
            row.verdict,
            row.mean_peg * 100.0,
            row.max_peg * 100.0,
            row.total_liqs,
            row.bad_debt,
        );

        all_rows.push(row);
    }

    // ─── Console summary table ───
    println!("\n{}", "═".repeat(160));
    println!("  SUMMARY: 90% DECLINE SURVIVAL ($50 → $5 over 50K blocks)");
    println!("{}", "═".repeat(160));
    println!(
        "  {:<12} {:<10} {:>9} {:>9} {:>6} {:>10} {:>8} {:>10} {:>12} {:>12} {:>10}",
        "Config", "Verdict", "Mean Peg", "Max Peg", "Liqs", "Bad Debt",
        "Zombies", "Solvency", "Exhaust Blk", "Worst Blk", "Worst Peg"
    );
    println!("  {}", "─".repeat(155));
    for r in &all_rows {
        let exhaust_str = r
            .arber_exhaust_block
            .map(|b| format!("{}", b))
            .unwrap_or_else(|| "never".to_string());
        let solvency_str = if r.final_solvency_ratio == f64::INFINITY {
            "∞".to_string()
        } else {
            format!("{:.2}x", r.final_solvency_ratio)
        };
        println!(
            "  {:<12} {:<10} {:>8.2}% {:>8.2}% {:>6} {:>10.2} {:>8} {:>10} {:>12} {:>12} {:>9.2}%",
            r.config_name,
            r.verdict,
            r.mean_peg * 100.0,
            r.max_peg * 100.0,
            r.total_liqs,
            r.bad_debt,
            r.final_zombie_count,
            solvency_str,
            exhaust_str,
            r.worst_block,
            r.worst_peg * 100.0,
        );
    }
    println!("{}", "═".repeat(160));

    // ─── Per-config analysis ───
    println!("\n  ANALYSIS");
    println!("  {}", "─".repeat(80));
    for r in &all_rows {
        let exhaust_info = match r.arber_exhaust_block {
            Some(b) => {
                let price_at_exhaust = 50.0 - 45.0 * (b as f64 - 1.0) / (BLOCKS - 1) as f64;
                format!(
                    "Arber exhausted at block {} (ext ${:.2})",
                    b, price_at_exhaust
                )
            }
            None => "Arber never exhausted".to_string(),
        };

        let solvency_note = if r.final_solvency_ratio == f64::INFINITY {
            "No remaining debt — fully deleveraged or no vaults"
        } else if r.final_solvency_ratio >= 1.0 {
            "Solvent (collateral*TWAP covers debt)"
        } else {
            "INSOLVENT (collateral*TWAP < debt)"
        };

        println!("  {} [{}]", r.config_name, r.verdict);
        println!("    {}", exhaust_info);
        println!(
            "    Solvency: {:.2}x — {}",
            r.final_solvency_ratio, solvency_note
        );
        if r.total_liqs > 0 {
            println!(
                "    Liquidations: {} | Bad debt: ${:.2}",
                r.total_liqs, r.bad_debt
            );
        } else {
            println!("    No liquidations triggered (AMM price stayed above thresholds)");
        }
        println!();
    }

    // ─── Key questions ───
    println!("  KEY QUESTIONS");
    println!("  {}", "─".repeat(80));

    let any_pass = all_rows.iter().any(|r| r.verdict == "PASS");
    let any_hard_fail = all_rows.iter().any(|r| r.verdict == "HARD FAIL");
    let best = all_rows
        .iter()
        .min_by(|a, b| a.mean_peg.partial_cmp(&b.mean_peg).unwrap())
        .unwrap();

    if any_pass {
        println!("  Q: Can any config survive 90% decline?");
        println!("  A: YES — at least one configuration PASSes.");
    } else if any_hard_fail {
        println!("  Q: Can any config survive 90% decline?");
        println!(
            "  A: NO — no configuration PASSes. Best: {} at {:.2}% mean peg.",
            best.config_name,
            best.mean_peg * 100.0
        );
    } else {
        println!("  Q: Can any config survive 90% decline?");
        println!(
            "  A: PARTIAL — all configs SOFT FAIL. Best: {} at {:.2}% mean peg.",
            best.config_name,
            best.mean_peg * 100.0
        );
    }

    let any_bad_debt = all_rows.iter().any(|r| r.bad_debt > 0.0);
    let any_insolvent = all_rows.iter().any(|r| r.final_solvency_ratio < 1.0);

    println!(
        "  Q: Does any config produce bad debt? A: {}",
        if any_bad_debt { "YES" } else { "NO" }
    );
    println!(
        "  Q: Does any config become insolvent? A: {}",
        if any_insolvent { "YES" } else { "NO" }
    );

    println!();

    // ─── Generate master index HTML ───
    let master_html = report::generate_master_summary(&report_entries);
    let master_path = PathBuf::from("reports/sustained_bear_50k/index.html");
    report::save_report(&master_html, &master_path).expect("save master index");
    println!(
        "  Reports saved to reports/sustained_bear_50k/ (6 individual + index.html)"
    );

    println!(
        "\n{}\n", "═".repeat(130)
    );

    // ─── Assertions ───
    // All 6 runs should produce metrics
    assert_eq!(all_rows.len(), 6, "Should have 6 config runs");

    // Reports should exist
    assert!(master_path.exists(), "Master index should exist");

    // Verify prices are correct
    let prices = bear_90pct_prices(BLOCKS);
    assert!((prices[0] - 50.0).abs() < 0.001, "Start price should be $50");
    assert!((prices[BLOCKS - 1] - 5.0).abs() < 0.001, "End price should be $5");
}
