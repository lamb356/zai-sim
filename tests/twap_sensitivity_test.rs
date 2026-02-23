/// TWAP Window Sensitivity Sweep — F-037
///
/// Maps the exact failure boundary of ZAI's most important parameter: the TWAP
/// window. F-028 established that TWAP inertia IS the stability mechanism.
/// This test sweeps 9 window sizes across 4 crash scenarios at two AMM depths
/// to find the minimum safe window and what breaks first when the window shrinks.
///
/// At $5M depth: TWAP window is expected to be irrelevant (pool depth dominates).
/// At $500K depth: the system CAN fail (per F-035), so window sensitivity appears.
///
/// Windows: 12 (15m), 24 (30m), 48 (1hr), 96 (2hr), 144 (3hr),
///          192 (4hr), 240 (5hr), 360 (7.5hr), 720 (15hr)
/// Scenarios: BlackThursday, SustainedBear, FlashCrash, BankRun
/// Depths: $5M (production), $500K (low liquidity)
use zai_sim::agents::*;
use zai_sim::controller::ControllerConfig;
use zai_sim::output;
use zai_sim::report;
use zai_sim::scenario::{Scenario, ScenarioConfig};
use zai_sim::scenarios::{generate_prices, ScenarioId};

use std::path::PathBuf;

const BLOCKS: usize = 1000;
const SEED: u64 = 42;

fn base_config(twap_window: u64, amm_zec: f64, amm_zai: f64) -> ScenarioConfig {
    let mut config = ScenarioConfig::default();
    config.amm_initial_zec = amm_zec;
    config.amm_initial_zai = amm_zai;
    config.cdp_config.min_ratio = 2.0;
    config.cdp_config.twap_window = twap_window;
    config.controller_config = ControllerConfig::default_tick();
    config.liquidation_config.max_liquidations_per_block = 50;
    config
}

