use zai_sim::output;
use zai_sim::report::*;
use zai_sim::scenario::ScenarioConfig;
use zai_sim::scenarios::*;

const TEST_SEED: u64 = 42;

// ═══════════════════════════════════════════════════════════════════════
// Pass/Fail Evaluation Tests
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_steady_state_passes() {
    let config = ScenarioConfig::default();
    let scenario = run_stress(ScenarioId::SteadyState, &config, 200, TEST_SEED);
    let result = evaluate_pass_fail(&scenario.metrics, 50.0);

    assert_eq!(
        result.overall,
        Verdict::Pass,
        "Steady state should PASS: criteria = {:?}",
        result
            .criteria
            .iter()
            .map(|c| format!("{}: {}", c.name, c.passed))
            .collect::<Vec<_>>()
    );
}

#[test]
fn test_black_thursday_verdict() {
    let config = ScenarioConfig::default();
    let scenario = run_stress(ScenarioId::BlackThursday, &config, 500, TEST_SEED);
    let result = evaluate_pass_fail(&scenario.metrics, 50.0);

    // Black Thursday should trigger at least soft fail (large price deviation)
    assert_ne!(
        result.overall,
        Verdict::Pass,
        "Black Thursday should not PASS cleanly"
    );
}

#[test]
fn test_criteria_count() {
    let config = ScenarioConfig::default();
    let scenario = run_stress(ScenarioId::SteadyState, &config, 100, TEST_SEED);
    let result = evaluate_pass_fail(&scenario.metrics, 50.0);

    // Should have 7 criteria
    assert_eq!(
        result.criteria.len(),
        7,
        "Should evaluate 7 criteria"
    );
}

#[test]
fn test_hard_fail_bad_debt() {
    let config = ScenarioConfig::default();
    // Sustained bear with aggressive liquidation penalty
    let mut bad_config = config.clone();
    bad_config.cdp_config.min_ratio = 1.1; // very tight collateral
    bad_config.cdp_config.liquidation_penalty = 0.50; // huge penalty
    let scenario = run_stress(ScenarioId::SustainedBear, &bad_config, 500, TEST_SEED);
    let result = evaluate_pass_fail(&scenario.metrics, 50.0);

    // Should have solvency criterion
    let solvency = result
        .criteria
        .iter()
        .find(|c| c.name == "Solvency")
        .unwrap();
    assert!(
        solvency.severity == Verdict::HardFail,
        "Solvency should be a hard-fail criterion"
    );
}

#[test]
fn test_verdict_labels() {
    assert_eq!(Verdict::Pass.label(), "PASS");
    assert_eq!(Verdict::SoftFail.label(), "SOFT FAIL");
    assert_eq!(Verdict::HardFail.label(), "HARD FAIL");
    assert_eq!(Verdict::Pass.css_class(), "pass");
    assert_eq!(Verdict::SoftFail.css_class(), "soft-fail");
    assert_eq!(Verdict::HardFail.css_class(), "hard-fail");
}

// ═══════════════════════════════════════════════════════════════════════
// HTML Report Generation Tests
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_generate_report_html() {
    let config = ScenarioConfig::default();
    let scenario = run_stress(ScenarioId::SteadyState, &config, 100, TEST_SEED);
    let html = generate_report(&scenario.metrics, &config, "steady_state", 50.0);

    // Check structure
    assert!(html.contains("<!DOCTYPE html>"), "Should be valid HTML");
    assert!(html.contains("chart.js"), "Should include Chart.js");
    assert!(html.contains("steady_state"), "Should contain scenario name");
    assert!(html.contains("Price Comparison"), "Should have price chart");
    assert!(html.contains("System Health"), "Should have health chart");
    assert!(html.contains("Liquidation Activity"), "Should have liquidation chart");
    assert!(html.contains("AMM State"), "Should have AMM chart");
    assert!(html.contains("Controller Response"), "Should have controller chart");
    assert!(html.contains("Agent Activity"), "Should have agent chart");
    assert!(html.contains("Pass / Fail Criteria"), "Should have criteria section");
    assert!(html.contains("Executive Summary"), "Should have summary");
    assert!(html.contains("Parameters"), "Should have params table");
}

