use approx::assert_relative_eq;
use zai_sim::controller::{Controller, ControllerConfig, ControllerMode};

// ─── Test 1: PI — market above peg → rate decreases → price falls ──────

#[test]
fn test_pi_peg_above() {
    let config = ControllerConfig {
        mode: ControllerMode::PI {
            kp: 1e-4,
            ki: 1e-6,
        },
        min_rate: -1e-3,
        max_rate: 1e-3,
        integral_min: -1e-3,
        integral_max: 1e-3,
    };

    let mut ctrl = Controller::new(config, 1.0, 0);

    // Market at $1.10, target at $1.00 → 10% above peg
    let rate = ctrl.update(1.10, 1);

    // Rate should be negative (push redemption_price down)
    assert!(
        rate < 0.0,
        "Rate should be negative when market > target, got {}",
        rate
    );

    // Proportional component: -kp * 0.10 = -1e-5
    // Integral component: -ki * 0.10 = -1e-7
    // Total ≈ -1.01e-5
    assert!(rate > -1e-3, "Rate should not hit lower bound");

    // After stepping, redemption_price should decrease
    let price_before = ctrl.redemption_price;
    ctrl.step(100);
    assert!(
        ctrl.redemption_price < price_before,
        "Redemption price should fall when rate is negative"
    );
}

// ─── Test 2: PI — market below peg → rate increases → price rises ──────

#[test]
fn test_pi_peg_below() {
    let config = ControllerConfig {
        mode: ControllerMode::PI {
            kp: 1e-4,
            ki: 1e-6,
        },
        min_rate: -1e-3,
        max_rate: 1e-3,
        integral_min: -1e-3,
        integral_max: 1e-3,
    };

    let mut ctrl = Controller::new(config, 1.0, 0);

    // Market at $0.90, target at $1.00 → 10% below peg
    let rate = ctrl.update(0.90, 1);

    // Rate should be positive (push redemption_price up)
    assert!(
        rate > 0.0,
        "Rate should be positive when market < target, got {}",
        rate
    );

    let price_before = ctrl.redemption_price;
    ctrl.step(100);
    assert!(
        ctrl.redemption_price > price_before,
        "Redemption price should rise when rate is positive"
    );
}

// ─── Test 3: PI — at peg → no change ───────────────────────────────────

#[test]
fn test_pi_at_peg() {
    let config = ControllerConfig {
        mode: ControllerMode::PI {
            kp: 1e-4,
            ki: 1e-6,
        },
        min_rate: -1e-3,
        max_rate: 1e-3,
        integral_min: -1e-3,
        integral_max: 1e-3,
    };

    let mut ctrl = Controller::new(config, 1.0, 0);

    // Market exactly at peg
    let rate = ctrl.update(1.0, 1);

    assert_relative_eq!(rate, 0.0, epsilon = 1e-15);
    assert_relative_eq!(ctrl.integral, 0.0, epsilon = 1e-15);
    assert_relative_eq!(ctrl.redemption_price, 1.0, epsilon = 1e-15);
}

// ─── Test 4: PI — anti-windup clamps integral ───────────────────────────

#[test]
fn test_pi_anti_windup() {
    let config = ControllerConfig {
        mode: ControllerMode::PI {
            kp: 1e-4,
            ki: 1e-2, // very high Ki to saturate quickly
        },
        min_rate: -1e-3,
        max_rate: 1e-3,
        integral_min: -5e-4,
        integral_max: 5e-4,
    };

    let mut ctrl = Controller::new(config, 1.0, 0);

    // Sustained 50% deviation above peg — integral should wind up
    for block in 1..=1000 {
        ctrl.update(1.50, block);
    }

    // Integral should be clamped at integral_min (negative because market > target)
    assert_relative_eq!(ctrl.integral, -5e-4, epsilon = 1e-10);

    // Rate should be clamped within bounds
    assert!(ctrl.redemption_rate >= -1e-3);
    assert!(ctrl.redemption_rate <= 1e-3);

    // Now reverse: sustained deviation below peg
    let mut ctrl2 = Controller::new(
        ControllerConfig {
            mode: ControllerMode::PI {
                kp: 1e-4,
                ki: 1e-2,
            },
            min_rate: -1e-3,
            max_rate: 1e-3,
            integral_min: -5e-4,
            integral_max: 5e-4,
        },
        1.0,
        0,
    );

    for block in 1..=1000 {
        ctrl2.update(0.50, block);
    }

    // Integral clamped at integral_max (positive because market < target)
    assert_relative_eq!(ctrl2.integral, 5e-4, epsilon = 1e-10);
}

