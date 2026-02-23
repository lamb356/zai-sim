/// Graduated Liquidation Test — F-035
///
/// Tests whether graduated (partial) liquidation reduces zombie vault duration
/// without triggering death spirals. Compares standard (all-or-nothing) vs
/// graduated (10% per block) liquidation across 4 scenarios at two AMM depths:
/// - $5M (production-recommended): TWAP inertia high, tests if graduated ever activates
/// - $500K (low liquidity): TWAP responsive, tests if graduated helps or cascades
///
/// Graduated zone: TWAP CR between 150% (graduated_cr_floor) and 200% (min_ratio).
use zai_sim::agents::*;
use zai_sim::controller::ControllerConfig;
use zai_sim::output;
use zai_sim::report;
use zai_sim::scenario::{Scenario, ScenarioConfig};
use zai_sim::scenarios::{generate_prices, ScenarioId};

use std::path::PathBuf;

const BLOCKS: usize = 1000;
const SEED: u64 = 42;

/// V-shape recovery: $50 → $25 → $50 over 1000 blocks.
fn recovery_prices(blocks: usize) -> Vec<f64> {
    let half = blocks / 2;
    (0..blocks)
        .map(|i| {
            if i < half {
                50.0 - 25.0 * (i as f64 / half as f64)
            } else {
                25.0 + 25.0 * ((i - half) as f64 / (blocks - half) as f64)
            }
        })
        .collect()
}

fn config_with_depth(graduated: bool, amm_zec: f64, amm_zai: f64) -> ScenarioConfig {
    let mut config = ScenarioConfig::default();
    config.amm_initial_zec = amm_zec;
    config.amm_initial_zai = amm_zai;
    config.cdp_config.min_ratio = 2.0;
    config.cdp_config.twap_window = 240;
    config.controller_config = ControllerConfig::default_tick();
    config.liquidation_config.max_liquidations_per_block = 50;
    if graduated {
        config.use_graduated_liquidation = true;
        config.liquidation_config.graduated_liquidation = true;
        config.liquidation_config.graduated_pct_per_block = 0.10;
        config.liquidation_config.graduated_cr_floor = 1.5;
    }
    config
}

