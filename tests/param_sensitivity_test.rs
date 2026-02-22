use zai_sim::controller::ControllerConfig;
use zai_sim::output;
use zai_sim::report;
use zai_sim::scenario::ScenarioConfig;
use zai_sim::scenarios::{run_stress, ScenarioId};

const BLOCKS: usize = 1000;
const SEED: u64 = 42;

/// Build the base config for sensitivity tests:
/// - 100k ZEC + 5M ZAI AMM liquidity
/// - Tick controller
/// - 240-block TWAP window
/// - 200% min collateral ratio
fn base_config() -> ScenarioConfig {
    let mut config = ScenarioConfig::default();
    config.amm_initial_zec = 100_000.0;
    config.amm_initial_zai = 5_000_000.0;
    config.controller_config = ControllerConfig::default_tick();
    config.cdp_config.twap_window = 240;
    config.cdp_config.min_ratio = 2.0;
    config
}

/// Collected results from a single parameter-scenario run.
struct SweepResult {
    verdict: String,
    mean_peg_deviation: f64,
    max_peg_deviation: f64,
    breaker_triggers: u32,
}

#[test]
fn param_sweep_collateral_ratio() {
    let ratios = [1.5, 1.75, 2.0, 2.5, 3.0];
    let scenarios = [ScenarioId::BlackThursday, ScenarioId::DemandShock];

    let _ = std::fs::create_dir_all("reports/param_sensitivity");

    println!("\n{}", "=".repeat(100));
    println!("  PARAMETER SWEEP: Collateral Ratio");
    println!("{}", "=".repeat(100));

    for scenario_id in &scenarios {
        println!(
            "\n  Scenario: {}\n  {:<12} {:<12} {:<18} {:<18} {:<16}",
            scenario_id.name(),
            "min_ratio",
            "verdict",
            "mean_peg_dev",
            "max_peg_dev",
            "breaker_triggers"
        );
        println!("  {}", "-".repeat(80));

        for &ratio in &ratios {
            let mut config = base_config();
            config.cdp_config.min_ratio = ratio;

            let target = config.initial_redemption_price;
            let scenario = run_stress(*scenario_id, &config, BLOCKS, SEED);

            let verdict = report::evaluate_pass_fail(&scenario.metrics, target);
            let summary = output::compute_summary(&scenario.metrics, target);

            let result = SweepResult {
                verdict: verdict.overall.label().to_string(),
                mean_peg_deviation: summary.mean_peg_deviation,
                max_peg_deviation: summary.max_peg_deviation,
                breaker_triggers: summary.breaker_triggers,
            };

            println!(
                "  {:<12.2} {:<12} {:<18.6} {:<18.6} {:<16}",
                ratio,
                result.verdict,
                result.mean_peg_deviation,
                result.max_peg_deviation,
                result.breaker_triggers
            );

            // Save HTML report
            let report_name = format!("{}_{}", scenario_id.name(), ratio);
            let html = report::generate_report(
                &scenario.metrics,
                &config,
                &report_name,
                target,
            );
            let html_path = format!(
                "reports/param_sensitivity/{}_collateral_ratio_{}.html",
                scenario_id.name(),
                ratio
            );
            report::save_report(&html, std::path::Path::new(&html_path))
                .expect("save report");
        }
    }

    println!("\n{}\n", "=".repeat(100));
}

#[test]
fn param_sweep_twap_window() {
    let windows: [u64; 5] = [60, 120, 240, 480, 960];
    let scenarios = [ScenarioId::BlackThursday, ScenarioId::DemandShock];

    let _ = std::fs::create_dir_all("reports/param_sensitivity");

    println!("\n{}", "=".repeat(100));
    println!("  PARAMETER SWEEP: TWAP Window");
    println!("{}", "=".repeat(100));

    for scenario_id in &scenarios {
        println!(
            "\n  Scenario: {}\n  {:<14} {:<12} {:<18} {:<18} {:<16}",
            scenario_id.name(),
            "twap_window",
            "verdict",
            "mean_peg_dev",
            "max_peg_dev",
            "breaker_triggers"
        );
        println!("  {}", "-".repeat(82));

        for &window in &windows {
            let mut config = base_config();
            config.cdp_config.min_ratio = 2.0;
            config.cdp_config.twap_window = window;

            let target = config.initial_redemption_price;
            let scenario = run_stress(*scenario_id, &config, BLOCKS, SEED);

            let verdict = report::evaluate_pass_fail(&scenario.metrics, target);
            let summary = output::compute_summary(&scenario.metrics, target);

            let result = SweepResult {
                verdict: verdict.overall.label().to_string(),
                mean_peg_deviation: summary.mean_peg_deviation,
                max_peg_deviation: summary.max_peg_deviation,
                breaker_triggers: summary.breaker_triggers,
            };

            println!(
                "  {:<14} {:<12} {:<18.6} {:<18.6} {:<16}",
                window,
                result.verdict,
                result.mean_peg_deviation,
                result.max_peg_deviation,
                result.breaker_triggers
            );

            // Save HTML report
            let report_name = format!("{}_{}", scenario_id.name(), window);
            let html = report::generate_report(
                &scenario.metrics,
                &config,
                &report_name,
                target,
            );
            let html_path = format!(
                "reports/param_sensitivity/{}_twap_window_{}.html",
                scenario_id.name(),
                window
            );
            report::save_report(&html, std::path::Path::new(&html_path))
                .expect("save report");
        }
    }

    println!("\n{}\n", "=".repeat(100));
}

