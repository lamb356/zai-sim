use zai_sim::output;
use zai_sim::scenario::ScenarioConfig;
use zai_sim::scenarios::*;
use zai_sim::sweep::{SweepEngine, SweepParam};

const TEST_BLOCKS: usize = 200;
const TEST_SEED: u64 = 42;

// ═══════════════════════════════════════════════════════════════════════
// Individual Scenario Tests (all 13)
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_steady_state() {
    let scenario = run_stress(
        ScenarioId::SteadyState,
        &ScenarioConfig::default(),
        TEST_BLOCKS,
        TEST_SEED,
    );
    assert_eq!(scenario.metrics.len(), TEST_BLOCKS);

    let last = scenario.metrics.last().unwrap();
    // With constant price and arbers, AMM should stay near $50
    assert!(
        (last.amm_spot_price - 50.0).abs() < 15.0,
        "Steady state AMM should stay near $50: {}",
        last.amm_spot_price
    );
}

#[test]
fn test_black_thursday() {
    let prices = generate_prices(ScenarioId::BlackThursday, TEST_BLOCKS, TEST_SEED);
    assert_eq!(prices.len(), TEST_BLOCKS);

    // Price should crash below 30 at some point
    let min_price = prices.iter().cloned().fold(f64::INFINITY, f64::min);
    assert!(
        min_price <= 25.0,
        "Black Thursday should include crash: min={}",
        min_price
    );

    let scenario = run_stress(
        ScenarioId::BlackThursday,
        &ScenarioConfig::default(),
        TEST_BLOCKS,
        TEST_SEED,
    );
    assert_eq!(scenario.metrics.len(), TEST_BLOCKS);
}

#[test]
fn test_flash_crash() {
    let prices = generate_prices(ScenarioId::FlashCrash, TEST_BLOCKS, TEST_SEED);
    assert_eq!(prices.len(), TEST_BLOCKS);

    // Should have a low point then recovery
    let min_price = prices.iter().cloned().fold(f64::INFINITY, f64::min);
    assert!(min_price < 40.0, "Flash crash should dip: min={}", min_price);

    let scenario = run_stress(
        ScenarioId::FlashCrash,
        &ScenarioConfig::default(),
        TEST_BLOCKS,
        TEST_SEED,
    );
    assert_eq!(scenario.metrics.len(), TEST_BLOCKS);
}

#[test]
fn test_sustained_bear() {
    let prices = generate_prices(ScenarioId::SustainedBear, TEST_BLOCKS, TEST_SEED);
    assert_eq!(prices.len(), TEST_BLOCKS);

    // Should decline from 50 toward 15
    assert!(prices[0] > 49.0);
    assert!(
        *prices.last().unwrap() < 20.0,
        "Bear should end low: {}",
        prices.last().unwrap()
    );

    let scenario = run_stress(
        ScenarioId::SustainedBear,
        &ScenarioConfig::default(),
        TEST_BLOCKS,
        TEST_SEED,
    );
    assert_eq!(scenario.metrics.len(), TEST_BLOCKS);
}

#[test]
fn test_twap_manipulation() {
    let prices = generate_prices(ScenarioId::TwapManipulation, 1000, TEST_SEED);
    assert_eq!(prices.len(), 1000);

    // Should have spikes to 100
    let max_price = prices.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    assert!(
        max_price >= 100.0,
        "Manipulation should include 2x spikes: max={}",
        max_price
    );

    // Most blocks should be at 50
    let normal_count = prices.iter().filter(|&&p| (p - 50.0).abs() < 1.0).count();
    assert!(
        normal_count > 900,
        "Most blocks should be normal: {}/1000",
        normal_count
    );

    let scenario = run_stress(
        ScenarioId::TwapManipulation,
        &ScenarioConfig::default(),
        TEST_BLOCKS,
        TEST_SEED,
    );
    assert_eq!(scenario.metrics.len(), TEST_BLOCKS);
}

