//! F-042: LP Withdrawal Stress Test
//!
//! Tests what happens when LPs withdraw liquidity during a crash.
//! The $5M AMM assumption assumes LPs stay put — in reality, LPs withdraw
//! to avoid impermanent loss. This determines the withdrawal threshold
//! at which bad debt appears.
//!
//! 5 withdrawal patterns × 2 scenarios = 10 runs.

use std::path::PathBuf;

use zai_sim::controller::ControllerConfig;
use zai_sim::output;
use zai_sim::report;
use zai_sim::scenario::{Scenario, ScenarioConfig};
use zai_sim::scenarios::{add_agents, generate_prices, ScenarioId};

const BLOCKS: usize = 1000;
const SEED: u64 = 42;
const TARGET_PRICE: f64 = 50.0;

// ═══════════════════════════════════════════════════════════════════════
// Withdrawal patterns
// ═══════════════════════════════════════════════════════════════════════

enum WithdrawalPattern {
    /// Baseline — LPs stay put
    None,
    /// 2% of current shares per block from `start`, capped at `max_frac` of initial
    Gradual {
        start: u64,
        rate: f64,
        max_frac: f64,
    },
    /// Instant removal of `frac` of initial shares at `block`
    Instant { block: u64, frac: f64 },
}

struct PatternDef {
    name: &'static str,
    pattern: WithdrawalPattern,
}

fn patterns() -> Vec<PatternDef> {
    vec![
        PatternDef {
            name: "none",
            pattern: WithdrawalPattern::None,
        },
        PatternDef {
            name: "gradual",
            pattern: WithdrawalPattern::Gradual {
                start: 250,
                rate: 0.02,
                max_frac: 0.50,
            },
        },
        PatternDef {
            name: "panic",
            pattern: WithdrawalPattern::Instant {
                block: 250,
                frac: 0.50,
            },
        },
        PatternDef {
            name: "late",
            pattern: WithdrawalPattern::Instant {
                block: 500,
                frac: 0.50,
            },
        },
        PatternDef {
            name: "near_total",
            pattern: WithdrawalPattern::Instant {
                block: 250,
                frac: 0.90,
            },
        },
    ]
}

// ═══════════════════════════════════════════════════════════════════════
// Config
// ═══════════════════════════════════════════════════════════════════════

fn default_config() -> ScenarioConfig {
    let mut c = ScenarioConfig::default();
    c.amm_initial_zec = 100_000.0;
    c.amm_initial_zai = 5_000_000.0;
    c.cdp_config.min_ratio = 2.0;
    c.cdp_config.twap_window = 240;
    c.controller_config = ControllerConfig::default_tick();
    c.liquidation_config.max_liquidations_per_block = 50;
    c
}

// ═══════════════════════════════════════════════════════════════════════
// Manual run with LP withdrawals injected
// ═══════════════════════════════════════════════════════════════════════

struct RunResult {
    scenario: Scenario,
    min_reserve_zai: f64,
}

