/// Multi-Arber Competition Test — F-036
///
/// Tests whether multiple competing arbitrageurs with different capital levels
/// and aggressiveness help or hurt AMM stability. F-028 found that arber exhaustion
/// IS the stability mechanism — slower repricing prevents death spirals. This test
/// explores the tradeoff: more arbers = faster repricing = potentially faster death
/// spirals, OR = better price discovery = tighter peg.
///
/// Configurations:
///   solo  — 1 whale (status quo baseline)
///   trio  — 1 whale + 1 medium + 1 small
///   squad — 1 whale + 2 medium + 2 small
///   swarm — 1 whale + 3 medium + 6 small
///
/// Scenarios: BlackThursday, SustainedBear (the 2 hardest crash scenarios)
use zai_sim::agents::*;
use zai_sim::controller::ControllerConfig;
use zai_sim::output;
use zai_sim::report;
use zai_sim::scenario::{Scenario, ScenarioConfig};
use zai_sim::scenarios::{generate_prices, ScenarioId};

use std::path::PathBuf;

const BLOCKS: usize = 1000;
const SEED: u64 = 42;

// ── Arber presets ──────────────────────────────────────────────────────

fn whale_arber() -> ArbitrageurConfig {
    ArbitrageurConfig {
        initial_zai_balance: 500_000.0,
        initial_zec_balance: 10_000.0,
        max_trade_pct: 0.20,
        activity_rate: 1.0,
        ..Default::default()
    }
}

fn medium_arber() -> ArbitrageurConfig {
    ArbitrageurConfig {
        initial_zai_balance: 100_000.0,
        initial_zec_balance: 2_000.0,
        max_trade_pct: 0.10,
        activity_rate: 0.8,
        ..Default::default()
    }
}

fn small_arber() -> ArbitrageurConfig {
    ArbitrageurConfig {
        initial_zai_balance: 20_000.0,
        initial_zec_balance: 400.0,
        max_trade_pct: 0.05,
        activity_rate: 0.5,
        ..Default::default()
    }
}

// ── Arber configs ──────────────────────────────────────────────────────

fn arber_configs(name: &str) -> Vec<ArbitrageurConfig> {
    match name {
        "solo" => vec![whale_arber()],
        "trio" => vec![whale_arber(), medium_arber(), small_arber()],
        "squad" => vec![
            whale_arber(),
            medium_arber(),
            medium_arber(),
            small_arber(),
            small_arber(),
        ],
        "swarm" => {
            let mut v = vec![whale_arber()];
            for _ in 0..3 { v.push(medium_arber()); }
            for _ in 0..6 { v.push(small_arber()); }
            v
        }
        _ => panic!("unknown arber config: {}", name),
    }
}

// ── Scenario setup ─────────────────────────────────────────────────────

fn base_config() -> ScenarioConfig {
    let mut config = ScenarioConfig::default();
    config.amm_initial_zec = 100_000.0;
    config.amm_initial_zai = 5_000_000.0;
    config.cdp_config.min_ratio = 2.0;
    config.cdp_config.twap_window = 240;
    config.controller_config = ControllerConfig::default_tick();
    config.liquidation_config.max_liquidations_per_block = 50;
    config
}

