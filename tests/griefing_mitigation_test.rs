//! F-046: Griefing Mitigation Test
//!
//! F-043 found the sustained manipulation attack (100K ZEC, 1K/block × 100 blocks)
//! creates $3,145 bad debt with a 5.4:1 griefing ratio. This test finds configurations
//! that make griefing prohibitively expensive (>100:1) or eliminate bad debt entirely.
//!
//! 8 defensive configurations tested against the same sustained attack.

use std::path::PathBuf;

use zai_sim::controller::ControllerConfig;
use zai_sim::output;
use zai_sim::report;
use zai_sim::scenario::{Scenario, ScenarioConfig};
use zai_sim::scenarios::{add_agents, ScenarioId};

const BLOCKS: usize = 1000;
const SEED: u64 = 42;
const TARGET_PRICE: f64 = 50.0;
const WARMUP: u64 = 240;
const NUM_VAULTS: usize = 25;
const VAULT_DEBT: f64 = 1000.0;

// ═══════════════════════════════════════════════════════════════════════
// Configurations
// ═══════════════════════════════════════════════════════════════════════

struct GriefConfig {
    name: &'static str,
    amm_zec: f64,
    amm_zai: f64,
    min_ratio: f64,
    twap_window: u64,
    label: &'static str,
}

fn configs() -> Vec<GriefConfig> {
    vec![
        GriefConfig {
            name: "baseline",
            amm_zec: 100_000.0,
            amm_zai: 5_000_000.0,
            min_ratio: 2.0,
            twap_window: 240,
            label: "$5M / 200% / 5h TWAP",
        },
        GriefConfig {
            name: "cr_250",
            amm_zec: 100_000.0,
            amm_zai: 5_000_000.0,
            min_ratio: 2.5,
            twap_window: 240,
            label: "$5M / 250% / 5h TWAP",
        },
        GriefConfig {
            name: "cr_300",
            amm_zec: 100_000.0,
            amm_zai: 5_000_000.0,
            min_ratio: 3.0,
            twap_window: 240,
            label: "$5M / 300% / 5h TWAP",
        },
        GriefConfig {
            name: "deep_pool",
            amm_zec: 200_000.0,
            amm_zai: 10_000_000.0,
            min_ratio: 2.0,
            twap_window: 240,
            label: "$10M / 200% / 5h TWAP",
        },
        GriefConfig {
            name: "deep_cr_250",
            amm_zec: 200_000.0,
            amm_zai: 10_000_000.0,
            min_ratio: 2.5,
            twap_window: 240,
            label: "$10M / 250% / 5h TWAP",
        },
        GriefConfig {
            name: "deep_cr_300",
            amm_zec: 200_000.0,
            amm_zai: 10_000_000.0,
            min_ratio: 3.0,
            twap_window: 240,
            label: "$10M / 300% / 5h TWAP",
        },
        GriefConfig {
            name: "short_twap",
            amm_zec: 100_000.0,
            amm_zai: 5_000_000.0,
            min_ratio: 2.0,
            twap_window: 48,
            label: "$5M / 200% / 1h TWAP",
        },
        GriefConfig {
            name: "short_cr_250",
            amm_zec: 100_000.0,
            amm_zai: 5_000_000.0,
            min_ratio: 2.5,
            twap_window: 48,
            label: "$5M / 250% / 1h TWAP",
        },
    ]
}

fn make_config(gc: &GriefConfig) -> ScenarioConfig {
    let mut config = ScenarioConfig::default();
    config.amm_initial_zec = gc.amm_zec;
    config.amm_initial_zai = gc.amm_zai;
    config.cdp_config.min_ratio = gc.min_ratio;
    config.cdp_config.twap_window = gc.twap_window;
    config.controller_config = ControllerConfig::default_tick();
    config.liquidation_config.max_liquidations_per_block = 50;
    config
}

// ═══════════════════════════════════════════════════════════════════════
// Whale (reused from economic_attack_test.rs pattern)
// ═══════════════════════════════════════════════════════════════════════

struct Whale {
    zec: f64,
    zai: f64,
}

impl Whale {
    fn new(zec: f64, zai: f64) -> Self {
        Self { zec, zai }
    }

    fn value_usd(&self, ext_price: f64) -> f64 {
        self.zec * ext_price + self.zai * (ext_price / TARGET_PRICE)
    }