// ─── Test 5: Tick — market above peg → rate goes negative ──────────────

#[test]
fn test_tick_peg_above() {
    let config = ControllerConfig {
        mode: ControllerMode::Tick {
            sensitivity: 1e-4,
        },
        min_rate: -1e-3,
        max_rate: 1e-3,
        integral_min: -1e-3,
        integral_max: 1e-3,
    };

    let mut ctrl = Controller::new(config, 1.0, 0);

    // Market 10% above peg
    let rate = ctrl.update(1.10, 1);

    // ln(1.10) ≈ 0.0953, integral = -1e-4 * 0.0953 ≈ -9.53e-6
    assert!(rate < 0.0, "Rate should be negative when market > target");

    let expected_integral = -1e-4 * (1.10_f64).ln();
    assert_relative_eq!(ctrl.integral, expected_integral, epsilon = 1e-12);

    // Step forward and verify price decreases
    let price_before = ctrl.redemption_price;
    ctrl.step(100);
    assert!(ctrl.redemption_price < price_before);
}

// ─── Test 6: Tick — market below peg → rate goes positive ──────────────

#[test]
fn test_tick_peg_below() {
    let config = ControllerConfig {
        mode: ControllerMode::Tick {
            sensitivity: 1e-4,
        },
        min_rate: -1e-3,
        max_rate: 1e-3,
        integral_min: -1e-3,
        integral_max: 1e-3,
    };

    let mut ctrl = Controller::new(config, 1.0, 0);

    // Market 10% below peg
    let rate = ctrl.update(0.90, 1);

    // ln(0.90) ≈ -0.1054, integral = -1e-4 * (-0.1054) ≈ +1.054e-5
    assert!(rate > 0.0, "Rate should be positive when market < target");

    let expected_integral = -1e-4 * (0.90_f64).ln();
    assert_relative_eq!(ctrl.integral, expected_integral, epsilon = 1e-12);
}

// ─── Test 7: Tick — rate clamped to min/max ─────────────────────────────

#[test]
fn test_tick_clamped() {
    let config = ControllerConfig {
        mode: ControllerMode::Tick {
            sensitivity: 1.0, // very high sensitivity to exceed bounds
        },
        min_rate: -5e-5,
        max_rate: 5e-5,
        integral_min: -5e-5,
        integral_max: 5e-5,
    };

    let mut ctrl = Controller::new(config, 1.0, 0);

    // Large deviation above peg — should hit lower clamp
    ctrl.update(2.0, 1);
    assert_relative_eq!(
        ctrl.redemption_rate,
        -5e-5,
        epsilon = 1e-15
    );
    assert_relative_eq!(ctrl.integral, -5e-5, epsilon = 1e-15);

    // Reset and test upper clamp
    let config2 = ControllerConfig {
        mode: ControllerMode::Tick {
            sensitivity: 1.0,
        },
        min_rate: -5e-5,
        max_rate: 5e-5,
        integral_min: -5e-5,
        integral_max: 5e-5,
    };
    let mut ctrl2 = Controller::new(config2, 1.0, 0);

    // Large deviation below peg — should hit upper clamp
    ctrl2.update(0.5, 1);
    assert_relative_eq!(
        ctrl2.redemption_rate,
        5e-5,
        epsilon = 1e-15
    );
}

