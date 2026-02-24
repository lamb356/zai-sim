//! F-043: Economic Attack Profitability Test
//!
//! Tests whether a whale can profit by manipulating the AMM to trigger
//! liquidations. Tracks attacker P&L across 4 attack strategies + baseline.
//! Expected result: every attack is unprofitable at $5M AMM depth due to
//! TWAP lag + arber healing.

use std::path::PathBuf;

use zai_sim::controller::ControllerConfig;
use zai_sim::output;
use zai_sim::report;
use zai_sim::scenario::{Scenario, ScenarioConfig};
use zai_sim::scenarios::{add_agents, generate_prices, ScenarioId};

const BLOCKS: usize = 1000;
const SEED: u64 = 42;
const TARGET_PRICE: f64 = 50.0;
const WARMUP: u64 = 240;

// ═══════════════════════════════════════════════════════════════════════
// Config and setup
// ═══════════════════════════════════════════════════════════════════════

fn attack_config() -> ScenarioConfig {
    let mut c = ScenarioConfig::default();
    c.amm_initial_zec = 100_000.0;
    c.amm_initial_zai = 5_000_000.0;
    c.cdp_config.min_ratio = 2.0;
    c.cdp_config.twap_window = 240;
    c.controller_config = ControllerConfig::default_tick();
    c.liquidation_config.max_liquidations_per_block = 50;
    c
}

fn setup_scenario(config: &ScenarioConfig) -> Scenario {
    let mut scenario = Scenario::new_with_seed(config, SEED);

    // Add standard agents (arber + miner)
    add_agents(ScenarioId::SteadyState, &mut scenario);

    // Initialize agents
    for lp in &mut scenario.lp_agents {
        lp.provide_liquidity(&mut scenario.amm);
    }
    for lp in &mut scenario.il_aware_lps {
        lp.provide_liquidity(&mut scenario.amm);
    }

    // Open 25 vaults at 210-280% CR
    for i in 0..25 {
        let cr = 2.10 + (i as f64) * 0.70 / 24.0;
        let debt = 1000.0;
        let collateral = cr * debt / 50.0;
        let owner = format!("victim_{}", i);
        scenario
            .registry
            .open_vault(&owner, collateral, debt, 0, &scenario.amm)
            .unwrap();
    }

    scenario
}

// ═══════════════════════════════════════════════════════════════════════
// Whale tracker
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
// Attack definitions
// ═══════════════════════════════════════════════════════════════════════

enum AttackAction {
    None,
    SellZec(f64),
    BuyBackAll,
    BuyBackSpread(f64),
}

struct AttackDef {
    name: &'static str,
    whale_start_zec: f64,
    whale_start_zai: f64,
    external_short_size: f64,
    use_black_thursday: bool,
    action_fn: fn(u64, &Whale) -> AttackAction,
}

fn action_baseline(_block: u64, _whale: &Whale) -> AttackAction {
    AttackAction::None
}

fn action_dump_and_hunt(block: u64, _whale: &Whale) -> AttackAction {
    if block >= WARMUP + 1 && block <= WARMUP + 10 {
        AttackAction::SellZec(5_000.0)
    } else if block == WARMUP + 500 {
        AttackAction::BuyBackAll
    } else {
        AttackAction::None
    }
}

fn action_crash_amplify(block: u64, _whale: &Whale) -> AttackAction {
    // Black Thursday crash starts at block 250. Whale dumps at block 250.
    if block == 250 {
        AttackAction::SellZec(10_000.0)
    } else if block == 501 {
        AttackAction::BuyBackAll
    } else {
        AttackAction::None
    }
}

fn action_sustained(block: u64, whale: &Whale) -> AttackAction {
    if block >= WARMUP + 1 && block <= WARMUP + 100 {
        AttackAction::SellZec(1_000.0)
    } else if block >= WARMUP + 101 && block <= WARMUP + 110 {
        // Spread buyback over 10 blocks
        AttackAction::BuyBackSpread(whale.zai / (WARMUP + 111 - block) as f64)
    } else {
        AttackAction::None
    }
}

fn action_short_plus_dump(block: u64, _whale: &Whale) -> AttackAction {
    if block == 250 {
        AttackAction::SellZec(20_000.0)
    } else if block == 500 {
        AttackAction::BuyBackAll
    } else {
        AttackAction::None
    }
}