    fn sell_zec(&mut self, amount: f64, amm: &mut zai_sim::amm::Amm, block: u64) -> f64 {
        let to_sell = amount.min(self.zec);
        if to_sell <= 0.0 {
            return 0.0;
        }
        match amm.swap_zec_for_zai(to_sell, block) {
            Ok(zai_out) => {
                self.zec -= to_sell;
                self.zai += zai_out;
                zai_out
            }
            Err(_) => 0.0,
        }
    }

    fn buy_zec(&mut self, zai_amount: f64, amm: &mut zai_sim::amm::Amm, block: u64) -> f64 {
        let to_spend = zai_amount.min(self.zai);
        if to_spend <= 0.0 {
            return 0.0;
        }
        match amm.swap_zai_for_zec(to_spend, block) {
            Ok(zec_out) => {
                self.zai -= to_spend;
                self.zec += zec_out;
                zec_out
            }
            Err(_) => 0.0,
        }
    }

    fn exit_all_zai(&mut self, amm: &mut zai_sim::amm::Amm, block: u64) {
        if self.zai > 0.0 {
            self.buy_zec(self.zai, amm, block);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Result tracking
// ═══════════════════════════════════════════════════════════════════════

struct GriefRow {
    config_name: String,
    #[allow(dead_code)]
    label: String,
    whale_pnl: f64,
    whale_pnl_pct: f64,
    total_liqs: u32,
    bad_debt: f64,
    griefing_ratio: f64,
    min_twap: f64,
    verdict: String,
    lowest_liq_cr: f64,
    highest_safe_cr: f64,
}

// ═══════════════════════════════════════════════════════════════════════
// Setup and run
// ═══════════════════════════════════════════════════════════════════════

fn setup_scenario(config: &ScenarioConfig) -> Scenario {
    let mut scenario = Scenario::new_with_seed(config, SEED);
    add_agents(ScenarioId::SteadyState, &mut scenario);

    // Initialize LP agents
    for lp in &mut scenario.lp_agents {
        lp.provide_liquidity(&mut scenario.amm);
    }
    for lp in &mut scenario.il_aware_lps {
        lp.provide_liquidity(&mut scenario.amm);
    }

    // Open 25 vaults at CR spread from min_ratio+0.10 to min_ratio+0.80
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

    scenario
}

fn run_griefing(gc: &GriefConfig) -> (GriefRow, Scenario) {
    let config = make_config(gc);
    let mut scenario = setup_scenario(&config);
    let prices = vec![TARGET_PRICE; BLOCKS];

    let mut whale = Whale::new(100_000.0, 0.0);
    let start_value = whale.value_usd(TARGET_PRICE);

    let mut min_twap = f64::MAX;

    for (i, &ext_price) in prices.iter().enumerate() {
        let block = i as u64 + 1;

        // Sustained attack: dump 1K ZEC/block for blocks 241-340
        if block >= WARMUP + 1 && block <= WARMUP + 100 {
            whale.sell_zec(1_000.0, &mut scenario.amm, block);
        }
        // Buyback: spread over blocks 341-350
        else if block >= WARMUP + 101 && block <= WARMUP + 110 {
            let remaining = (WARMUP + 111 - block) as f64;
            whale.buy_zec(whale.zai / remaining, &mut scenario.amm, block);
        }

        scenario.step(block, ext_price);

        // Track min TWAP after warmup
        if block > WARMUP {
            let twap = scenario.amm.get_twap(config.cdp_config.twap_window as u64);
            min_twap = min_twap.min(twap);
        }
    }

    // Final exit
    whale.exit_all_zai(&mut scenario.amm, BLOCKS as u64 + 1);

    let end_value = whale.value_usd(TARGET_PRICE);
    let pnl = end_value - start_value;
    let pnl_pct = if start_value > 0.0 {
        pnl / start_value * 100.0
    } else {
        0.0
    };

    let total_liqs: u32 = scenario.metrics.iter().map(|m| m.liquidation_count).sum();
    let bad_debt = scenario.metrics.last().map(|m| m.bad_debt).unwrap_or(0.0);
    let verdict = report::evaluate_pass_fail(&scenario.metrics, TARGET_PRICE);

    let griefing_ratio = if bad_debt > 0.0 {
        pnl.abs() / bad_debt
    } else {
        f64::INFINITY
    };

    // Track which vault CRs got liquidated
    let base_cr = config.cdp_config.min_ratio + 0.10;
    let initial_crs: Vec<f64> = (0..NUM_VAULTS)
        .map(|i| base_cr + (i as f64) * 0.70 / (NUM_VAULTS - 1) as f64)
        .collect();

    let mut lowest_liq_cr = f64::MAX;
    let mut highest_safe_cr = 0.0_f64;

    for (i, &cr) in initial_crs.iter().enumerate() {
        let owner = format!("vault_{}", i);
        let exists = scenario
            .registry
            .vaults
            .values()
            .any(|v| v.owner == owner && v.debt_zai > 0.0);
        if exists {
            highest_safe_cr = highest_safe_cr.max(cr);
        } else {
            lowest_liq_cr = lowest_liq_cr.min(cr);
        }
    }

    let row = GriefRow {
        config_name: gc.name.to_string(),
        label: gc.label.to_string(),
        whale_pnl: pnl,
        whale_pnl_pct: pnl_pct,
        total_liqs,
        bad_debt,
        griefing_ratio,
        min_twap: if min_twap == f64::MAX { 0.0 } else { min_twap },
        verdict: verdict.overall.label().to_string(),
        lowest_liq_cr,
        highest_safe_cr,
    };

    (row, scenario)
}

// ═══════════════════════════════════════════════════════════════════════
// Test
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn griefing_mitigation() {
    println!(
        "\n{}",
        "═".repeat(160)
    );
    println!(
        "  F-046: GRIEFING MITIGATION TEST"
    );
    println!(
        "  Sustained manipulation: 100K ZEC whale, 1K/block × 100 blocks"
    );
    println!(
        "  8 defensive configurations | Which makes griefing prohibitively expensive?"
    );
    println!(
        "{}\n",
        "═".repeat(160)
    );

    let defs = configs();
    let mut all_rows: Vec<GriefRow> = Vec::new();
    let mut report_entries: Vec<(String, report::PassFailResult, output::SummaryMetrics)> =
        Vec::new();

    for gc in &defs {
        println!("  Running: {} — {}", gc.name, gc.label);

        let config = make_config(gc);
        let (row, scenario) = run_griefing(gc);
        let metrics = &scenario.metrics;

        let summary = output::compute_summary(metrics, TARGET_PRICE);
        let verdict = report::evaluate_pass_fail(metrics, TARGET_PRICE);

        let label = format!("grief_{}", gc.name);
        let html = report::generate_report(metrics, &config, &label, TARGET_PRICE);
        let html_path = PathBuf::from(format!("reports/griefing_mitigation/{}.html", label));
        report::save_report(&html, &html_path).expect("save report");

        report_entries.push((label, verdict, summary));

        let ratio_str = if row.griefing_ratio == f64::INFINITY {
            "∞ (no bad debt)".to_string()
        } else {
            format!("{:.1}:1", row.griefing_ratio)
        };
        println!(
            "    → {} | whale ${:.0} ({:.2}%) | liqs {} | bad_debt ${:.2} | ratio {} | min_twap ${:.2}",
            row.verdict,
            row.whale_pnl,
            row.whale_pnl_pct,
            row.total_liqs,
            row.bad_debt,
            ratio_str,
            row.min_twap,
        );

        all_rows.push(row);
    }

    // ─── Summary table ───
    println!(
        "\n{}",
        "═".repeat(160)
    );
    println!("  SUMMARY: Sustained Manipulation Attack × Defensive Configurations");
    println!(
        "{}",
        "═".repeat(160)
    );
    println!(
        "  {:<14} {:<26} {:<10} {:>11} {:>8} {:>5} {:>10} {:>12} {:>10} {:>9} {:>9}",
        "Config", "Parameters", "Verdict", "Whale P&L", "P&L %", "Liqs", "Bad Debt",
        "Grief Ratio", "Min TWAP", "Low LiqCR", "High SafeCR"
    );
    println!("  {}", "─".repeat(155));

    for (i, r) in all_rows.iter().enumerate() {
        let ratio_str = if r.griefing_ratio == f64::INFINITY {
            "∞".to_string()
        } else {
            format!("{:.1}:1", r.griefing_ratio)
        };
        let low_cr_str = if r.lowest_liq_cr == f64::MAX {
            "none".to_string()
        } else {
            format!("{:.0}%", r.lowest_liq_cr * 100.0)
        };
        let high_cr_str = if r.highest_safe_cr == 0.0 {
            "none".to_string()
        } else {
            format!("{:.0}%", r.highest_safe_cr * 100.0)
        };
        println!(
            "  {:<14} {:<26} {:<10} {:>11.0} {:>7.2}% {:>5} {:>10.2} {:>12} {:>10.2} {:>9} {:>9}",
            r.config_name,
            defs[i].label,
            r.verdict,
            r.whale_pnl,
            r.whale_pnl_pct,
            r.total_liqs,
            r.bad_debt,
            ratio_str,
            r.min_twap,
            low_cr_str,
            high_cr_str,
        );
    }
    println!(
        "{}",
        "═".repeat(160)
    );

    // ─── Analysis ───
    println!("\n  GRIEFING RESISTANCE ANALYSIS");
    println!("  {}", "─".repeat(80));

    // Zero bad debt configs
    let zero_bd: Vec<&str> = all_rows
        .iter()
        .filter(|r| r.bad_debt == 0.0)
        .map(|r| r.config_name.as_str())
        .collect();

    if zero_bd.is_empty() {
        println!("  Zero bad debt configs: NONE");
    } else {
        println!("  Zero bad debt configs: {}", zero_bd.join(", "));
    }

    // >100:1 ratio configs
    let high_ratio: Vec<(&str, f64)> = all_rows
        .iter()
        .filter(|r| r.griefing_ratio > 100.0)
        .map(|r| (r.config_name.as_str(), r.griefing_ratio))
        .collect();

    if high_ratio.is_empty() {
        println!("  Configs with >100:1 griefing ratio: NONE");
    } else {
        for (name, ratio) in &high_ratio {
            let ratio_str = if *ratio == f64::INFINITY {
                "∞".to_string()
            } else {
                format!("{:.1}:1", ratio)
            };
            println!("  >100:1 griefing ratio: {} ({})", name, ratio_str);
        }
    }

    // Cheapest defense (first config with zero bad debt, or highest ratio)
    let best = all_rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r.bad_debt == 0.0)
        .next()
        .or_else(|| {
            all_rows
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a.griefing_ratio.partial_cmp(&b.griefing_ratio).unwrap())
        });

    if let Some((idx, r)) = best {
        println!(
            "\n  RECOMMENDED FOR GRIEFING RESISTANCE: {} ({})",
            r.config_name, defs[idx].label
        );
        if r.bad_debt == 0.0 {
            println!(
                "    Zero bad debt, {} liquidations, whale loses ${:.0}",
                r.total_liqs,
                r.whale_pnl.abs()
            );
        } else {
            let ratio_str = format!("{:.1}:1", r.griefing_ratio);
            println!(
                "    Griefing ratio: {} — whale loses ${:.0} to create ${:.2} bad debt",
                ratio_str,
                r.whale_pnl.abs(),
                r.bad_debt
            );
        }
    }

    // ─── CR threshold analysis ───
    println!("\n  VAULT CR LIQUIDATION THRESHOLDS");
    println!("  {}", "─".repeat(80));
    println!(
        "  {:<14} {:>12} {:>12} {:>30}",
        "Config", "Lowest Liq", "Highest Safe", "Interpretation"
    );
    for r in &all_rows {
        let low_str = if r.lowest_liq_cr == f64::MAX {
            "none".to_string()
        } else {
            format!("{:.0}%", r.lowest_liq_cr * 100.0)
        };
        let high_str = if r.highest_safe_cr == 0.0 {
            "all liquidated".to_string()
        } else {
            format!("{:.0}%", r.highest_safe_cr * 100.0)
        };
        let interp = if r.lowest_liq_cr == f64::MAX {
            "No vaults liquidated"
        } else if r.highest_safe_cr == 0.0 {
            "ALL vaults liquidated"
        } else {
            "Partial liquidation"
        };
        println!(
            "  {:<14} {:>12} {:>12} {:>30}",
            r.config_name, low_str, high_str, interp
        );
    }

    println!(
        "\n{}",
        "═".repeat(160)
    );

    // ─── Generate master index HTML ───
    let master_html = report::generate_master_summary(&report_entries);
    let master_path = PathBuf::from("reports/griefing_mitigation/index.html");
    report::save_report(&master_html, &master_path).expect("save master index");
    println!(
        "  Reports saved to reports/griefing_mitigation/ (8 individual + index.html)\n"
    );

    // ─── Assertions ───
    assert_eq!(all_rows.len(), 8, "Should have 8 config runs");
    assert!(master_path.exists(), "Master index should exist");

    // Baseline should match F-043 (sustained attack produced bad debt)
    let baseline = &all_rows[0];
    assert_eq!(baseline.config_name, "baseline");
    assert!(
        baseline.total_liqs > 0,
        "Baseline should have liquidations"
    );
}