fn add_agents(scenario: &mut Scenario) {
    scenario
        .arbers
        .push(Arbitrageur::new(ArbitrageurConfig::default()));
    scenario
        .miners
        .push(MinerAgent::new(MinerAgentConfig::default()));

    // 25 vaults at varying CRs for meaningful fee generation and liquidation exposure
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

struct GraduatedResult {
    _label: String,
    mode: String,
    verdict: String,
    mean_peg: f64,
    max_peg: f64,
    total_liquidations: u32,
    graduated_liquidations: u32,
    total_bad_debt: f64,
    max_zombie_count: u32,
    zombie_duration: u64,
    max_zombie_gap: f64,
    death_spiral_detected: bool,
    total_liq_volume_zec: f64,
}

fn analyze(scenario: &Scenario, label: &str, mode: &str, target: f64) -> GraduatedResult {
    let verdict = report::evaluate_pass_fail(&scenario.metrics, target);
    let summary = output::compute_summary(&scenario.metrics, target);

    let mut max_zombie_count = 0u32;
    let mut zombie_duration = 0u64;
    let mut max_zombie_gap = 0.0f64;
    let mut total_graduated = 0u32;

    for m in &scenario.metrics {
        if m.zombie_vault_count > max_zombie_count {
            max_zombie_count = m.zombie_vault_count;
        }
        if m.zombie_vault_count > 0 {
            zombie_duration += 1;
        }
        if m.max_zombie_gap > max_zombie_gap {
            max_zombie_gap = m.max_zombie_gap;
        }
        total_graduated += m.graduated_liquidation_count;
    }

    // Death spiral detection: AMM price dropped >90% with no recovery in last 100 blocks
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

    // Total liquidation volume in ZEC
    let total_liq_volume_zec: f64 = scenario
        .liquidation_engine
        .history
        .iter()
        .map(|r| r.collateral_seized)
        .sum();

    GraduatedResult {
        _label: label.to_string(),
        mode: mode.to_string(),
        verdict: verdict.overall.label().to_string(),
        mean_peg: summary.mean_peg_deviation,
        max_peg: summary.max_peg_deviation,
        total_liquidations: summary.total_liquidations,
        graduated_liquidations: total_graduated,
        total_bad_debt: summary.total_bad_debt,
        max_zombie_count,
        zombie_duration,
        max_zombie_gap,
        death_spiral_detected: death_spiral,
        total_liq_volume_zec,
    }
}

fn run_one(prices: &[f64], graduated: bool, amm_zec: f64, amm_zai: f64) -> Scenario {
    let config = config_with_depth(graduated, amm_zec, amm_zai);
    let mut scenario = Scenario::new_with_seed(&config, SEED);
    add_agents(&mut scenario);
    scenario.run(prices);
    scenario
}

fn print_section(
    name: &str,
    depth_label: &str,
    std_result: &GraduatedResult,
    grad_result: &GraduatedResult,
) {
    println!(
        "  ┌─ {} ({}) ──────────────────────────────────────────────────────────",
        name, depth_label
    );
    println!(
        "  │ {:<12} {:>9} {:>8} {:>8} {:>6} {:>6} {:>10} {:>8} {:>6} {:>7} {:>6} {:>10}",
        "Mode", "Verdict", "MeanPeg", "MaxPeg", "Liqs", "Grad", "BadDebt", "Zombies",
        "ZDur", "ZGap", "Sprl", "LiqVolZEC"
    );
    println!("  │ {}", "─".repeat(107));

    for r in [std_result, grad_result] {
        println!(
            "  │ {:<12} {:>9} {:>7.2}% {:>7.2}% {:>6} {:>6} {:>10.2} {:>8} {:>5}b {:>6.3} {:>6} {:>10.1}",
            r.mode,
            r.verdict,
            r.mean_peg * 100.0,
            r.max_peg * 100.0,
            r.total_liquidations,
            r.graduated_liquidations,
            r.total_bad_debt,
            r.max_zombie_count,
            r.zombie_duration,
            r.max_zombie_gap,
            if r.death_spiral_detected { "YES" } else { "no" },
            r.total_liq_volume_zec,
        );
    }

    let zombie_delta =
        grad_result.max_zombie_count as i64 - std_result.max_zombie_count as i64;
    let dur_delta = grad_result.zombie_duration as i64 - std_result.zombie_duration as i64;
    let debt_delta = grad_result.total_bad_debt - std_result.total_bad_debt;
    let liq_delta =
        grad_result.total_liquidations as i64 - std_result.total_liquidations as i64;

    println!("  │");
    println!(
        "  │ Delta: liqs {:+}, grad +{}, zombies {:+}, duration {:+}b, bad_debt {:+.2}",
        liq_delta,
        grad_result.graduated_liquidations,
        zombie_delta,
        dur_delta,
        debt_delta,
    );

    let helped = grad_result.total_bad_debt <= std_result.total_bad_debt
        && grad_result.max_zombie_count <= std_result.max_zombie_count;
    let cascade = grad_result.death_spiral_detected && !std_result.death_spiral_detected;

    if cascade {
        println!(
            "  │ → DEATH SPIRAL: graduated mode triggered cascade that standard avoided!"
        );
    } else if grad_result.graduated_liquidations == 0 && std_result.total_liquidations == 0 {
        println!("  │ → INERT: graduated never activated (TWAP too resilient for warning zone)");
    } else if helped {
        println!("  │ → HELPED: reduced zombie risk without cascade");
    } else {
        println!("  │ → NEUTRAL/MIXED: no clear improvement");
    }

    println!(
        "  └──────────────────────────────────────────────────────────────────────────────────\n"
    );
}

#[test]
fn graduated_liquidation_comparison() {
    let report_dir = PathBuf::from("reports/graduated_liquidation");
    let _ = std::fs::create_dir_all(&report_dir);

    // 4 scenarios: 3 from ScenarioId + 1 custom recovery
    let scenario_specs: Vec<(&str, Vec<f64>)> = vec![
        (
            "black_thursday",
            generate_prices(ScenarioId::BlackThursday, BLOCKS, SEED),
        ),
        (
            "sustained_bear",
            generate_prices(ScenarioId::SustainedBear, BLOCKS, SEED),
        ),
        (
            "bank_run",
            generate_prices(ScenarioId::BankRun, BLOCKS, SEED),
        ),
        ("recovery", recovery_prices(BLOCKS)),
    ];

    // Two AMM depths: $5M (production) and $500K (low liquidity)
    let amm_depths: Vec<(&str, f64, f64)> = vec![
        ("$5M", 100_000.0, 5_000_000.0),
        ("$500K", 10_000.0, 500_000.0),
    ];

    let mut all_results: Vec<(String, String, GraduatedResult, GraduatedResult)> = Vec::new();
    let mut report_entries: Vec<(String, report::PassFailResult, output::SummaryMetrics)> =
        Vec::new();

    println!(
        "\n═══════════════════════════════════════════════════════════════════════════════════════════════════════════"
    );
    println!("  GRADUATED LIQUIDATION COMPARISON — F-035");
    println!("  Config: 200% CR, 150% graduated floor, 10%/block, Tick, 240-TWAP, 25 CDPs");
    println!("  AMM depths: $5M (production) and $500K (low liquidity)");
    println!("  4 scenarios x 2 modes x 2 depths = 16 runs");
    println!(
        "═══════════════════════════════════════════════════════════════════════════════════════════════════════════\n"
    );

    for (depth_label, amm_zec, amm_zai) in &amm_depths {
        let target = *amm_zai / *amm_zec; // $50

        for (name, prices) in &scenario_specs {
            let std_scenario = run_one(prices, false, *amm_zec, *amm_zai);
            let std_result = analyze(&std_scenario, name, "standard", target);

            let grad_scenario = run_one(prices, true, *amm_zec, *amm_zai);
            let grad_result = analyze(&grad_scenario, name, "graduated", target);

            // Save HTML reports
            for (suffix, scen, graduated) in [
                ("standard", &std_scenario, false),
                ("graduated", &grad_scenario, true),
            ] {
                let run_name = format!(
                    "{}_{}_{}", name, suffix,
                    depth_label.trim_start_matches('$').to_lowercase()
                );
                let config = config_with_depth(graduated, *amm_zec, *amm_zai);
                let html = report::generate_report(&scen.metrics, &config, &run_name, target);
                let html_path = report_dir.join(format!("{}.html", run_name));
                report::save_report(&html, &html_path).expect("save HTML report");

                let verdict = report::evaluate_pass_fail(&scen.metrics, target);
                let summary = output::compute_summary(&scen.metrics, target);
                report_entries.push((run_name, verdict, summary));
            }

            print_section(name, depth_label, &std_result, &grad_result);
            all_results.push((
                name.to_string(),
                depth_label.to_string(),
                std_result,
                grad_result,
            ));
        }
    }

    // Generate master index
    let master_html = report::generate_master_summary(&report_entries);
    let master_path = report_dir.join("index.html");
    report::save_report(&master_html, &master_path).expect("save master index");

    // ── Summary table ──────────────────────────────────────────────────
    println!("  GRADUATED LIQUIDATION SUMMARY");
    println!(
        "  {:<6} {:<16} {:<12} {:>9} {:>6} {:>6} {:>10} {:>8} {:>6} {:>6}",
        "Depth", "Scenario", "Mode", "Verdict", "Liqs", "Grad", "BadDebt", "Zombies", "ZDur", "Sprl"
    );
    println!("  {}", "─".repeat(100));

    for (name, depth, ref s, ref g) in &all_results {
        for r in [s, g] {
            println!(
                "  {:<6} {:<16} {:<12} {:>9} {:>6} {:>6} {:>10.2} {:>8} {:>5}b {:>6}",
                if r.mode == "standard" { depth.as_str() } else { "" },
                if r.mode == "standard" { name.as_str() } else { "" },
                r.mode,
                r.verdict,
                r.total_liquidations,
                r.graduated_liquidations,
                r.total_bad_debt,
                r.max_zombie_count,
                r.zombie_duration,
                if r.death_spiral_detected { "YES" } else { "no" },
            );
        }
    }

    println!("  {}", "─".repeat(100));

    // Count activations
    let graduated_activations: u32 = all_results.iter().map(|(_, _, _, g)| g.graduated_liquidations).sum();
    let death_spirals: usize = all_results.iter().filter(|(_, _, s, g)| g.death_spiral_detected && !s.death_spiral_detected).count();
    println!(
        "\n  Total graduated activations: {}, Death spirals triggered: {}",
        graduated_activations, death_spirals
    );
    println!("\n  Reports: reports/graduated_liquidation/");
    println!("  Master:  reports/graduated_liquidation/index.html");
    println!(
        "═══════════════════════════════════════════════════════════════════════════════════════════════════════════\n"
    );

    // Verify reports exist
    assert!(master_path.exists(), "Master index should exist");
    assert_eq!(
        report_entries.len(),
        16,
        "Should have 16 report entries (4 scenarios x 2 modes x 2 depths)"
    );

    // Document bad debt impact: graduated may increase bad debt (finding, not bug)
    println!("\n  ── BAD DEBT IMPACT ──────────────────────────────────────────────────");
    let mut worse_count = 0u32;
    for (name, depth, ref s, ref g) in &all_results {
        if g.total_bad_debt > s.total_bad_debt + 1.0 {
            worse_count += 1;
            println!(
                "  WARNING {}/{}: graduated increased bad debt {:.2} → {:.2} ({:+.1}%)",
                name,
                depth,
                s.total_bad_debt,
                g.total_bad_debt,
                if s.total_bad_debt > 0.0 {
                    (g.total_bad_debt - s.total_bad_debt) / s.total_bad_debt * 100.0
                } else {
                    f64::INFINITY
                },
            );
        }
    }
    if worse_count == 0 {
        println!("  No bad debt regressions.");
    }
    println!();

    // At $5M: graduated should be completely inert (identical to standard)
    for (name, depth, ref s, ref g) in &all_results {
        if *depth == "$5M" {
            assert_eq!(
                g.graduated_liquidations, 0,
                "{} at $5M: graduated should be inert but had {} activations",
                name, g.graduated_liquidations,
            );
            assert!(
                (g.total_bad_debt - s.total_bad_debt).abs() < 0.01,
                "{} at $5M: bad debt should be identical in both modes",
                name,
            );
        }
    }
}