// ─── Test 8: Tick — higher sensitivity → faster response ────────────────

#[test]
fn test_tick_sensitivity() {
    let make_ctrl = |sens: f64| -> Controller {
        Controller::new(
            ControllerConfig {
                mode: ControllerMode::Tick {
                    sensitivity: sens,
                },
                min_rate: -1e-2,
                max_rate: 1e-2,
                integral_min: -1e-2,
                integral_max: 1e-2,
            },
            1.0,
            0,
        )
    };

    let mut ctrl_low = make_ctrl(1e-6);
    let mut ctrl_high = make_ctrl(1e-4);

    // Same deviation
    let rate_low = ctrl_low.update(1.05, 1);
    let rate_high = ctrl_high.update(1.05, 1);

    // Higher sensitivity → larger magnitude rate change
    assert!(
        rate_high.abs() > rate_low.abs(),
        "Higher sensitivity should produce larger rate: low={}, high={}",
        rate_low.abs(),
        rate_high.abs()
    );

    // Ratio should be ~100x (sensitivity ratio)
    let ratio = rate_high.abs() / rate_low.abs();
    assert_relative_eq!(ratio, 100.0, epsilon = 0.01);
}

// ─── Test 9: Step advances redemption_price correctly ───────────────────

#[test]
fn test_step_advances_price() {
    let config = ControllerConfig::default_pi();
    let mut ctrl = Controller::new(config, 1.0, 0);

    // Set a known positive rate
    ctrl.redemption_rate = 1e-5; // per block

    // Step 1000 blocks
    ctrl.step(1000);

    // Expected: 1.0 * (1 + 1e-5)^1000 ≈ 1.01005
    let expected = 1.0 * (1.0 + 1e-5_f64).powi(1000);
    assert_relative_eq!(ctrl.redemption_price, expected, epsilon = 1e-10);
    assert_eq!(ctrl.last_block, 1000);

    // Negative rate
    let mut ctrl2 = Controller::new(ControllerConfig::default_pi(), 1.0, 0);
    ctrl2.redemption_rate = -1e-5;
    ctrl2.step(1000);

    let expected2 = 1.0 * (1.0 - 1e-5_f64).powi(1000);
    assert_relative_eq!(ctrl2.redemption_price, expected2, epsilon = 1e-10);
    assert!(ctrl2.redemption_price < 1.0);

    // Step to same block → no change
    let price_before = ctrl2.redemption_price;
    ctrl2.step(1000);
    assert_relative_eq!(ctrl2.redemption_price, price_before);
}

// ─── Test 10: PI convergence simulation ─────────────────────────────────

#[test]
fn test_pi_sustained_correction() {
    // The controller adjusts redemption_price as a lever to push market back to peg.
    // With a fixed above-peg market, the controller should consistently apply
    // downward pressure (negative rate) and the integral should accumulate.
    let config = ControllerConfig {
        mode: ControllerMode::PI {
            kp: 1e-4,
            ki: 1e-6,
        },
        min_rate: -1e-3,
        max_rate: 1e-3,
        integral_min: -1e-3,
        integral_max: 1e-3,
    };

    let mut ctrl = Controller::new(config, 1.0, 0);

    // Market 5% above peg
    let market = 1.05;

    // First update
    let rate1 = ctrl.update(market, 1);
    assert!(rate1 < 0.0, "Rate should be negative when market > target");

    // After several updates, integral should have accumulated
    for block in 2..=100 {
        ctrl.update(market, block);
    }

    let rate100 = ctrl.redemption_rate;
    // Integral adds to proportional → rate magnitude should grow over time
    assert!(
        rate100.abs() > rate1.abs(),
        "Rate magnitude should grow with integral: block1={}, block100={}",
        rate1.abs(),
        rate100.abs()
    );

    // Rate should still be negative (consistent direction)
    assert!(rate100 < 0.0, "Rate should stay negative");

    // Integral should be negative (accumulated below-peg corrections)
    assert!(ctrl.integral < 0.0, "Integral should be negative");

    // Redemption price should have decreased (lever pushing market down)
    assert!(
        ctrl.redemption_price < 1.0,
        "Target should have fallen: {}",
        ctrl.redemption_price
    );
}