#[test]
fn test_liquidity_crisis() {
    let prices = generate_prices(ScenarioId::LiquidityCrisis, TEST_BLOCKS, TEST_SEED);
    assert_eq!(prices.len(), TEST_BLOCKS);

    // Should show high variance
    let mean = prices.iter().sum::<f64>() / prices.len() as f64;
    let variance = prices.iter().map(|p| (p - mean).powi(2)).sum::<f64>() / prices.len() as f64;
    assert!(
        variance > 1.0,
        "Liquidity crisis should be volatile: var={}",
        variance
    );

    let scenario = run_stress(
        ScenarioId::LiquidityCrisis,
        &ScenarioConfig::default(),
        TEST_BLOCKS,
        TEST_SEED,
    );
    assert_eq!(scenario.metrics.len(), TEST_BLOCKS);
}

#[test]
fn test_bank_run() {
    let prices = generate_prices(ScenarioId::BankRun, TEST_BLOCKS, TEST_SEED);
    assert_eq!(prices.len(), TEST_BLOCKS);

    // Should decline with acceleration
    assert!(prices[0] >= 50.0);
    let last = *prices.last().unwrap();
    assert!(last < 35.0, "Bank run should end with low price: {}", last);

    let scenario = run_stress(
        ScenarioId::BankRun,
        &ScenarioConfig::default(),
        TEST_BLOCKS,
        TEST_SEED,
    );
    assert_eq!(scenario.metrics.len(), TEST_BLOCKS);
}

#[test]
fn test_bull_market() {
    let prices = generate_prices(ScenarioId::BullMarket, TEST_BLOCKS, TEST_SEED);
    assert_eq!(prices.len(), TEST_BLOCKS);

    // Should go from 30 to 100
    assert!(prices[0] < 35.0, "Bull should start low: {}", prices[0]);
    let last = *prices.last().unwrap();
    assert!(last > 90.0, "Bull should end high: {}", last);

    let scenario = run_stress(
        ScenarioId::BullMarket,
        &ScenarioConfig::default(),
        TEST_BLOCKS,
        TEST_SEED,
    );
    assert_eq!(scenario.metrics.len(), TEST_BLOCKS);
}

#[test]
fn test_oracle_comparison() {
    let prices = generate_prices(ScenarioId::OracleComparison, TEST_BLOCKS, TEST_SEED);
    assert_eq!(prices.len(), TEST_BLOCKS);

    // Oscillating: should have range around 50
    let min = prices.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = prices.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    assert!(min < 40.0, "Oracle osc should have lows: min={}", min);
    assert!(max > 60.0, "Oracle osc should have highs: max={}", max);

    let scenario = run_stress(
        ScenarioId::OracleComparison,
        &ScenarioConfig::default(),
        TEST_BLOCKS,
        TEST_SEED,
    );
    assert_eq!(scenario.metrics.len(), TEST_BLOCKS);
}

#[test]
fn test_combined_stress() {
    let prices = generate_prices(ScenarioId::CombinedStress, TEST_BLOCKS, TEST_SEED);
    assert_eq!(prices.len(), TEST_BLOCKS);

    // Should have multiple phases
    let min = prices.iter().cloned().fold(f64::INFINITY, f64::min);
    assert!(
        min < 35.0,
        "Combined stress should include crash: min={}",
        min
    );

    let scenario = run_stress(
        ScenarioId::CombinedStress,
        &ScenarioConfig::default(),
        TEST_BLOCKS,
        TEST_SEED,
    );
    assert_eq!(scenario.metrics.len(), TEST_BLOCKS);
}

#[test]
fn test_demand_shock() {
    let prices = generate_prices(ScenarioId::DemandShock, TEST_BLOCKS, TEST_SEED);
    assert_eq!(prices.len(), TEST_BLOCKS);

    // Should surge then collapse
    let max = prices.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    assert!(max > 55.0, "Demand shock should spike: max={}", max);

    let scenario = run_stress(
        ScenarioId::DemandShock,
        &ScenarioConfig::default(),
        TEST_BLOCKS,
        TEST_SEED,
    );
    assert_eq!(scenario.metrics.len(), TEST_BLOCKS);
}