fn attacks() -> Vec<AttackDef> {
    vec![
        AttackDef {
            name: "baseline",
            whale_start_zec: 0.0,
            whale_start_zai: 0.0,
            external_short_size: 0.0,
            use_black_thursday: false,
            action_fn: action_baseline,
        },
        AttackDef {
            name: "dump_and_hunt",
            whale_start_zec: 50_000.0,
            whale_start_zai: 0.0,
            external_short_size: 0.0,
            use_black_thursday: false,
            action_fn: action_dump_and_hunt,
        },
        AttackDef {
            name: "crash_amplify",
            whale_start_zec: 10_000.0,
            whale_start_zai: 100_000.0,
            external_short_size: 0.0,
            use_black_thursday: true,
            action_fn: action_crash_amplify,
        },
        AttackDef {
            name: "sustained",
            whale_start_zec: 100_000.0,
            whale_start_zai: 0.0,
            external_short_size: 0.0,
            use_black_thursday: false,
            action_fn: action_sustained,
        },
        AttackDef {
            name: "short_plus_dump",
            whale_start_zec: 20_000.0,
            whale_start_zai: 0.0,
            external_short_size: 1_000_000.0,
            use_black_thursday: false,
            action_fn: action_short_plus_dump,
        },
    ]
}

// ═══════════════════════════════════════════════════════════════════════
// Result row
// ═══════════════════════════════════════════════════════════════════════

struct AttackRow {
    name: String,
    whale_start_value: f64,
    whale_end_value: f64,
    whale_pnl: f64,
    whale_pnl_pct: f64,
    liquidations_triggered: u32,
    bad_debt: f64,
    system_verdict: String,
    min_twap: f64,
    min_spot: f64,
    external_short_gain: f64,
    net_pnl_with_short: f64,
}

// ═══════════════════════════════════════════════════════════════════════
// Core run function
// ═══════════════════════════════════════════════════════════════════════

fn run_attack(config: &ScenarioConfig, attack: &AttackDef) -> (AttackRow, Scenario) {
    let mut scenario = setup_scenario(config);

    let prices: Vec<f64> = if attack.use_black_thursday {
        generate_prices(ScenarioId::BlackThursday, BLOCKS, SEED)
    } else {
        vec![TARGET_PRICE; BLOCKS]
    };

    let mut whale = Whale::new(attack.whale_start_zec, attack.whale_start_zai);
    let start_value = whale.value_usd(TARGET_PRICE);

    let mut min_twap = f64::MAX;
    let mut min_spot = f64::MAX;

    for (i, &ext_price) in prices.iter().enumerate() {
        let block = i as u64 + 1;

        // Inject whale action before step
        let action = (attack.action_fn)(block, &whale);
        match action {
            AttackAction::None => {}
            AttackAction::SellZec(amount) => {
                whale.sell_zec(amount, &mut scenario.amm, block);
            }
            AttackAction::BuyBackAll => {
                whale.exit_all_zai(&mut scenario.amm, block);
            }
            AttackAction::BuyBackSpread(amount) => {
                whale.buy_zec(amount, &mut scenario.amm, block);
            }
        }

        scenario.step(block, ext_price);

        // Track min prices after warmup
        if block > WARMUP {
            let twap = scenario.amm.get_twap(config.cdp_config.twap_window as u64);
            min_twap = min_twap.min(twap);
            min_spot = min_spot.min(scenario.amm.spot_price());
        }
    }

    // Final exit: convert any remaining ZAI
    whale.exit_all_zai(&mut scenario.amm, BLOCKS as u64 + 1);

    let end_ext = *prices.last().unwrap_or(&TARGET_PRICE);
    let end_value = whale.value_usd(end_ext);
    let pnl = end_value - start_value;
    let pnl_pct = if start_value > 0.0 {
        pnl / start_value * 100.0
    } else {
        0.0
    };

    let total_liqs: u32 = scenario.metrics.iter().map(|m| m.liquidation_count).sum();
    let bad_debt = scenario
        .metrics
        .last()
        .map(|m| m.bad_debt)
        .unwrap_or(0.0);
    let verdict = report::evaluate_pass_fail(&scenario.metrics, TARGET_PRICE);

    // External short gain: $1 per 1% external price drop
    let ext_drop_pct = (1.0 - end_ext / TARGET_PRICE) * 100.0;
    let short_gain = if attack.external_short_size > 0.0 && ext_drop_pct > 0.0 {
        attack.external_short_size * ext_drop_pct / 100.0
    } else {
        0.0
    };

    let row = AttackRow {
        name: attack.name.to_string(),
        whale_start_value: start_value,
        whale_end_value: end_value,
        whale_pnl: pnl,
        whale_pnl_pct: pnl_pct,
        liquidations_triggered: total_liqs,
        bad_debt,
        system_verdict: verdict.overall.label().to_string(),
        min_twap: if min_twap == f64::MAX { 0.0 } else { min_twap },
        min_spot: if min_spot == f64::MAX { 0.0 } else { min_spot },
        external_short_gain: short_gain,
        net_pnl_with_short: pnl + short_gain,
    };

    (row, scenario)
}

