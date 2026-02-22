/// Final Reports Generation
///
/// Generates fresh HTML reports from the final codebase at reports/final/.
/// Config: $5M AMM (100K ZEC + 5M ZAI), 200% CR, Tick controller, 240-block TWAP,
/// stochastic: true, seed=42.
///
/// Outputs:
///   - 13 per-scenario HTML reports (1000 blocks each)
///   - 1 sustained_bear_50k.html (50,000 blocks)
///   - index.html master summary (14 entries)
use zai_sim::controller::ControllerConfig;
use zai_sim::output;
use zai_sim::report;
use zai_sim::scenario::ScenarioConfig;
use zai_sim::scenarios::{run_stress, ScenarioId};
use std::path::PathBuf;

const BLOCKS: usize = 1000;
const LONG_BLOCKS: usize = 50_000;
const SEED: u64 = 42;

fn config_5m_stochastic() -> ScenarioConfig {
    let mut config = ScenarioConfig::default();
    config.amm_initial_zec = 100_000.0;
    config.amm_initial_zai = 5_000_000.0;
    config.cdp_config.min_ratio = 2.0;
    config.cdp_config.twap_window = 240;
    config.controller_config = ControllerConfig::default_tick();
    config.stochastic = true;
    config.noise_sigma = 0.02;
    config.arber_activity_rate = 0.8;
    config.demand_jitter_blocks = 10;
    config.miner_batch_window = 10;
    config
}

#[test]
fn final_reports() {
    let report_dir = PathBuf::from("reports/final");
    std::fs::create_dir_all(&report_dir).unwrap();

    let config = config_5m_stochastic();
    let target = config.initial_redemption_price;

    println!(
        "\n═══════════════════════════════════════════════════════════════════════════════════════════════════"
    );
    println!("  FINAL REPORT GENERATION");
    println!("  Config: $5M AMM, 200% CR, Tick, 240-block TWAP, stochastic (seed=42)");
    println!("  Output: reports/final/");
    println!(
        "═══════════════════════════════════════════════════════════════════════════════════════════════════\n"
    );

    let mut entries: Vec<(String, report::PassFailResult, output::SummaryMetrics)> = Vec::new();

    // 13 standard scenarios at 1000 blocks
    println!(
        "  {:<22} {:>8} {:>8} {:>8} {:>6} {:>8} {:>8}",
        "Scenario", "Verdict", "MeanPeg", "MaxPeg", "Liqs", "BadDebt", "Breakers"
    );
    println!("  {}", "─".repeat(75));

    for sid in ScenarioId::all() {
        let scenario = run_stress(sid, &config, BLOCKS, SEED);

        let html = report::generate_report(&scenario.metrics, &config, sid.name(), target);
        report::save_report(&html, &report_dir.join(format!("{}.html", sid.name()))).unwrap();

        let verdict = report::evaluate_pass_fail(&scenario.metrics, target);
        let summary = output::compute_summary(&scenario.metrics, target);

        println!(
            "  {:<22} {:>8} {:>7.2}% {:>7.2}% {:>6} {:>8.2} {:>8}",
            sid.name(),
            verdict.overall.label(),
            summary.mean_peg_deviation * 100.0,
            summary.max_peg_deviation * 100.0,
            summary.total_liquidations,
            summary.total_bad_debt,
            summary.breaker_triggers,
        );

        entries.push((sid.name().to_string(), verdict, summary));
    }

    // Sustained bear at 50,000 blocks
    println!("\n  ── Long Duration ──");
    let sb_scenario = run_stress(ScenarioId::SustainedBear, &config, LONG_BLOCKS, SEED);

    let sb_html = report::generate_report(
        &sb_scenario.metrics,
        &config,
        "sustained_bear_50k",
        target,
    );
    report::save_report(&sb_html, &report_dir.join("sustained_bear_50k.html")).unwrap();

    let sb_verdict = report::evaluate_pass_fail(&sb_scenario.metrics, target);
    let sb_summary = output::compute_summary(&sb_scenario.metrics, target);

    println!(
        "  {:<22} {:>8} {:>7.2}% {:>7.2}% {:>6} {:>8.2} {:>8}",
        "sustained_bear_50k",
        sb_verdict.overall.label(),
        sb_summary.mean_peg_deviation * 100.0,
        sb_summary.max_peg_deviation * 100.0,
        sb_summary.total_liquidations,
        sb_summary.total_bad_debt,
        sb_summary.breaker_triggers,
    );

    entries.push(("sustained_bear_50k".to_string(), sb_verdict, sb_summary));

    // Generate master index
    let master_html = report::generate_master_summary(&entries);
    report::save_report(&master_html, &report_dir.join("index.html")).unwrap();

    // Summary
    let pass_count = entries
        .iter()
        .filter(|(_, v, _)| v.overall == report::Verdict::Pass)
        .count();
    let soft_fail_count = entries
        .iter()
        .filter(|(_, v, _)| v.overall == report::Verdict::SoftFail)
        .count();
    let hard_fail_count = entries
        .iter()
        .filter(|(_, v, _)| v.overall == report::Verdict::HardFail)
        .count();

    println!("\n  ── Summary ──");
    println!(
        "  {} PASS / {} SOFT FAIL / {} HARD FAIL out of {} scenarios",
        pass_count,
        soft_fail_count,
        hard_fail_count,
        entries.len(),
    );
    println!("  Reports written to: reports/final/");
    println!("  Master index: reports/final/index.html");
    println!(
        "  Total HTML files: {} (13 standard + 1 long-duration + 1 index)",
        entries.len() + 1,
    );

    println!(
        "\n═══════════════════════════════════════════════════════════════════════════════════════════════════\n"
    );

    // Verify all files exist
    assert!(report_dir.join("index.html").exists(), "index.html missing");
    for sid in ScenarioId::all() {
        assert!(
            report_dir.join(format!("{}.html", sid.name())).exists(),
            "{}.html missing",
            sid.name()
        );
    }
    assert!(
        report_dir.join("sustained_bear_50k.html").exists(),
        "sustained_bear_50k.html missing"
    );
}