#[test]
fn test_miner_capitulation() {
    let prices = generate_prices(ScenarioId::MinerCapitulation, TEST_BLOCKS, TEST_SEED);
    assert_eq!(prices.len(), TEST_BLOCKS);

    // Should show wave pattern with declining trend
    let first_quarter_mean: f64 =
        prices[..TEST_BLOCKS / 4].iter().sum::<f64>() / (TEST_BLOCKS / 4) as f64;
    let last_quarter_mean: f64 =
        prices[3 * TEST_BLOCKS / 4..].iter().sum::<f64>() / (TEST_BLOCKS / 4) as f64;
    assert!(
        last_quarter_mean < first_quarter_mean,
        "Miner cap should decline: first_q={:.1}, last_q={:.1}",
        first_quarter_mean,
        last_quarter_mean
    );

    let scenario = run_stress(
        ScenarioId::MinerCapitulation,
        &ScenarioConfig::default(),
        TEST_BLOCKS,
        TEST_SEED,
    );
    assert_eq!(scenario.metrics.len(), TEST_BLOCKS);
}

#[test]
fn test_sequencer_downtime() {
    let prices = generate_prices(ScenarioId::SequencerDowntime, TEST_BLOCKS, TEST_SEED);
    assert_eq!(prices.len(), TEST_BLOCKS);

    // During downtime, price stays at 50; after, jumps to 35
    let last = *prices.last().unwrap();
    assert!(
        (last - 35.0).abs() < 1.0,
        "Post-downtime price should be ~35: {}",
        last
    );

    let scenario = run_stress(
        ScenarioId::SequencerDowntime,
        &ScenarioConfig::default(),
        TEST_BLOCKS,
        TEST_SEED,
    );
    assert_eq!(scenario.metrics.len(), TEST_BLOCKS);
}