// ─── Test 11: Tick convergence simulation ───────────────────────────────

#[test]
fn test_tick_sustained_correction() {
    // Tick controller with market below peg: should build positive rate
    // that pushes redemption_price down (making ZAI more attractive → price rises).
    let config = ControllerConfig {
        mode: ControllerMode::Tick {
            sensitivity: 1e-5,
        },
        min_rate: -1e-3,
        max_rate: 1e-3,
        integral_min: -1e-3,
        integral_max: 1e-3,
    };

    let mut ctrl = Controller::new(config, 1.0, 0);

    // Market 5% below peg
    let market = 0.95;

    let rate1 = ctrl.update(market, 1);
    assert!(rate1 > 0.0, "Rate should be positive when market < target");

    // Sustained below-peg: integral (= rate) should grow
    for block in 2..=100 {
        ctrl.update(market, block);
    }

    let rate100 = ctrl.redemption_rate;
    assert!(
        rate100 > rate1,
        "Rate should grow with sustained deviation: block1={}, block100={}",
        rate1,
        rate100
    );

    // Integral is the rate in Tick mode
    assert_relative_eq!(ctrl.integral, ctrl.redemption_rate, epsilon = 1e-15);

    // Redemption price should have risen (positive rate compounds upward)
    assert!(
        ctrl.redemption_price > 1.0,
        "Target should have risen: {}",
        ctrl.redemption_price
    );

    // Verify log-scale: integral should equal -sensitivity * sum(ln(market/target))
    // Since target changes each block, we can't easily compute exact value,
    // but direction and monotonicity are verified above.
}

// ─── Test 12: PI vs Tick — both correct in same direction ───────────────

#[test]
fn test_pi_vs_tick_direction() {
    // Both controllers should push in the same direction for the same deviation

    let mut pi = Controller::new(
        ControllerConfig {
            mode: ControllerMode::PI {
                kp: 1e-4,
                ki: 1e-6,
            },
            min_rate: -1e-3,
            max_rate: 1e-3,
            integral_min: -1e-3,
            integral_max: 1e-3,
        },
        1.0,
        0,
    );

    let mut tick = Controller::new(
        ControllerConfig {
            mode: ControllerMode::Tick {
                sensitivity: 1e-4,
            },
            min_rate: -1e-3,
            max_rate: 1e-3,
            integral_min: -1e-3,
            integral_max: 1e-3,
        },
        1.0,
        0,
    );

    // Market above peg
    let pi_rate = pi.update(1.10, 1);
    let tick_rate = tick.update(1.10, 1);

    assert!(pi_rate < 0.0, "PI rate should be negative");
    assert!(tick_rate < 0.0, "Tick rate should be negative");

    // Market below peg (fresh controllers)
    let mut pi2 = Controller::new(
        ControllerConfig {
            mode: ControllerMode::PI {
                kp: 1e-4,
                ki: 1e-6,
            },
            min_rate: -1e-3,
            max_rate: 1e-3,
            integral_min: -1e-3,
            integral_max: 1e-3,
        },
        1.0,
        0,
    );

    let mut tick2 = Controller::new(
        ControllerConfig {
            mode: ControllerMode::Tick {
                sensitivity: 1e-4,
            },
            min_rate: -1e-3,
            max_rate: 1e-3,
            integral_min: -1e-3,
            integral_max: 1e-3,
        },
        1.0,
        0,
    );

    let pi_rate2 = pi2.update(0.90, 1);
    let tick_rate2 = tick2.update(0.90, 1);

    assert!(pi_rate2 > 0.0, "PI rate should be positive");
    assert!(tick_rate2 > 0.0, "Tick rate should be positive");
}