fn add_agents(scenario: &mut Scenario) {
    scenario.arbers.push(Arbitrageur::new(ArbitrageurConfig::default()));
    scenario.miners.push(MinerAgent::new(MinerAgentConfig::default()));

    // 25 vaults at varying CRs — critical for TWAP sensitivity testing
    let vault_configs: Vec<(f64, f64, f64)> = vec![
        // ~210% CR (5 vaults) — closest to liquidation
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
        // ~280% CR (5 vaults) — safest
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

struct TwapResult {
    twap_window: u64,
    scenario_name: String,
    depth_label: String,
    verdict: String,
    mean_peg: f64,
    max_peg: f64,
    total_liquidations: u32,
    total_bad_debt: f64,
    max_zombie_count: u32,
    zombie_duration: u64,
    death_spiral_detected: bool,
    arber_exhausted: bool,
}

fn analyze(
    scenario: &Scenario,
    twap_window: u64,
    scenario_name: &str,
    depth_label: &str,
    target: f64,
) -> TwapResult {
    let verdict = report::evaluate_pass_fail(&scenario.metrics, target);
    let summary = output::compute_summary(&scenario.metrics, target);

    let mut max_zombie_count = 0u32;
    let mut zombie_duration = 0u64;

    for m in &scenario.metrics {
        if m.zombie_vault_count > max_zombie_count {
            max_zombie_count = m.zombie_vault_count;
        }
        if m.zombie_vault_count > 0 {
            zombie_duration += 1;
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

    // Arber exhaustion
    let arber_capital: f64 = scenario
        .arbers
        .iter()
        .map(|a| a.zai_balance + a.zec_balance * target)
        .sum();

    TwapResult {
        twap_window,
        scenario_name: scenario_name.to_string(),
        depth_label: depth_label.to_string(),
        verdict: verdict.overall.label().to_string(),
        mean_peg: summary.mean_peg_deviation,
        max_peg: summary.max_peg_deviation,
        total_liquidations: summary.total_liquidations,
        total_bad_debt: summary.total_bad_debt,
        max_zombie_count,
        zombie_duration,
        death_spiral_detected: death_spiral,
        arber_exhausted: arber_capital < 100.0,
    }
}

fn run_one(scenario_id: ScenarioId, twap_window: u64, amm_zec: f64, amm_zai: f64) -> Scenario {
    let config = base_config(twap_window, amm_zec, amm_zai);
    let prices = generate_prices(scenario_id, BLOCKS, SEED);
    let mut scenario = Scenario::new_with_seed(&config, SEED);
    add_agents(&mut scenario);
    scenario.run(&prices);
    scenario
}

#[test]
fn twap_window_sensitivity_sweep() {
    let report_dir = PathBuf::from("reports/twap_sensitivity");
    let _ = std::fs::create_dir_all(&report_dir);

    let target = 50.0;

    let windows: [u64; 9] = [12, 24, 48, 96, 144, 192, 240, 360, 720];
    let scenarios: Vec<(&str, ScenarioId)> = vec![
        ("black_thursday", ScenarioId::BlackThursday),
        ("sustained_bear", ScenarioId::SustainedBear),
        ("flash_crash", ScenarioId::FlashCrash),
        ("bank_run", ScenarioId::BankRun),
    ];
    let depths: Vec<(&str, f64, f64)> = vec![
        ("$5M", 100_000.0, 5_000_000.0),
        ("$500K", 10_000.0, 500_000.0),
    ];

    let mut all_results: Vec<TwapResult> = Vec::new();
    let mut report_entries: Vec<(String, report::PassFailResult, output::SummaryMetrics)> =
        Vec::new();

    println!(
        "\n═══════════════════════════════════════════════════════════════════════════════════════════════════════════"
    );
    println!("  TWAP WINDOW SENSITIVITY SWEEP — F-037");
    println!("  Config: 200% CR, Tick controller, 25 CDPs at 210-280% CR");
    println!("  9 windows x 4 scenarios x 2 depths = 72 runs");
    println!(
        "═══════════════════════════════════════════════════════════════════════════════════════════════════════════\n"
    );

    for (depth_label, amm_zec, amm_zai) in &depths {
        println!(
            "  ╔══ AMM DEPTH: {} ════════════════════════════════════════════════════════════",
            depth_label
        );

        for (scenario_name, scenario_id) in &scenarios {
            println!(
                "  ┌─ {} ──────────────────────────────────────────────────────────────────",
                scenario_name.to_uppercase()
            );
            println!(
                "  │ {:>6} {:>9} {:>8} {:>8} {:>6} {:>10} {:>8} {:>6} {:>6} {:>6}",
                "Window", "Verdict", "MeanPeg", "MaxPeg", "Liqs", "BadDebt",
                "Zombies", "ZDur", "Sprl", "Exhst"
            );
            println!("  │ {}", "─".repeat(88));

            for &window in &windows {
                let scenario = run_one(*scenario_id, window, *amm_zec, *amm_zai);
                let result = analyze(&scenario, window, scenario_name, depth_label, target);

                // Save HTML report
                let run_name = format!("{}_{}_{}", scenario_name, window,
                    depth_label.trim_start_matches('$').to_lowercase());
                let config = base_config(window, *amm_zec, *amm_zai);
                let html = report::generate_report(&scenario.metrics, &config, &run_name, target);
                let html_path = report_dir.join(format!("{}.html", run_name));
                report::save_report(&html, &html_path).expect("save HTML report");

                let verdict = report::evaluate_pass_fail(&scenario.metrics, target);
                let summary = output::compute_summary(&scenario.metrics, target);
                report_entries.push((run_name, verdict, summary));

                println!(
                    "  │ {:>5}b {:>9} {:>7.2}% {:>7.2}% {:>6} {:>10.2} {:>8} {:>5}b {:>6} {:>6}",
                    result.twap_window,
                    result.verdict,
                    result.mean_peg * 100.0,
                    result.max_peg * 100.0,
                    result.total_liquidations,
                    result.total_bad_debt,
                    result.max_zombie_count,
                    result.zombie_duration,
                    if result.death_spiral_detected { "YES" } else { "no" },
                    if result.arber_exhausted { "YES" } else { "no" },
                );

                all_results.push(result);
            }

            println!(
                "  └──────────────────────────────────────────────────────────────────────────────────\n"
            );
        }

        println!(
            "  ╚══════════════════════════════════════════════════════════════════════════════════\n"
        );
    }

    // Generate master index
    let master_html = report::generate_master_summary(&report_entries);
    let master_path = report_dir.join("index.html");
    report::save_report(&master_html, &master_path).expect("save master index");

    // ── Boundary analysis per depth ────────────────────────────────────
    let scenario_names: Vec<&str> = scenarios.iter().map(|(n, _)| *n).collect();
    let depth_labels: Vec<&str> = depths.iter().map(|(d, _, _)| *d).collect();

    for depth_label in &depth_labels {
        println!(
            "  BOUNDARY ANALYSIS — {} depth",
            depth_label
        );
        println!("  {}", "─".repeat(80));

        for scenario_name in &scenario_names {
            let scenario_results: Vec<&TwapResult> = all_results
                .iter()
                .filter(|r| r.scenario_name == *scenario_name && r.depth_label == *depth_label)
                .collect();

            let min_safe = scenario_results
                .iter()
                .filter(|r| r.total_bad_debt == 0.0 && !r.death_spiral_detected)
                .map(|r| r.twap_window)
                .min();

            let min_not_hard_fail = scenario_results
                .iter()
                .filter(|r| r.verdict != "HARD FAIL")
                .map(|r| r.twap_window)
                .min();

            let bad_debt_boundary = scenario_results
                .iter()
                .filter(|r| r.total_bad_debt > 0.0)
                .map(|r| r.twap_window)
                .max();

            println!(
                "  {:<16} min_safe: {:>5}   not_hard_fail: {:>5}   bad_debt_boundary: {}",
                scenario_name,
                min_safe.map_or("NONE".to_string(), |w| format!("{}b", w)),
                min_not_hard_fail.map_or("NONE".to_string(), |w| format!("{}b", w)),
                bad_debt_boundary.map_or("none".to_string(), |w| format!("<={}b", w)),
            );
        }

        // Min safe across all scenarios at this depth
        let mut safe_across_all = Vec::new();
        for &window in &windows {
            let window_results: Vec<&TwapResult> = all_results
                .iter()
                .filter(|r| r.twap_window == window && r.depth_label == *depth_label)
                .collect();
            let all_safe = window_results
                .iter()
                .all(|r| r.total_bad_debt == 0.0 && !r.death_spiral_detected);
            if all_safe {
                safe_across_all.push(window);
            }
        }

        let min_safe_all = safe_across_all.iter().min().copied();
        println!(
            "  → Min safe across ALL scenarios at {}: {}",
            depth_label,
            min_safe_all.map_or("NONE".to_string(), |w| format!("{}b", w)),
        );

        if let Some(safe_w) = min_safe_all {
            if safe_w < 240 {
                let margin = 240.0 / safe_w as f64;
                println!(
                    "  → 240b recommendation gives {:.1}x safety margin",
                    margin,
                );
            }
        }
        println!();
    }

    // ── Cross-scenario summary per depth ───────────────────────────────
    for depth_label in &depth_labels {
        println!("  CROSS-SCENARIO SUMMARY — {}", depth_label);
        println!(
            "  {:>6} {:>16} {:>16} {:>16} {:>16}",
            "Window", "black_thursday", "sustained_bear", "flash_crash", "bank_run"
        );
        println!("  {}", "─".repeat(74));

        for &window in &windows {
            let mut row = format!("  {:>5}b", window);
            for scenario_name in &scenario_names {
                let r = all_results
                    .iter()
                    .find(|r| {
                        r.twap_window == window
                            && r.scenario_name == *scenario_name
                            && r.depth_label == *depth_label
                    })
                    .unwrap();
                let cell = if r.death_spiral_detected {
                    "SPIRAL".to_string()
                } else if r.total_bad_debt > 0.0 {
                    format!("${:.0}bd", r.total_bad_debt)
                } else {
                    format!("{}", r.verdict)
                };
                row.push_str(&format!(" {:>16}", cell));
            }
            println!("{}", row);
        }
        println!("  {}", "─".repeat(74));
        println!();
    }

    println!("  Reports: reports/twap_sensitivity/");
    println!("  Master:  reports/twap_sensitivity/index.html");
    println!(
        "═══════════════════════════════════════════════════════════════════════════════════════════════════════════\n"
    );

    // ── Assertions ─────────────────────────────────────────────────────

    assert!(master_path.exists(), "Master index should exist");
    assert_eq!(report_entries.len(), 72, "Should have 72 report entries");

    // At $5M/window=240, all scenarios should have zero bad debt
    for r in all_results
        .iter()
        .filter(|r| r.twap_window == 240 && r.depth_label == "$5M")
    {
        assert_eq!(
            r.total_bad_debt, 0.0,
            "At $5M/TWAP=240, {} should have zero bad debt but has {:.2}",
            r.scenario_name, r.total_bad_debt,
        );
    }

    // At $5M/window=720, no death spirals
    for r in all_results
        .iter()
        .filter(|r| r.twap_window == 720 && r.depth_label == "$5M")
    {
        assert!(
            !r.death_spiral_detected,
            "At $5M/TWAP=720, {} should not have death spiral",
            r.scenario_name,
        );
    }
}
