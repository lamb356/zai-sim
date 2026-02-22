use zai_sim::output;
use zai_sim::report;
use zai_sim::scenario::ScenarioConfig;
use zai_sim::scenarios::{run_stress, ScenarioId};

use std::path::PathBuf;

const BLOCKS: usize = 1000;
const SEED: u64 = 42;

#[test]
fn full_suite_all_13_scenarios() {
    let config = ScenarioConfig::default();
    let target = config.initial_redemption_price;
    let report_dir = PathBuf::from("reports");
    let _ = std::fs::create_dir_all(&report_dir);

    let mut entries = Vec::new();

    // Collect results for summary table
    struct Row {
        name: String,
        verdict: String,
        mean_peg: f64,
        max_peg: f64,
        liqs: u32,
        bad_debt: f64,
        volatility: f64,
        halt_blocks: u64,
        breaker_triggers: u32,
    }
    let mut rows: Vec<Row> = Vec::new();

    println!("\n  Running all 13 stress scenarios ({} blocks each, seed={})...\n", BLOCKS, SEED);

    for sid in ScenarioId::all() {
        let scenario = run_stress(sid, &config, BLOCKS, SEED);

        // Generate + save individual HTML report
        let html = report::generate_report(&scenario.metrics, &config, sid.name(), target);
        let html_path = report_dir.join(format!("{}.html", sid.name()));
        report::save_report(&html, &html_path).expect("save individual report");

        // Evaluate
        let verdict = report::evaluate_pass_fail(&scenario.metrics, target);
        let summary = output::compute_summary(&scenario.metrics, target);

        // Compute volatility ratio (std/mean of AMM price)
        let prices: Vec<f64> = scenario.metrics.iter().map(|m| m.amm_spot_price).collect();
        let mean = prices.iter().sum::<f64>() / prices.len() as f64;
        let variance = prices.iter().map(|p| (p - mean).powi(2)).sum::<f64>() / prices.len() as f64;
        let volatility = variance.sqrt() / mean;

        rows.push(Row {
            name: sid.name().to_string(),
            verdict: verdict.overall.label().to_string(),
            mean_peg: summary.mean_peg_deviation,
            max_peg: summary.max_peg_deviation,
            liqs: summary.total_liquidations,
            bad_debt: summary.total_bad_debt,
            volatility,
            halt_blocks: summary.halt_blocks,
            breaker_triggers: summary.breaker_triggers,
        });

        // Collect for master summary
        entries.push((sid.name().to_string(), verdict, summary));
    }

    // Generate master summary
    let master_html = report::generate_master_summary(&entries);
    let master_path = report_dir.join("index.html");
    report::save_report(&master_html, &master_path).expect("save master summary");

    // Print summary table
    println!("\n═══════════════════════════════════════════════════════════════════════════════════════════════════════════════════");
    println!("  ZAI SIMULATOR — FULL STRESS SUITE (default params: 150% CR, 1h TWAP, PI controller, $500K AMM)");
    println!("═══════════════════════════════════════════════════════════════════════════════════════════════════════════════════");
    println!(
        "  {:<24} {:>10} {:>10} {:>10} {:>6} {:>12} {:>10} {:>6} {:>8}",
        "Scenario", "Verdict", "Mean Peg", "Max Peg", "Liqs", "Bad Debt", "Volatility", "Halts", "Breakers"
    );
    println!("  {}", "─".repeat(104));

    let mut pass_count = 0u32;
    let mut soft_count = 0u32;
    let mut hard_count = 0u32;

    for r in &rows {
        let verdict_marker = match r.verdict.as_str() {
            "PASS" => { pass_count += 1; "PASS" },
            "SOFT FAIL" => { soft_count += 1; "SOFT FAIL" },
            "HARD FAIL" => { hard_count += 1; "HARD FAIL" },
            _ => &r.verdict,
        };
        println!(
            "  {:<24} {:>10} {:>9.4}% {:>9.4}% {:>6} {:>12.2} {:>10.4} {:>6} {:>8}",
            r.name,
            verdict_marker,
            r.mean_peg * 100.0,
            r.max_peg * 100.0,
            r.liqs,
            r.bad_debt,
            r.volatility,
            r.halt_blocks,
            r.breaker_triggers,
        );
    }

    println!("  {}", "─".repeat(104));
    println!(
        "  TOTALS: {} PASS / {} SOFT FAIL / {} HARD FAIL out of {} scenarios",
        pass_count,
        soft_count,
        hard_count,
        rows.len()
    );
    println!("═══════════════════════════════════════════════════════════════════════════════════════════════════════════════════");
    println!("\n  Reports saved to: reports/");
    println!("  Master summary : reports/index.html");
    println!("  Individual     : reports/<scenario_name>.html\n");

    // Verify all files exist
    for sid in ScenarioId::all() {
        let path = report_dir.join(format!("{}.html", sid.name()));
        assert!(path.exists(), "Report should exist for {}", sid.name());
    }
    assert!(master_path.exists(), "Master summary should exist");
}