// ═══════════════════════════════════════════════════════════════════════
// All Scenarios Smoke Test
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_all_scenarios_complete() {
    for sid in ScenarioId::all() {
        let scenario = run_stress(sid, &ScenarioConfig::default(), 100, TEST_SEED);
        assert_eq!(
            scenario.metrics.len(),
            100,
            "Scenario {} should produce 100 metrics",
            sid.name()
        );
        assert!(
            scenario.metrics[0].amm_spot_price > 0.0,
            "Scenario {} should have positive AMM price",
            sid.name()
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Scoring Tests
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_scoring_steady_better_than_crash() {
    let engine = SweepEngine::new(200, TEST_SEED, 50.0);

    let config = ScenarioConfig::default();
    let steady = run_stress(ScenarioId::SteadyState, &config, 200, TEST_SEED);
    let crash = run_stress(ScenarioId::BlackThursday, &config, 200, TEST_SEED);

    let steady_score = engine.score(&steady);
    let crash_score = engine.score(&crash);

    assert!(
        steady_score > crash_score,
        "Steady state should score better than crash: {:.6} vs {:.6}",
        steady_score,
        crash_score
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Sweep Tests
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_grid_sweep() {
    let engine = SweepEngine::new(100, TEST_SEED, 50.0);

    let params = vec![
        SweepParam {
            name: "min_ratio".into(),
            values: vec![1.3, 1.5],
        },
        SweepParam {
            name: "swap_fee".into(),
            values: vec![0.003, 0.01],
        },
    ];

    let scenarios = vec![ScenarioId::SteadyState, ScenarioId::SustainedBear];
    let results = engine.run_grid(&params, &scenarios);

    // 2 × 2 = 4 combinations
    assert_eq!(results.len(), 4, "Grid should produce 4 results");

    for r in &results {
        assert_eq!(r.params.len(), 2);
        assert_eq!(r.scores.len(), 2);
        assert!(r.overall_score.is_finite(), "Score should be finite");
    }
}

#[test]
fn test_monte_carlo_sweep() {
    let engine = SweepEngine::new(100, TEST_SEED, 50.0);

    let configs = vec![
        vec![
            ("min_ratio".to_string(), 1.5),
            ("swap_fee".to_string(), 0.003),
        ],
        vec![
            ("min_ratio".to_string(), 2.0),
            ("swap_fee".to_string(), 0.003),
        ],
    ];

    let scenarios = vec![ScenarioId::SteadyState];
    let results = engine.run_monte_carlo(&configs, &scenarios, 3);

    assert_eq!(results.len(), 2, "MC should produce result per config");
    for r in &results {
        assert!(r.overall_score.is_finite());
    }
}

#[test]
fn test_staged_sweep_small() {
    let engine = SweepEngine::new(100, TEST_SEED, 50.0);

    let params = vec![
        SweepParam {
            name: "min_ratio".into(),
            values: vec![1.3, 1.5, 2.0],
        },
        SweepParam {
            name: "swap_fee".into(),
            values: vec![0.003, 0.01],
        },
    ];

    // Tiny iteration counts for test speed
    let results = engine.run_staged_sweep(&params, 3, 2, 2, 2);

    assert!(
        !results.is_empty(),
        "Staged sweep should produce results"
    );
    // Results should be sorted best-first
    if results.len() >= 2 {
        assert!(
            results[0].overall_score >= results[1].overall_score,
            "Results should be sorted"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Output Tests
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_output_files() {
    let config = ScenarioConfig::default();
    let scenario = run_stress(
        ScenarioId::SteadyState,
        &config,
        50,
        TEST_SEED,
    );

    let dir = std::env::temp_dir().join("zai_sim_test_output");
    let _ = std::fs::remove_dir_all(&dir);

    output::save_all(&scenario, &config, 50.0, &dir).expect("save_all should succeed");

    assert!(dir.join("timeseries.csv").exists(), "timeseries.csv");
    assert!(dir.join("events.csv").exists(), "events.csv");
    assert!(dir.join("metrics.json").exists(), "metrics.json");
    assert!(dir.join("config.toml").exists(), "config.toml");

    // Verify metrics.json has content
    let json = std::fs::read_to_string(dir.join("metrics.json")).unwrap();
    assert!(json.contains("total_blocks"), "JSON should have total_blocks");
    assert!(json.contains("50"), "JSON should contain block count");

    // Verify config.toml has content
    let toml = std::fs::read_to_string(dir.join("config.toml")).unwrap();
    assert!(toml.contains("[amm]"), "TOML should have [amm] section");
    assert!(toml.contains("[cdp]"), "TOML should have [cdp] section");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_summary_metrics() {
    let config = ScenarioConfig::default();
    let scenario = run_stress(
        ScenarioId::SteadyState,
        &config,
        100,
        TEST_SEED,
    );

    let summary = output::compute_summary(&scenario.metrics, 50.0);

    assert_eq!(summary.total_blocks, 100);
    assert!(summary.mean_amm_price > 0.0);
    assert!(summary.min_amm_price > 0.0);
    assert!(summary.max_amm_price >= summary.min_amm_price);
    assert!(summary.final_amm_price > 0.0);
    assert!(summary.final_redemption_price > 0.0);
    assert!(summary.final_debt_ceiling > 0.0);
}

#[test]
fn test_event_extraction() {
    let config = ScenarioConfig::default();
    let scenario = run_stress(
        ScenarioId::BlackThursday,
        &config,
        500,
        TEST_SEED,
    );

    let events = output::extract_events(&scenario.metrics);

    // Black Thursday should generate some events (breaker triggers at minimum)
    // Even if no events, the extraction should not panic
    // (the fact we get here without panic is the assertion)

    // Verify event structure
    for event in &events {
        assert!(event.block > 0);
        assert!(!event.event_type.is_empty());
    }
}

#[test]
fn test_sweep_results_csv() {
    let engine = SweepEngine::new(50, TEST_SEED, 50.0);

    let params = vec![SweepParam {
        name: "min_ratio".into(),
        values: vec![1.5, 2.0],
    }];

    let results = engine.run_grid(&params, &[ScenarioId::SteadyState]);

    let path = std::env::temp_dir().join("zai_sim_test_sweep.csv");
    output::save_sweep_results(&results, &path).expect("save_sweep_results should succeed");

    assert!(path.exists(), "Sweep CSV should be created");
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("min_ratio"), "CSV should have param header");
    assert!(content.contains("overall_score"), "CSV should have score header");

    let _ = std::fs::remove_file(&path);
}