fn run_with_withdrawals(
    config: &ScenarioConfig,
    sid: ScenarioId,
    prices: &[f64],
    pattern: &WithdrawalPattern,
) -> RunResult {
    let mut scenario = Scenario::new_with_seed(config, SEED);
    add_agents(sid, &mut scenario);

    // Replicate run() initialization
    for lp in &mut scenario.lp_agents {
        lp.provide_liquidity(&mut scenario.amm);
    }
    for lp in &mut scenario.il_aware_lps {
        lp.provide_liquidity(&mut scenario.amm);
    }
    for holder in &mut scenario.cdp_holders {
        let _ = holder.open_vault(&mut scenario.registry, &scenario.amm, 0);
    }

    let initial_genesis_shares = scenario
        .amm
        .lp_shares
        .get("genesis")
        .copied()
        .unwrap_or(0.0);
    let mut total_removed_frac: f64 = 0.0;
    let mut min_reserve_zai = scenario.amm.reserve_zai;

    for (i, &price) in prices.iter().enumerate() {
        let block = i as u64 + 1;

        // Inject withdrawal before step
        match pattern {
            WithdrawalPattern::None => {}
            WithdrawalPattern::Gradual {
                start,
                rate,
                max_frac,
            } => {
                if block >= *start && total_removed_frac < *max_frac {
                    let current_shares = scenario
                        .amm
                        .lp_shares
                        .get("genesis")
                        .copied()
                        .unwrap_or(0.0);
                    let to_remove = (current_shares * rate)
                        .min(initial_genesis_shares * (max_frac - total_removed_frac));
                    if to_remove > 0.0 {
                        let _ = scenario.amm.remove_liquidity(to_remove, "genesis");
                        total_removed_frac += to_remove / initial_genesis_shares;
                    }
                }
            }
            WithdrawalPattern::Instant { block: wb, frac } => {
                if block == *wb {
                    let shares = initial_genesis_shares * frac;
                    let _ = scenario.amm.remove_liquidity(shares, "genesis");
                    total_removed_frac = *frac;
                }
            }
        }

        scenario.step(block, price);
        min_reserve_zai = min_reserve_zai.min(scenario.amm.reserve_zai);
    }

    RunResult {
        scenario,
        min_reserve_zai,
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Result row
// ═══════════════════════════════════════════════════════════════════════

struct LpRow {
    pattern_name: String,
    scenario_name: String,
    verdict: String,
    mean_peg: f64,
    max_peg: f64,
    liqs: u32,
    bad_debt: f64,
    max_zombie_count: u32,
    final_amm_zec: f64,
    final_amm_zai: f64,
    min_amm_depth: f64,
}

// ═══════════════════════════════════════════════════════════════════════
// Test
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn lp_withdrawal_stress() {
    let config = default_config();
    let target = TARGET_PRICE;

    let scenario_ids = [
        (ScenarioId::BlackThursday, "black_thursday"),
        (ScenarioId::SustainedBear, "sustained_bear"),
    ];

    let mut all_rows: Vec<LpRow> = Vec::new();
    let mut report_entries: Vec<(String, report::PassFailResult, output::SummaryMetrics)> =
        Vec::new();

    for (sid, sname) in &scenario_ids {
        let prices = generate_prices(*sid, BLOCKS, SEED);

        for pdef in patterns() {
            let label = format!("{}__{}", sname, pdef.name);
            println!("\n--- Running: {} ---", label);

            let result = run_with_withdrawals(&config, *sid, &prices, &pdef.pattern);
            let metrics = &result.scenario.metrics;

            let summary = output::compute_summary(metrics, target);
            let verdict = report::evaluate_pass_fail(metrics, target);

            let max_zombie = metrics.iter().map(|m| m.zombie_vault_count).max().unwrap_or(0);
            let last = metrics.last().unwrap();

            let row = LpRow {
                pattern_name: pdef.name.to_string(),
                scenario_name: sname.to_string(),
                verdict: verdict.overall.label().to_string(),
                mean_peg: summary.mean_peg_deviation * 100.0,
                max_peg: summary.max_peg_deviation * 100.0,
                liqs: summary.total_liquidations,
                bad_debt: summary.total_bad_debt,
                max_zombie_count: max_zombie,
                final_amm_zec: last.amm_reserve_zec,
                final_amm_zai: last.amm_reserve_zai,
                min_amm_depth: result.min_reserve_zai,
            };

            // Generate individual HTML report
            let html = report::generate_report(metrics, &config, &label, target);
            let html_path = PathBuf::from(format!("reports/lp_withdrawal/{}.html", label));
            report::save_report(&html, &html_path).expect("save individual report");

            report_entries.push((label.clone(), verdict.clone(), summary));
            all_rows.push(row);
        }
    }

    // ─── Console summary table ───
    println!("\n{}", "=".repeat(140));
    println!("  F-042: LP WITHDRAWAL STRESS TEST — SUMMARY");
    println!("{}", "=".repeat(140));
    println!(
        "{:<14} {:<16} {:<10} {:>9} {:>9} {:>6} {:>10} {:>8} {:>12} {:>12} {:>12}",
        "Pattern", "Scenario", "Verdict", "Mean Peg", "Max Peg", "Liqs", "Bad Debt", "Zombies",
        "Final ZEC", "Final ZAI", "Min Depth"
    );
    println!("{}", "-".repeat(140));
    for r in &all_rows {
        println!(
            "{:<14} {:<16} {:<10} {:>8.2}% {:>8.2}% {:>6} {:>10.2} {:>8} {:>12.0} {:>12.0} {:>12.0}",
            r.pattern_name,
            r.scenario_name,
            r.verdict,
            r.mean_peg,
            r.max_peg,
            r.liqs,
            r.bad_debt,
            r.max_zombie_count,
            r.final_amm_zec,
            r.final_amm_zai,
            r.min_amm_depth,
        );
    }
    println!("{}", "=".repeat(140));

    // ─── Per-scenario cross-table: withdrawal pattern vs bad debt ───
    for (sid, sname) in &scenario_ids {
        println!("\n  {} — Bad Debt by Withdrawal Pattern", sname);
        println!("  {}", "-".repeat(50));
        let rows: Vec<&LpRow> = all_rows
            .iter()
            .filter(|r| r.scenario_name == *sname)
            .collect();
        for r in &rows {
            println!(
                "  {:<14}  bad_debt={:>10.2}  verdict={}",
                r.pattern_name, r.bad_debt, r.verdict
            );
        }
        let _ = sid; // suppress unused warning
    }

    // ─── Generate master index HTML ───
    let master_html = report::generate_master_summary(&report_entries);
    let master_path = PathBuf::from("reports/lp_withdrawal/index.html");
    report::save_report(&master_html, &master_path).expect("save master index");
    println!("\nReports saved to reports/lp_withdrawal/");

    // ─── Assertions ───
    // Baseline (no withdrawal) must pass for both scenarios
    for r in &all_rows {
        if r.pattern_name == "none" {
            assert_eq!(
                r.verdict, "PASS",
                "Baseline (no withdrawal) should PASS for {}",
                r.scenario_name
            );
            assert!(
                r.bad_debt == 0.0,
                "Baseline should have zero bad debt for {}",
                r.scenario_name
            );
        }
    }

    // All 10 runs must produce metrics
    assert_eq!(all_rows.len(), 10, "Should have 10 runs (5 patterns × 2 scenarios)");

    // Reports should exist
    assert!(master_path.exists(), "Master index should exist");
}