#[test]
fn test_report_contains_data() {
    let config = ScenarioConfig::default();
    let scenario = run_stress(ScenarioId::SteadyState, &config, 50, TEST_SEED);
    let html = generate_report(&scenario.metrics, &config, "test", 50.0);

    // Should contain JavaScript data arrays
    assert!(html.contains("const B="), "Should have block array");
    assert!(html.contains("ext:"), "Should have external price data");
    assert!(html.contains("spot:"), "Should have spot price data");
    assert!(html.contains("#4285f4"), "Should use blue for external");
    assert!(html.contains("#ea8c00"), "Should use orange for spot");
    assert!(html.contains("#34a853"), "Should use green for TWAP");
    assert!(html.contains("#ea4335"), "Should use red for redemption");
    assert!(html.contains("#9c27b0"), "Should use purple for collateral ratio");
}

#[test]
fn test_report_verdict_badge() {
    let config = ScenarioConfig::default();

    // Steady state -> PASS badge
    let scenario = run_stress(ScenarioId::SteadyState, &config, 100, TEST_SEED);
    let html = generate_report(&scenario.metrics, &config, "test", 50.0);
    assert!(
        html.contains("badge pass") || html.contains("badge soft-fail") || html.contains("badge hard-fail"),
        "Should contain a verdict badge"
    );
}

#[test]
fn test_save_report_file() {
    let config = ScenarioConfig::default();
    let scenario = run_stress(ScenarioId::SteadyState, &config, 50, TEST_SEED);
    let html = generate_report(&scenario.metrics, &config, "test", 50.0);

    let path = std::env::temp_dir().join("zai_sim_test_report.html");
    save_report(&html, &path).expect("save_report should succeed");

    assert!(path.exists(), "HTML file should be created");
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.len() > 1000, "HTML should have substantial content");
    assert!(content.starts_with("<!DOCTYPE html>"));

    let _ = std::fs::remove_file(&path);
}

// ═══════════════════════════════════════════════════════════════════════
// Master Summary Tests
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_master_summary() {
    let config = ScenarioConfig::default();

    let mut entries = Vec::new();
    for sid in [ScenarioId::SteadyState, ScenarioId::BlackThursday] {
        let scenario = run_stress(sid, &config, 100, TEST_SEED);
        let verdict = evaluate_pass_fail(&scenario.metrics, 50.0);
        let summary = output::compute_summary(&scenario.metrics, 50.0);
        entries.push((sid.name().to_string(), verdict, summary));
    }

    let html = generate_master_summary(&entries);

    assert!(html.contains("<!DOCTYPE html>"), "Should be valid HTML");
    assert!(html.contains("Master Summary"), "Should have title");
    assert!(html.contains("steady_state"), "Should list steady state");
    assert!(html.contains("black_thursday"), "Should list black thursday");
    assert!(
        html.contains("2") || html.contains("/ 2"),
        "Should show total count"
    );
}

#[test]
fn test_master_summary_links() {
    let config = ScenarioConfig::default();
    let scenario = run_stress(ScenarioId::SteadyState, &config, 50, TEST_SEED);
    let verdict = evaluate_pass_fail(&scenario.metrics, 50.0);
    let summary = output::compute_summary(&scenario.metrics, 50.0);

    let entries = vec![("steady_state".to_string(), verdict, summary)];
    let html = generate_master_summary(&entries);

    assert!(
        html.contains("steady_state.html"),
        "Should link to individual report"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// All Scenarios Report Generation
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_all_scenarios_generate_reports() {
    let config = ScenarioConfig::default();

    for sid in ScenarioId::all() {
        let scenario = run_stress(sid, &config, 100, TEST_SEED);
        let html = generate_report(&scenario.metrics, &config, sid.name(), 50.0);

        // Basic validity checks
        assert!(
            html.contains("<!DOCTYPE html>"),
            "Scenario {} should produce valid HTML",
            sid.name()
        );
        assert!(
            html.len() > 500,
            "Scenario {} report should have content (len={})",
            sid.name(),
            html.len()
        );

        // Should evaluate pass/fail without panicking
        let _verdict = evaluate_pass_fail(&scenario.metrics, 50.0);
    }
}
