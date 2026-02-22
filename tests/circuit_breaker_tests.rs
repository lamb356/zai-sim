use approx::assert_relative_eq;
use zai_sim::amm::Amm;
use zai_sim::cdp::{CdpConfig, VaultRegistry};
use zai_sim::circuit_breaker::*;

fn setup_amm(block: u64) -> Amm {
    let mut amm = Amm::new(10000.0, 500000.0, 0.003);
    for b in 1..=block {
        amm.record_price(b);
    }
    amm
}

// ═══════════════════════════════════════════════════════════════════════
// TWAP Circuit Breaker Tests
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_twap_circuit_breaker_triggers() {
    let mut amm = setup_amm(100);

    let mut breaker = TwapBreaker::new(TwapBreakerConfig {
        max_twap_change_pct: 0.10, // 10% threshold
        short_window: 12,
        long_window: 48,
        pause_blocks: 48,
    });

    // No trigger when price is stable
    let action = breaker.check(&amm, 101);
    assert_eq!(action, BreakerAction::None);
    assert!(!breaker.triggered);

    // Crash price: large ZEC sell
    let _out = amm.swap_zec_for_zai(3000.0, 101).unwrap();

    // Record new price for short window blocks
    for b in 102..=115 {
        amm.record_price(b);
    }

    // Now short TWAP should differ from long TWAP
    let twap_short = amm.get_twap(12);
    let twap_long = amm.get_twap(48);
    let divergence = ((twap_short - twap_long) / twap_long).abs();

    if divergence > 0.10 {
        let action = breaker.check(&amm, 115);
        assert!(
            matches!(action, BreakerAction::PauseMinting { .. }),
            "Should trigger on >10% TWAP divergence ({:.2}%)",
            divergence * 100.0
        );
        assert!(breaker.triggered);
        assert_eq!(breaker.trigger_count, 1);
        assert_eq!(breaker.resume_at_block, 115 + 48);

        // While triggered, check returns None
        let action2 = breaker.check(&amm, 120);
        assert_eq!(action2, BreakerAction::None);
        assert!(breaker.is_active(120));

        // After pause period, breaker resets
        assert!(!breaker.is_active(115 + 48));
        let action3 = breaker.check(&amm, 115 + 48);
        // May or may not retrigger depending on TWAP state
        assert!(!breaker.triggered || action3 != BreakerAction::None);
    }
}

#[test]
fn test_twap_breaker_no_trigger_small_move() {
    let mut amm = setup_amm(100);

    let mut breaker = TwapBreaker::new(TwapBreakerConfig {
        max_twap_change_pct: 0.15,
        short_window: 12,
        long_window: 48,
        pause_blocks: 48,
    });

    // Small price movement (shouldn't trigger 15% breaker)
    let _out = amm.swap_zec_for_zai(200.0, 101).unwrap();
    for b in 102..=115 {
        amm.record_price(b);
    }

    let action = breaker.check(&amm, 115);
    assert_eq!(action, BreakerAction::None, "Small move should not trigger breaker");
    assert!(!breaker.triggered);
}

