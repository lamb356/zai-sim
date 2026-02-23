use zai_sim::agents::*;
use zai_sim::historical::{config_for_historical, interpolate_to_blocks, load_hourly_prices};
use zai_sim::output;
use zai_sim::report;
use zai_sim::scenario::Scenario;

use std::path::PathBuf;

const BLOCKS_PER_HOUR: usize = 48;

struct HistoricalScenario {
    name: &'static str,
    csv_path: &'static str,
    expected_start_price: f64,
}

fn add_historical_agents(scenario: &mut Scenario) {
    scenario
        .arbers
        .push(Arbitrageur::new(ArbitrageurConfig::default()));
    scenario
        .miners
        .push(MinerAgent::new(MinerAgentConfig::default()));
    scenario
        .demand_agents
        .push(DemandAgent::new(DemandAgentConfig::default()));
    scenario
        .cdp_holders
        .push(CdpHolder::new(CdpHolderConfig::default()));
}

#[test]
fn historical_replay_suite() {
    let scenarios = vec![
        HistoricalScenario {
            name: "black_thursday_2020",
            csv_path: "data/black_thursday_2020_hourly.csv",
            expected_start_price: 42.0,
        },
        HistoricalScenario {
            name: "ftx_collapse_2022",
            csv_path: "data/ftx_collapse_2022_hourly.csv",
            expected_start_price: 53.0,
        },
        HistoricalScenario {
            name: "rally_2024",
            csv_path: "data/rally_2024_hourly.csv",
            expected_start_price: 37.0,
        },
    ];

    let report_dir = PathBuf::from("reports/historical_replay");
    let _ = std::fs::create_dir_all(&report_dir);

    struct Row {
        name: String,
        verdict: String,
        mean_peg: f64,
        max_peg: f64,
        liqs: u32,
        bad_debt: f64,
        breaker_triggers: u32,
        final_amm_price: f64,
    }

    let mut rows: Vec<Row> = Vec::new();
    let mut entries = Vec::new();

    println!("\n  Running historical replay test suite...");
    println!("  Config: 100K ZEC AMM (price-matched), 200% CR, Tick controller, 240-block TWAP\n");

    for hs in &scenarios {
        println!("  Loading {}...", hs.name);

        // Load hourly prices from CSV
        let hourly = load_hourly_prices(hs.csv_path);
        let first_price = hourly[0];

        // Sanity check: first price should be within 20% of expected
        let deviation = (first_price - hs.expected_start_price).abs() / hs.expected_start_price;
        assert!(
            deviation < 0.20,
            "{}: first price {:.2} deviates {:.0}% from expected {:.2}",
            hs.name,
            first_price,
            deviation * 100.0,
            hs.expected_start_price,
        );

        // Interpolate to per-block prices
        let block_prices = interpolate_to_blocks(&hourly, BLOCKS_PER_HOUR);
        let total_blocks = block_prices.len();

        println!(
            "    {} hourly prices -> {} blocks ({:.1} hours)",
            hourly.len(),
            total_blocks,
            total_blocks as f64 / BLOCKS_PER_HOUR as f64,
        );
        println!(
            "    Price range: ${:.2} -> ${:.2} (start ${:.2})",
            hourly.iter().cloned().fold(f64::INFINITY, f64::min),
            hourly.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
            first_price,
        );

        // Create config with AMM starting price matching CSV
        let config = config_for_historical(first_price);
        let target = first_price;

        // Build and run scenario
        let mut scenario = Scenario::new_with_seed(&config, 42);
        add_historical_agents(&mut scenario);
        scenario.run(&block_prices);

        // Evaluate and report
        let verdict = report::evaluate_pass_fail(&scenario.metrics, target);
        let summary = output::compute_summary(&scenario.metrics, target);

        let html = report::generate_report(&scenario.metrics, &config, hs.name, target);
        let html_path = report_dir.join(format!("{}.html", hs.name));
        report::save_report(&html, &html_path).expect("save report");

        rows.push(Row {
            name: hs.name.to_string(),
            verdict: verdict.overall.label().to_string(),
            mean_peg: summary.mean_peg_deviation,
            max_peg: summary.max_peg_deviation,
            liqs: summary.total_liquidations,
            bad_debt: summary.total_bad_debt,
            breaker_triggers: summary.breaker_triggers,
            final_amm_price: summary.final_amm_price,
        });

        entries.push((hs.name.to_string(), verdict, summary));
    }

    // Generate master index
    let master_html = report::generate_master_summary(&entries);
    let master_path = report_dir.join("index.html");
    report::save_report(&master_html, &master_path).expect("save master summary");

    // Print summary table
    println!("\n═══════════════════════════════════════════════════════════════════════════════════════════════════════════════════");
    println!("  ZAI SIMULATOR — HISTORICAL REPLAY (Real ZEC Price Data)");
    println!("═══════════════════════════════════════════════════════════════════════════════════════════════════════════════════");
    println!(
        "  {:<24} {:>10} {:>10} {:>10} {:>6} {:>12} {:>10} {:>12}",
        "Scenario", "Verdict", "Mean Peg", "Max Peg", "Liqs", "Bad Debt", "Breakers", "Final AMM"
    );
    println!("  {}", "─".repeat(98));

    for r in &rows {
        println!(
            "  {:<24} {:>10} {:>9.4}% {:>9.4}% {:>6} {:>12.2} {:>10} {:>11.2}",
            r.name,
            r.verdict,
            r.mean_peg * 100.0,
            r.max_peg * 100.0,
            r.liqs,
            r.bad_debt,
            r.breaker_triggers,
            r.final_amm_price,
        );
    }

    println!("  {}", "─".repeat(98));
    println!("═══════════════════════════════════════════════════════════════════════════════════════════════════════════════════");
    println!("\n  Reports saved to: reports/historical_replay/");
    println!("  Master summary:   reports/historical_replay/index.html\n");

    // Verify all reports exist
    for hs in &scenarios {
        let path = report_dir.join(format!("{}.html", hs.name));
        assert!(path.exists(), "Report should exist for {}", hs.name);
    }
    assert!(master_path.exists(), "Master summary should exist");
}