// ═══════════════════════════════════════════════════════════════════════
// Test
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn economic_attack_profitability() {
    let config = attack_config();

    let mut all_rows: Vec<AttackRow> = Vec::new();
    let mut report_entries: Vec<(String, report::PassFailResult, output::SummaryMetrics)> =
        Vec::new();

    for attack in attacks() {
        println!("\n--- Running: {} ---", attack.name);

        let (row, scenario) = run_attack(&config, &attack);
        let metrics = &scenario.metrics;

        let summary = output::compute_summary(metrics, TARGET_PRICE);
        let verdict = report::evaluate_pass_fail(metrics, TARGET_PRICE);

        // Generate individual HTML report
        let label = format!("attack_{}", attack.name);
        let html = report::generate_report(metrics, &config, &label, TARGET_PRICE);
        let html_path = PathBuf::from(format!("reports/economic_attack/{}.html", label));
        report::save_report(&html, &html_path).expect("save individual report");

        report_entries.push((label, verdict, summary));

        println!(
            "  Whale P&L: ${:.2} ({:.2}%)  Liqs: {}  Bad debt: ${:.2}  Min TWAP: ${:.2}  Min Spot: ${:.2}",
            row.whale_pnl, row.whale_pnl_pct, row.liquidations_triggered,
            row.bad_debt, row.min_twap, row.min_spot
        );

        all_rows.push(row);
    }

    // ─── Console summary table ───
    println!("\n{}", "=".repeat(150));
    println!("  F-043: ECONOMIC ATTACK PROFITABILITY TEST — SUMMARY");
    println!("{}", "=".repeat(150));
    println!(
        "{:<18} {:>14} {:>14} {:>9} {:>6} {:>10} {:>10} {:>10} {:>12} {:>12} {:<10}",
        "Attack", "Start Value", "End Value", "P&L %", "Liqs", "Bad Debt",
        "Min TWAP", "Min Spot", "Short Gain", "Net P&L", "Verdict"
    );
    println!("{}", "-".repeat(150));
    for r in &all_rows {
        println!(
            "{:<18} {:>14.2} {:>14.2} {:>8.2}% {:>6} {:>10.2} {:>10.2} {:>10.2} {:>12.2} {:>12.2} {:<10}",
            r.name,
            r.whale_start_value,
            r.whale_end_value,
            r.whale_pnl_pct,
            r.liquidations_triggered,
            r.bad_debt,
            r.min_twap,
            r.min_spot,
            r.external_short_gain,
            r.net_pnl_with_short,
            r.system_verdict,
        );
    }
    println!("{}", "=".repeat(150));

    // ─── Attack profitability analysis ───
    println!("\n  PROFITABILITY ANALYSIS");
    println!("  {}", "-".repeat(60));
    for r in &all_rows {
        if r.name == "baseline" {
            continue;
        }
        let profitable = r.net_pnl_with_short > 0.0;
        let status = if profitable {
            "PROFITABLE — VULNERABILITY"
        } else {
            "UNPROFITABLE — SAFE"
        };
        println!(
            "  {:<18} ${:>12.2} ({:>7.2}%)  {}",
            r.name, r.net_pnl_with_short, r.whale_pnl_pct, status
        );
    }

    // ─── Generate master index HTML ───
    let master_html = report::generate_master_summary(&report_entries);
    let master_path = PathBuf::from("reports/economic_attack/index.html");
    report::save_report(&master_html, &master_path).expect("save master index");
    println!("\n  Reports saved to reports/economic_attack/");

    // ─── Assertions ───
    // Baseline should pass with zero bad debt
    let baseline = &all_rows[0];
    assert_eq!(baseline.name, "baseline");
    assert_eq!(
        baseline.bad_debt, 0.0,
        "Baseline should have zero bad debt"
    );
    assert_eq!(
        baseline.system_verdict, "PASS",
        "Baseline should pass"
    );

    // All 5 runs produce metrics
    assert_eq!(all_rows.len(), 5, "Should have 5 runs");

    // Reports exist
    assert!(master_path.exists(), "Master index should exist");
}