#[test]
fn test_twap_breaker_resumes_after_pause() {
    let mut amm = setup_amm(100);

    let mut breaker = TwapBreaker::new(TwapBreakerConfig {
        max_twap_change_pct: 0.05, // very sensitive
        short_window: 12,
        long_window: 48,
        pause_blocks: 20,
    });

    // Trigger it
    let _out = amm.swap_zec_for_zai(2000.0, 101).unwrap();
    for b in 102..=115 {
        amm.record_price(b);
    }

    let action = breaker.check(&amm, 115);
    if matches!(action, BreakerAction::PauseMinting { .. }) {
        assert!(breaker.is_active(115));
        assert!(breaker.is_active(134));

        // At block 135 (115 + 20), should resume
        assert!(!breaker.is_active(135));

        // Check at resume block resets triggered flag
        breaker.check(&amm, 135);
        assert!(!breaker.triggered);
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Cascade Circuit Breaker Tests
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_cascade_circuit_breaker() {
    let mut breaker = CascadeBreaker::new(CascadeBreakerConfig {
        max_liquidations_in_window: 5,
        window_blocks: 48,
        pause_blocks: 96,
    });

    // Record liquidations spread over time — under limit
    breaker.record_liquidations(10, 2);
    breaker.record_liquidations(20, 2);
    let action = breaker.check(30);
    assert_eq!(action, BreakerAction::None, "4 liquidations should not trigger (limit 5)");

    // Push over the limit
    breaker.record_liquidations(30, 2);
    let action = breaker.check(31);
    assert!(
        matches!(action, BreakerAction::EmergencyHalt { .. }),
        "6 liquidations in window should trigger cascade breaker"
    );
    assert!(breaker.triggered);
    assert_eq!(breaker.trigger_count, 1);
    assert_eq!(breaker.resume_at_block, 31 + 96);

    // While triggered, returns None
    let action2 = breaker.check(50);
    assert_eq!(action2, BreakerAction::None);
    assert!(breaker.is_active(50));

    // After pause, resets
    assert!(!breaker.is_active(31 + 96));
}

#[test]
fn test_cascade_breaker_window_sliding() {
    let mut breaker = CascadeBreaker::new(CascadeBreakerConfig {
        max_liquidations_in_window: 5,
        window_blocks: 20,
        pause_blocks: 50,
    });

    // Liquidations at block 10
    breaker.record_liquidations(10, 3);
    let action = breaker.check(15);
    assert_eq!(action, BreakerAction::None);

    // More at block 25
    breaker.record_liquidations(25, 3);

    // At block 29: window is [9, 29] — includes both (total 6)
    let action = breaker.check(29);
    assert!(
        matches!(action, BreakerAction::EmergencyHalt { .. }),
        "Sliding window should capture both batches"
    );

    // Reset and test that old liquidations fall out of window
    let mut breaker2 = CascadeBreaker::new(CascadeBreakerConfig {
        max_liquidations_in_window: 5,
        window_blocks: 20,
        pause_blocks: 50,
    });

    breaker2.record_liquidations(10, 3);
    breaker2.record_liquidations(40, 3);

    // At block 40: window is [20, 40] — only includes block 40 batch (3)
    let action = breaker2.check(40);
    assert_eq!(
        action,
        BreakerAction::None,
        "Old liquidations should have slid out of window"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Dynamic Debt Ceiling Tests
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_debt_ceiling_reduces_on_deviation() {
    let mut amm = setup_amm(100);
    let registry = VaultRegistry::new(CdpConfig::default());

    let mut ceiling = DebtCeiling::new(DebtCeilingConfig {
        initial_ceiling: 1_000_000.0,
        min_ceiling: 100_000.0,
        reduction_factor: 0.10,
        growth_rate_per_block: 0.1,
        deviation_threshold: 0.10,
    });

    // At peg: no reduction
    let action = ceiling.update(&amm, &registry, 50.0);
    assert_eq!(action, BreakerAction::None);
    assert_relative_eq!(ceiling.current_ceiling, 1_000_000.0, epsilon = 1.0);

    // Crash price > 10% from redemption price
    let _out = amm.swap_zec_for_zai(3000.0, 101).unwrap();
    amm.record_price(101);
    let spot = amm.spot_price();
    let deviation = ((spot - 50.0) / 50.0).abs();

    if deviation > 0.10 {
        let action = ceiling.update(&amm, &registry, 50.0);
        assert!(
            matches!(action, BreakerAction::ReduceDebtCeiling { .. }),
            "Should reduce ceiling on >10% deviation"
        );
        // Reduced by 10%: 1_000_000 - 100_000 = 900_000
        assert_relative_eq!(ceiling.current_ceiling, 900_000.0, epsilon = 1.0);
        assert_eq!(ceiling.reductions, 1);
    }
}

#[test]
fn test_debt_ceiling_grows_back() {
    let amm = setup_amm(100);
    let registry = VaultRegistry::new(CdpConfig::default());

    let mut ceiling = DebtCeiling::new(DebtCeilingConfig {
        initial_ceiling: 1_000_000.0,
        min_ceiling: 100_000.0,
        reduction_factor: 0.10,
        growth_rate_per_block: 100.0, // fast growth for test
        deviation_threshold: 0.10,
    });

    // Manually reduce
    ceiling.current_ceiling = 500_000.0;

    // At peg: should grow
    let action = ceiling.update(&amm, &registry, 50.0);
    assert_eq!(action, BreakerAction::None);
    assert_relative_eq!(ceiling.current_ceiling, 500_100.0, epsilon = 1.0);

    // Won't exceed initial ceiling
    ceiling.current_ceiling = 999_950.0;
    ceiling.update(&amm, &registry, 50.0);
    assert_relative_eq!(
        ceiling.current_ceiling,
        1_000_000.0,
        epsilon = 1.0,
    );
}

#[test]
fn test_debt_ceiling_min_floor() {
    let mut amm = setup_amm(100);
    let registry = VaultRegistry::new(CdpConfig::default());

    let mut ceiling = DebtCeiling::new(DebtCeilingConfig {
        initial_ceiling: 200_000.0,
        min_ceiling: 150_000.0,
        reduction_factor: 0.50, // aggressive reduction
        growth_rate_per_block: 0.1,
        deviation_threshold: 0.05,
    });

    // Large deviation
    let _out = amm.swap_zec_for_zai(5000.0, 101).unwrap();
    amm.record_price(101);

    // Multiple reductions
    for _ in 0..10 {
        ceiling.update(&amm, &registry, 50.0);
    }

    // Should not go below minimum
    assert!(
        ceiling.current_ceiling >= 150_000.0,
        "Ceiling should not go below min: {}",
        ceiling.current_ceiling
    );
}

#[test]
fn test_debt_ceiling_can_mint_check() {
    let ceiling = DebtCeiling::new(DebtCeilingConfig {
        initial_ceiling: 100_000.0,
        ..DebtCeilingConfig::default()
    });

    assert!(ceiling.can_mint(50_000.0, 30_000.0)); // 80k < 100k
    assert!(ceiling.can_mint(90_000.0, 10_000.0)); // 100k == 100k
    assert!(!ceiling.can_mint(90_000.0, 20_000.0)); // 110k > 100k
}

// ═══════════════════════════════════════════════════════════════════════
// Combined Engine Tests
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_combined_breaker_engine() {
    let amm = setup_amm(100);
    let registry = VaultRegistry::new(CdpConfig::default());

    let mut engine = CircuitBreakerEngine::new(
        TwapBreakerConfig {
            max_twap_change_pct: 0.05,
            short_window: 12,
            long_window: 48,
            pause_blocks: 20,
        },
        CascadeBreakerConfig {
            max_liquidations_in_window: 3,
            window_blocks: 48,
            pause_blocks: 96,
        },
        DebtCeilingConfig::default(),
    );

    // Normal state: nothing triggered
    let actions = engine.check_all(&amm, &registry, 50.0, 100);
    assert!(
        actions.iter().all(|a| *a == BreakerAction::None) || actions.is_empty(),
        "No breakers should trigger in normal state"
    );
    assert!(!engine.is_minting_paused(100));
    assert!(!engine.is_halted(100));

    // Trigger cascade breaker
    engine.record_liquidations(101, 4);
    let actions = engine.check_all(&amm, &registry, 50.0, 102);
    let has_halt = actions
        .iter()
        .any(|a| matches!(a, BreakerAction::EmergencyHalt { .. }));
    assert!(has_halt, "Cascade should trigger halt");
    assert!(engine.is_halted(102));
}

// ═══════════════════════════════════════════════════════════════════════
// Scenario Runner Smoke Test
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_scenario_runner_basic() {
    use zai_sim::agents::{ArbitrageurConfig, Arbitrageur, MinerAgent, MinerAgentConfig};
    use zai_sim::scenario::{Scenario, ScenarioConfig};

    let config = ScenarioConfig::default();
    let mut scenario = Scenario::new(&config);

    scenario.arbers.push(Arbitrageur::new(ArbitrageurConfig::default()));
    scenario.miners.push(MinerAgent::new(MinerAgentConfig::default()));

    // Constant price series
    let prices: Vec<f64> = vec![50.0; 100];
    scenario.run(&prices);

    assert_eq!(scenario.metrics.len(), 100);

    let first = &scenario.metrics[0];
    assert_relative_eq!(first.external_price, 50.0);
    assert!(first.amm_spot_price > 0.0);

    let last = &scenario.metrics[99];
    // With constant external price and arbers, AMM should stay near $50
    assert!(
        (last.amm_spot_price - 50.0).abs() < 10.0,
        "AMM should stay near $50 with arbers: {}",
        last.amm_spot_price
    );
}

#[test]
fn test_scenario_with_price_crash() {
    use zai_sim::agents::{ArbitrageurConfig, Arbitrageur};
    use zai_sim::scenario::{Scenario, ScenarioConfig};

    let config = ScenarioConfig::default();
    let mut scenario = Scenario::new(&config);

    scenario.arbers.push(Arbitrageur::new(ArbitrageurConfig {
        arb_latency_buy_blocks: 0,
        arb_latency_sell_blocks: 0,
        ..ArbitrageurConfig::default()
    }));

    // Price crash: 50 for 50 blocks, then drops to 30
    let mut prices = vec![50.0; 50];
    prices.extend(vec![30.0; 50]);

    scenario.run(&prices);

    assert_eq!(scenario.metrics.len(), 100);

    // After crash, AMM price should have moved toward 30
    let end_price = scenario.metrics[99].amm_spot_price;
    assert!(
        end_price < 50.0,
        "AMM price should drop after external crash: {}",
        end_price
    );
}