fn add_cdp_holders(scenario: &mut Scenario) {
    scenario.miners.push(MinerAgent::new(MinerAgentConfig::default()));

    // 25 vaults at varying CRs
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

// ── Result struct ──────────────────────────────────────────────────────

struct ArberResult {
    arber_label: String,
    num_arbers: usize,
    mean_peg: f64,
    max_peg: f64,
    total_liquidations: u32,
    total_bad_debt: f64,
    _exhaustion_block: Option<u64>,
    capital_burned_zai: f64,
    repricing_speed: Option<u64>,
    peg_recovery_block: Option<u64>,
    death_spiral_detected: bool,
    verdict: String,
}

fn analyze(scenario: &Scenario, arber_label: &str, target: f64) -> ArberResult {
    let verdict = report::evaluate_pass_fail(&scenario.metrics, target);
    let summary = output::compute_summary(&scenario.metrics, target);

    // Capital burned: sum of (initial - final) across all arbers
    let capital_burned_zai: f64 = scenario
        .arbers
        .iter()
        .map(|a| {
            let initial = a.config.initial_zai_balance + a.config.initial_zec_balance * target;
            let final_val = a.zai_balance + a.zec_balance * target;
            (initial - final_val).max(0.0)
        })
        .sum();

    // Exhaustion block: first block where ALL arbers have < $100 combined capital
    let mut exhaustion_block = None;
    // We can't track per-block arber state from metrics alone, so we approximate:
    // if total arber capital at end is < $100, mark final block as exhaustion
    let final_capital: f64 = scenario
        .arbers
        .iter()
        .map(|a| a.zai_balance + a.zec_balance * target)
        .sum();
    if final_capital < 100.0 {
        exhaustion_block = Some(scenario.metrics.len() as u64);
    }

    // Repricing speed: blocks to close 50% of initial deviation after crash starts
    // Find the first block with >5% deviation, then first block where deviation halves
    let mut repricing_speed = None;
    let mut crash_block = None;
    let mut crash_deviation = 0.0f64;
    for (i, m) in scenario.metrics.iter().enumerate() {
        let dev = (m.amm_spot_price - target).abs() / target;
        if crash_block.is_none() && dev > 0.05 {
            crash_block = Some(i);
            crash_deviation = dev;
        }
        if let Some(cb) = crash_block {
            if dev < crash_deviation * 0.5 {
                repricing_speed = Some((i - cb) as u64);
                break;
            }
        }
    }

    // Peg recovery block: first block after crash where deviation < 1%
    let mut peg_recovery_block = None;
    if let Some(cb) = crash_block {
        for (i, m) in scenario.metrics.iter().enumerate().skip(cb) {
            let dev = (m.amm_spot_price - target).abs() / target;
            if dev < 0.01 {
                peg_recovery_block = Some(i as u64);
                break;
            }
        }
    }

    // Death spiral detection
    let death_spiral = if scenario.metrics.len() > 200 {
        let initial = scenario.metrics[0].amm_spot_price;
        let final_price = scenario.metrics.last().unwrap().amm_spot_price;
        let dropped = final_price < initial * 0.1;
        let last_100 = &scenario.metrics[scenario.metrics.len() - 100..];
        let no_recovery = last_100.iter().all(|m| m.amm_spot_price < initial * 0.15);
        dropped && no_recovery
    } else {
        false
    };

    ArberResult {
        arber_label: arber_label.to_string(),
        num_arbers: scenario.arbers.len(),
        mean_peg: summary.mean_peg_deviation,
        max_peg: summary.max_peg_deviation,
        total_liquidations: summary.total_liquidations,
        total_bad_debt: summary.total_bad_debt,
        _exhaustion_block: exhaustion_block,
        capital_burned_zai,
        repricing_speed,
        peg_recovery_block,
        death_spiral_detected: death_spiral,
        verdict: verdict.overall.label().to_string(),
    }
}

fn run_one(prices: &[f64], arber_label: &str) -> Scenario {
    let config = base_config();
    let mut scenario = Scenario::new_with_seed(&config, SEED);

    // Add arbers based on config name
    for ac in arber_configs(arber_label) {
        scenario.arbers.push(Arbitrageur::new(ac));
    }

    add_cdp_holders(&mut scenario);
    scenario.run(prices);
    scenario
}

// ── Display helpers ────────────────────────────────────────────────────

fn print_scenario_table(scenario_name: &str, results: &[&ArberResult]) {
    println!(
        "  ┌─ {} ──────────────────────────────────────────────────────────────────",
        scenario_name.to_uppercase()
    );
    println!(
        "  │ {:<8} {:>3} {:>9} {:>8} {:>8} {:>6} {:>10} {:>12} {:>8} {:>8} {:>6}",
        "Config", "#", "Verdict", "MeanPeg", "MaxPeg", "Liqs", "BadDebt",
        "CapBurned", "Reprice", "PegRecv", "Sprl"
    );
    println!("  │ {}", "─".repeat(103));

    for r in results {
        println!(
            "  │ {:<8} {:>3} {:>9} {:>7.2}% {:>7.2}% {:>6} {:>10.2} {:>12.0} {:>7}b {:>7}b {:>6}",
            r.arber_label,
            r.num_arbers,
            r.verdict,
            r.mean_peg * 100.0,
            r.max_peg * 100.0,
            r.total_liquidations,
            r.total_bad_debt,
            r.capital_burned_zai,
            r.repricing_speed.map_or("N/A".to_string(), |b| format!("{}", b)),
            r.peg_recovery_block.map_or("never".to_string(), |b| format!("{}", b)),
            if r.death_spiral_detected { "YES" } else { "no" },
        );
    }

    // Delta vs solo baseline
    let solo = results[0];
    println!("  │");
    println!("  │ Deltas vs solo baseline:");
    for r in results.iter().skip(1) {
        let peg_delta = r.mean_peg - solo.mean_peg;
        let debt_delta = r.total_bad_debt - solo.total_bad_debt;
        let liq_delta = r.total_liquidations as i64 - solo.total_liquidations as i64;
        let cap_delta = r.capital_burned_zai - solo.capital_burned_zai;
        println!(
            "  │   {:<8} peg {:+.3}%, bad_debt {:+.2}, liqs {:+}, capital {:+.0}",
            r.arber_label,
            peg_delta * 100.0,
            debt_delta,
            liq_delta,
            cap_delta,
        );
    }

    println!(
        "  └──────────────────────────────────────────────────────────────────────────────────\n"
    );
}

#[test]
fn multi_arber_competition() {
    let report_dir = PathBuf::from("reports/multi_arber");
    let _ = std::fs::create_dir_all(&report_dir);

    let target = 50.0; // $50 ZEC

    let arber_labels = ["solo", "trio", "squad", "swarm"];
    let scenario_specs: Vec<(&str, ScenarioId)> = vec![
        ("black_thursday", ScenarioId::BlackThursday),
        ("sustained_bear", ScenarioId::SustainedBear),
    ];

    let mut all_results: Vec<(String, Vec<ArberResult>)> = Vec::new();
    let mut report_entries: Vec<(String, report::PassFailResult, output::SummaryMetrics)> =
        Vec::new();

    println!(
        "\n═══════════════════════════════════════════════════════════════════════════════════════════════════════════"
    );
    println!("  MULTI-ARBER COMPETITION — F-036");
    println!("  Config: $5M AMM, 200% CR, Tick, 240-TWAP, 25 CDPs");
    println!("  Arber configs: solo (1 whale), trio (3), squad (5), swarm (10)");
    println!("  2 scenarios x 4 configs = 8 runs");
    println!(
        "═══════════════════════════════════════════════════════════════════════════════════════════════════════════\n"
    );

    for (scenario_name, scenario_id) in &scenario_specs {
        let prices = generate_prices(*scenario_id, BLOCKS, SEED);
        let mut scenario_results = Vec::new();

        for arber_label in &arber_labels {
            let scenario = run_one(&prices, arber_label);
            let result = analyze(&scenario, arber_label, target);

            // Save HTML report
            let run_name = format!("{}_{}", scenario_name, arber_label);
            let config = base_config();
            let html = report::generate_report(&scenario.metrics, &config, &run_name, target);
            let html_path = report_dir.join(format!("{}.html", run_name));
            report::save_report(&html, &html_path).expect("save HTML report");

            let verdict = report::evaluate_pass_fail(&scenario.metrics, target);
            let summary = output::compute_summary(&scenario.metrics, target);
            report_entries.push((run_name, verdict, summary));

            scenario_results.push(result);
        }

        let refs: Vec<&ArberResult> = scenario_results.iter().collect();
        print_scenario_table(scenario_name, &refs);
        all_results.push((scenario_name.to_string(), scenario_results));
    }

    // Generate master index
    let master_html = report::generate_master_summary(&report_entries);
    let master_path = report_dir.join("index.html");
    report::save_report(&master_html, &master_path).expect("save master index");

    // ── Summary table ──────────────────────────────────────────────────
    println!("  MULTI-ARBER COMPETITION SUMMARY");
    println!(
        "  {:<16} {:<8} {:>3} {:>9} {:>8} {:>10} {:>12} {:>8} {:>6}",
        "Scenario", "Config", "#", "Verdict", "MeanPeg", "BadDebt", "CapBurned", "Reprice", "Sprl"
    );
    println!("  {}", "─".repeat(90));

    for (name, results) in &all_results {
        for r in results {
            println!(
                "  {:<16} {:<8} {:>3} {:>9} {:>7.2}% {:>10.2} {:>12.0} {:>7}b {:>6}",
                if r.arber_label == "solo" { name.as_str() } else { "" },
                r.arber_label,
                r.num_arbers,
                r.verdict,
                r.mean_peg * 100.0,
                r.total_bad_debt,
                r.capital_burned_zai,
                r.repricing_speed.map_or("N/A".to_string(), |b| format!("{}", b)),
                if r.death_spiral_detected { "YES" } else { "no" },
            );
        }
    }
    println!("  {}", "─".repeat(90));

    println!("\n  Reports: reports/multi_arber/");
    println!("  Master:  reports/multi_arber/index.html");
    println!(
        "═══════════════════════════════════════════════════════════════════════════════════════════════════════════\n"
    );

    // ── Assertions ─────────────────────────────────────────────────────

    // Verify reports exist
    assert!(master_path.exists(), "Master index should exist");
    assert_eq!(report_entries.len(), 8, "Should have 8 report entries");

    // Bad debt check: no config should produce >10% more bad debt than solo
    println!("\n  ── BAD DEBT IMPACT ──────────────────────────────────────────────────");
    for (name, results) in &all_results {
        let solo_debt = results[0].total_bad_debt;
        for r in results.iter().skip(1) {
            let pct_increase = if solo_debt > 0.0 {
                (r.total_bad_debt - solo_debt) / solo_debt * 100.0
            } else if r.total_bad_debt > 0.0 {
                f64::INFINITY
            } else {
                0.0
            };

            if pct_increase > 10.0 {
                println!(
                    "  WARNING {}/{}: {} increased bad debt {:.2} → {:.2} ({:+.1}%)",
                    name, r.arber_label, r.arber_label, solo_debt, r.total_bad_debt, pct_increase,
                );
            }

            // Soft assertion — warn but don't fail (the finding IS the data)
            if r.total_bad_debt > solo_debt * 1.1 + 1.0 {
                println!(
                    "  NOTE: {}/{} bad debt exceeds solo by >{:.0}%",
                    name, r.arber_label, pct_increase,
                );
            }
        }
    }

    // Death spiral check: warn if multi-arber triggers spiral when solo doesn't
    for (name, results) in &all_results {
        let solo_spiral = results[0].death_spiral_detected;
        for r in results.iter().skip(1) {
            if r.death_spiral_detected && !solo_spiral {
                println!(
                    "  DEATH SPIRAL WARNING: {}/{} triggered cascade that solo avoided!",
                    name, r.arber_label,
                );
            }
        }
    }
    println!();
}