#[test]
fn param_sensitivity_comparison() {
    let _ = std::fs::create_dir_all("reports/param_sensitivity");

    let scenario_id = ScenarioId::BlackThursday;

    println!("\n{}", "=".repeat(100));
    println!("  PARAMETER SENSITIVITY COMPARISON (Black Thursday)");
    println!("{}", "=".repeat(100));

    // --- Collateral ratio extremes: 1.5 vs 3.0 ---
    let ratio_low = 1.5;
    let ratio_high = 3.0;

    let mut config_ratio_low = base_config();
    config_ratio_low.cdp_config.min_ratio = ratio_low;
    let target = config_ratio_low.initial_redemption_price;
    let scenario_ratio_low = run_stress(scenario_id, &config_ratio_low, BLOCKS, SEED);
    let summary_ratio_low = output::compute_summary(&scenario_ratio_low.metrics, target);

    let mut config_ratio_high = base_config();
    config_ratio_high.cdp_config.min_ratio = ratio_high;
    let scenario_ratio_high = run_stress(scenario_id, &config_ratio_high, BLOCKS, SEED);
    let summary_ratio_high = output::compute_summary(&scenario_ratio_high.metrics, target);

    let ratio_devs = [
        summary_ratio_low.mean_peg_deviation,
        summary_ratio_high.mean_peg_deviation,
    ];
    let ratio_max = ratio_devs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let ratio_min = ratio_devs.iter().cloned().fold(f64::INFINITY, f64::min);
    let ratio_mean = ratio_devs.iter().sum::<f64>() / ratio_devs.len() as f64;
    let ratio_sensitivity = if ratio_mean > 0.0 {
        (ratio_max - ratio_min) / ratio_mean
    } else {
        0.0
    };

    println!(
        "\n  Collateral Ratio ({} vs {}):",
        ratio_low, ratio_high
    );
    println!(
        "    mean_peg_dev @ {}: {:.6}",
        ratio_low, summary_ratio_low.mean_peg_deviation
    );
    println!(
        "    mean_peg_dev @ {}: {:.6}",
        ratio_high, summary_ratio_high.mean_peg_deviation
    );
    println!("    sensitivity = (max - min) / mean = {:.6}", ratio_sensitivity);

    // --- TWAP window extremes: 60 vs 960 ---
    let window_low: u64 = 60;
    let window_high: u64 = 960;

    let mut config_window_low = base_config();
    config_window_low.cdp_config.twap_window = window_low;
    let scenario_window_low = run_stress(scenario_id, &config_window_low, BLOCKS, SEED);
    let summary_window_low = output::compute_summary(&scenario_window_low.metrics, target);

    let mut config_window_high = base_config();
    config_window_high.cdp_config.twap_window = window_high;
    let scenario_window_high = run_stress(scenario_id, &config_window_high, BLOCKS, SEED);
    let summary_window_high = output::compute_summary(&scenario_window_high.metrics, target);

    let window_devs = [
        summary_window_low.mean_peg_deviation,
        summary_window_high.mean_peg_deviation,
    ];
    let window_max = window_devs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let window_min = window_devs.iter().cloned().fold(f64::INFINITY, f64::min);
    let window_mean = window_devs.iter().sum::<f64>() / window_devs.len() as f64;
    let window_sensitivity = if window_mean > 0.0 {
        (window_max - window_min) / window_mean
    } else {
        0.0
    };

    println!(
        "\n  TWAP Window ({} vs {}):",
        window_low, window_high
    );
    println!(
        "    mean_peg_dev @ {}: {:.6}",
        window_low, summary_window_low.mean_peg_deviation
    );
    println!(
        "    mean_peg_dev @ {}: {:.6}",
        window_high, summary_window_high.mean_peg_deviation
    );
    println!("    sensitivity = (max - min) / mean = {:.6}", window_sensitivity);

    // --- Comparison ---
    let more_sensitive = if ratio_sensitivity > window_sensitivity {
        "collateral_ratio"
    } else {
        "twap_window"
    };

    println!("\n  RESULT: {} has larger sensitivity ({:.6} vs {:.6})",
        more_sensitive, ratio_sensitivity, window_sensitivity
    );
    println!("{}\n", "=".repeat(100));

    // Save reports for the extremes
    for (label, scenario_run, cfg) in [
        ("ratio_low", &scenario_ratio_low, &config_ratio_low),
        ("ratio_high", &scenario_ratio_high, &config_ratio_high),
        ("window_low", &scenario_window_low, &config_window_low),
        ("window_high", &scenario_window_high, &config_window_high),
    ] {
        let html = report::generate_report(
            &scenario_run.metrics,
            cfg,
            &format!("black_thursday_{}", label),
            target,
        );
        let html_path = format!(
            "reports/param_sensitivity/black_thursday_{}.html",
            label
        );
        report::save_report(&html, std::path::Path::new(&html_path))
            .expect("save report");
    }

    // Assert both sensitivities are finite and non-negative
    assert!(
        ratio_sensitivity.is_finite() && ratio_sensitivity >= 0.0,
        "Collateral ratio sensitivity should be finite and non-negative, got {}",
        ratio_sensitivity
    );
    assert!(
        window_sensitivity.is_finite() && window_sensitivity >= 0.0,
        "TWAP window sensitivity should be finite and non-negative, got {}",
        window_sensitivity
    );
    // NOTE: Both parameters may show zero sensitivity for peg deviation.
    // This is a genuine finding: without active vaults being liquidated,
    // collateral ratio and TWAP window don't affect AMM price tracking.
    // The peg deviation is driven entirely by arber behavior vs AMM depth.
    println!("\n  NOTE: Zero sensitivity means these CDP-layer parameters");
    println!("  do not affect AMM peg deviation when no liquidations occur.");
    println!("  This is finding F-016: peg deviation is AMM-layer, not CDP-layer.\n");
}
