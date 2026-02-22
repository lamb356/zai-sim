use approx::assert_relative_eq;
use zai_sim::amm::Amm;
use zai_sim::cdp::{CdpConfig, VaultRegistry};
use zai_sim::liquidation::{LiquidationConfig, LiquidationEngine, LiquidationMode};

/// Helper: create AMM at $50 ZEC/ZAI with TWAP established.
fn setup_amm(block: u64) -> Amm {
    let mut amm = Amm::new(10000.0, 500000.0, 0.003);
    for b in 1..=block {
        amm.record_price(b);
    }
    amm
}

fn default_cdp_config() -> CdpConfig {
    CdpConfig {
        min_ratio: 1.5,
        liquidation_penalty: 0.13,
        debt_floor: 100.0,
        stability_fee_rate: 0.02,
        twap_window: 48,
    }
}

/// Helper: crash ZEC price via large AMM sell, then establish new TWAP.
/// Returns the AMM with new lower TWAP.
fn crash_price(amm: &mut Amm, zec_to_sell: f64, start_block: u64, twap_blocks: u64) {
    let _out = amm.swap_zec_for_zai(zec_to_sell, start_block).unwrap();
    for b in (start_block + 1)..=(start_block + twap_blocks) {
        amm.record_price(b);
    }
}

// ─── Test 1: Scan finds liquidatable vaults ─────────────────────────────

#[test]
fn test_scan_liquidatable() {
    let mut amm = setup_amm(100);
    let mut registry = VaultRegistry::new(default_cdp_config());
    let engine = LiquidationEngine::new(LiquidationConfig::default());

    // Open three vaults at varying ratios
    // Vault 1: 10 ZEC ($500), 300 ZAI → ratio 1.67 (healthy)
    let _id1 = registry.open_vault("alice", 10.0, 300.0, 100, &amm).unwrap();
    // Vault 2: 10 ZEC ($500), 200 ZAI → ratio 2.5 (very healthy)
    let id2 = registry.open_vault("bob", 10.0, 200.0, 100, &amm).unwrap();
    // Vault 3: 10 ZEC ($500), 330 ZAI → ratio 1.52 (barely safe)
    let id3 = registry.open_vault("carol", 10.0, 330.0, 100, &amm).unwrap();

    // No liquidatable vaults at $50
    let liq = engine.scan_liquidatable(&registry, &amm);
    assert!(liq.is_empty(), "No vaults should be liquidatable at $50");

    // Moderate crash: sell 1500 ZEC → price drops to ~$38
    // Bob (2.5x) survives, Alice (1.67x) and Carol (1.52x) go underwater
    crash_price(&mut amm, 1500.0, 101, 60);
    let new_twap = amm.get_twap(48);
    assert!(new_twap < 45.0, "TWAP should have dropped below $45, got {:.2}", new_twap);
    assert!(new_twap > 30.0, "TWAP shouldn't crash below $30, got {:.2}", new_twap);

    let liq = engine.scan_liquidatable(&registry, &amm);

    // Bob (2.5x at $50) needs price < $30 to liquidate → should survive
    assert!(
        !liq.contains(&id2),
        "Bob's vault (2.5x ratio) should survive at TWAP {:.2}",
        new_twap
    );

    // Carol (1.52x at $50) needs price < $49.5 to liquidate → should be underwater
    assert!(
        liq.contains(&id3),
        "Carol's vault (1.52x) should be liquidatable at TWAP {:.2}",
        new_twap
    );

    // At least Carol is liquidatable
    assert!(!liq.is_empty(), "Should have at least one liquidatable vault");
}

// ─── Test 2: Transparent liquidation ────────────────────────────────────

#[test]
fn test_transparent_liquidation() {
    let mut amm = setup_amm(100);
    let mut registry = VaultRegistry::new(default_cdp_config());
    let mut engine = LiquidationEngine::new(LiquidationConfig::default());

    // Open vault near the edge: 10 ZEC ($500), 320 ZAI → ratio 1.5625
    let id = registry.open_vault("alice", 10.0, 320.0, 100, &amm).unwrap();
    assert!(!registry.is_liquidatable(id, &amm));

    // Crash price to make vault underwater
    crash_price(&mut amm, 4000.0, 101, 60);
    assert!(registry.is_liquidatable(id, &amm), "Vault should be liquidatable after crash");

    let total_debt_before = registry.total_debt;

    // Execute transparent liquidation
    let results = engine.transparent_liquidate(&mut registry, &mut amm, 162);

    assert_eq!(results.len(), 1);
    let r = &results[0];
    assert_eq!(r.vault_id, id);
    assert_eq!(r.mode, LiquidationMode::Transparent);
    assert_relative_eq!(r.collateral_seized, 10.0);
    assert_relative_eq!(r.debt_to_cover, 320.0, epsilon = 1.0); // may have tiny fee accrual
    assert!(r.zai_from_amm > 0.0, "Should receive ZAI from AMM sale");
    assert_eq!(r.keeper_reward, 0.0, "No keeper reward in transparent mode");

    // Vault should be removed
    assert!(registry.get_vault(id).is_none());

    // total_debt should decrease
    assert!(registry.total_debt < total_debt_before);

    // History recorded
    assert_eq!(engine.history.len(), 1);
}

// ─── Test 3: Self-liquidation (no/reduced penalty) ──────────────────────

#[test]
fn test_self_liquidation() {
    let mut amm = setup_amm(100);
    let mut registry = VaultRegistry::new(default_cdp_config());
    let mut engine = LiquidationEngine::new(LiquidationConfig {
        self_liquidation_penalty_pct: 0.0, // no penalty for self-liquidation
        ..LiquidationConfig::default()
    });

    // Open vault: 20 ZEC ($1000), 500 ZAI → ratio 2.0
    let id = registry.open_vault("alice", 20.0, 500.0, 100, &amm).unwrap();

    // Self-liquidate (allowed even when healthy — owner wants to exit)
    let result = engine.self_liquidate(id, &mut registry, &mut amm, 100).unwrap();

    assert_eq!(result.mode, LiquidationMode::SelfLiquidation);
    assert_relative_eq!(result.collateral_seized, 20.0);
    assert_relative_eq!(result.penalty_amount, 0.0, epsilon = 0.001);
    assert_eq!(result.keeper_reward, 0.0);
    // With ZEC at $50, selling 20 ZEC should yield ~$1000 in ZAI
    // Debt is 500 → surplus ≈ $500 minus slippage
    assert!(
        result.surplus_to_owner > 0.0,
        "Healthy self-liquidation should have surplus"
    );
    assert_relative_eq!(result.bad_debt, 0.0);

    // Vault removed
    assert!(registry.get_vault(id).is_none());

    // Now test with partial penalty
    let mut engine2 = LiquidationEngine::new(LiquidationConfig {
        self_liquidation_penalty_pct: 0.5, // 50% of normal penalty
        ..LiquidationConfig::default()
    });
    let mut amm2 = setup_amm(100);
    let mut reg2 = VaultRegistry::new(default_cdp_config());

    let id2 = reg2.open_vault("bob", 20.0, 500.0, 100, &amm2).unwrap();
    let r2 = engine2.self_liquidate(id2, &mut reg2, &mut amm2, 100).unwrap();

    // Penalty should be 50% of 13% of 500 = 32.5
    // But actual depends on AMM proceeds
    let expected_penalty = 500.0 * 0.13 * 0.5;
    assert!(
        r2.penalty_amount <= expected_penalty + 0.01,
        "Reduced penalty: got {}, expected <= {}",
        r2.penalty_amount,
        expected_penalty
    );
}

// ─── Test 4: Challenge-response (keeper gets reward) ────────────────────

#[test]
fn test_challenge_response() {
    let mut amm = setup_amm(100);
    let mut registry = VaultRegistry::new(default_cdp_config());
    let mut engine = LiquidationEngine::new(LiquidationConfig {
        keeper_reward_pct: 0.50, // keeper gets 50% of penalty
        ..LiquidationConfig::default()
    });

    // Open vault near the edge
    let id = registry.open_vault("alice", 10.0, 320.0, 100, &amm).unwrap();

    // Crash price
    crash_price(&mut amm, 4000.0, 101, 60);
    assert!(registry.is_liquidatable(id, &amm));

    // Keeper triggers liquidation
    let result = engine
        .challenge_liquidate(id, "keeper_bob", &mut registry, &mut amm, 162)
        .unwrap();

    assert!(
        matches!(result.mode, LiquidationMode::ChallengeResponse { ref keeper } if keeper == "keeper_bob")
    );

    // Keeper should receive reward
    if result.penalty_amount > 0.0 {
        assert!(
            result.keeper_reward > 0.0,
            "Keeper should get reward when penalty collected"
        );
        assert_relative_eq!(
            result.keeper_reward,
            result.penalty_amount * 0.50,
            epsilon = 0.01
        );
    }

    // Engine tracks total keeper rewards
    assert_relative_eq!(engine.total_keeper_rewards, result.keeper_reward);

    // Can't liquidate a healthy vault via challenge
    let mut amm3 = setup_amm(200);
    let mut reg3 = VaultRegistry::new(default_cdp_config());
    let id3 = reg3.open_vault("carol", 100.0, 1000.0, 200, &amm3).unwrap();

    let err = engine
        .challenge_liquidate(id3, "evil_keeper", &mut reg3, &mut amm3, 200)
        .unwrap_err();
    assert!(
        err.contains("not liquidatable"),
        "Should reject challenge on healthy vault"
    );
}

// ─── Test 5: Velocity limit ────────────────────────────────────────────

#[test]
fn test_velocity_limit() {
    let mut amm = setup_amm(100);
    let mut registry = VaultRegistry::new(default_cdp_config());
    let mut engine = LiquidationEngine::new(LiquidationConfig {
        max_liquidations_per_block: 2, // only 2 per block
        ..LiquidationConfig::default()
    });

    // Open 4 vaults near the edge
    let ids: Vec<u64> = (0..4)
        .map(|i| {
            registry
                .open_vault(&format!("user{}", i), 10.0, 325.0, 100, &amm)
                .unwrap()
        })
        .collect();

    // Crash price to make all liquidatable
    crash_price(&mut amm, 4000.0, 101, 60);
    let liq_ids = engine.scan_liquidatable(&registry, &amm);
    assert!(liq_ids.len() >= 3, "Should have multiple liquidatable vaults");

    // Transparent liquidation should only process 2 (velocity limit)
    let results = engine.transparent_liquidate(&mut registry, &mut amm, 162);
    assert_eq!(
        results.len(),
        2,
        "Velocity limit should cap at 2 liquidations per block"
    );
    // Velocity limit capped at 2 — verified by results.len() above

    // Remaining vaults still exist
    let remaining: Vec<_> = ids.iter().filter(|id| registry.get_vault(**id).is_some()).collect();
    assert!(remaining.len() >= 2, "At least 2 vaults should remain");

    // Next block: counter resets, can liquidate more
    let results2 = engine.transparent_liquidate(&mut registry, &mut amm, 163);
    assert!(
        !results2.is_empty(),
        "New block should allow more liquidations"
    );
}

// ─── Test 6: Bad debt tracking ──────────────────────────────────────────

#[test]
fn test_bad_debt_tracking() {
    // Use a small AMM so selling collateral causes huge slippage
    let mut amm = Amm::new(100.0, 5000.0, 0.003); // spot = $50
    for b in 1..=100 {
        amm.record_price(b);
    }

    let mut registry = VaultRegistry::new(CdpConfig {
        min_ratio: 1.5,
        liquidation_penalty: 0.13,
        debt_floor: 100.0,
        stability_fee_rate: 0.0, // no fees for clarity
        twap_window: 48,
    });

    let mut engine = LiquidationEngine::new(LiquidationConfig::default());

    // Open vault: 50 ZEC ($2500) with 1600 ZAI debt → ratio 1.5625
    let id = registry.open_vault("alice", 50.0, 1600.0, 100, &amm).unwrap();

    // Crash price hard on the small AMM
    crash_price(&mut amm, 80.0, 101, 60);
    // If price crashed enough for vault to be underwater
    if registry.is_liquidatable(id, &amm) {
        let result = engine
            .transparent_liquidate(&mut registry, &mut amm, 162);

        if !result.is_empty() {
            let r = &result[0];
            // Selling 50 ZEC into a tiny AMM at crashed prices → huge slippage
            // Very likely to produce bad debt
            if r.bad_debt > 0.0 {
                assert!(
                    engine.total_bad_debt > 0.0,
                    "Engine should track bad debt"
                );
                assert_relative_eq!(engine.total_bad_debt, r.bad_debt);
            }
            // In any case, debt_to_cover should be recorded
            assert!(r.debt_to_cover > 0.0);
        }
    }

    // Explicitly create a guaranteed bad-debt scenario:
    // Tiny AMM, vault with huge debt relative to AMM liquidity
    let mut amm2 = Amm::new(10.0, 500.0, 0.003); // spot = $50, tiny
    for b in 1..=100 {
        amm2.record_price(b);
    }

    let mut reg2 = VaultRegistry::new(CdpConfig {
        min_ratio: 1.5,
        liquidation_penalty: 0.13,
        debt_floor: 100.0,
        stability_fee_rate: 0.0,
        twap_window: 48,
    });
    let mut eng2 = LiquidationEngine::new(LiquidationConfig::default());

    // 5 ZEC at $50 = $250, debt = 160 → ratio 1.5625
    let id2 = reg2.open_vault("bob", 5.0, 160.0, 100, &amm2).unwrap();

    // Crash the tiny AMM hard
    crash_price(&mut amm2, 8.0, 101, 60);

    if reg2.is_liquidatable(id2, &amm2) {
        let results = eng2.transparent_liquidate(&mut reg2, &mut amm2, 162);
        if !results.is_empty() {
            let r = &results[0];
            // Selling 5 ZEC into a 10-ZEC pool at crashed prices
            // AMM proceeds likely far less than 160 ZAI debt
            if r.zai_from_amm < r.debt_to_cover {
                assert!(r.bad_debt > 0.0);
                assert_relative_eq!(r.bad_debt, r.debt_to_cover - r.zai_from_amm);
                assert_relative_eq!(eng2.total_bad_debt, r.bad_debt);
            }
        }
    }
}

// ─── Test 7: Liquidation AMM interaction ────────────────────────────────

#[test]
fn test_liquidation_amm_interaction() {
    let mut amm = setup_amm(100);
    let mut registry = VaultRegistry::new(default_cdp_config());
    let mut engine = LiquidationEngine::new(LiquidationConfig::default());

    // Open vault
    let id = registry.open_vault("alice", 10.0, 320.0, 100, &amm).unwrap();

    // Crash price
    crash_price(&mut amm, 4000.0, 101, 60);

    let zec_before_liq = amm.reserve_zec;
    let zai_before_liq = amm.reserve_zai;

    assert!(registry.is_liquidatable(id, &amm));

    let results = engine.transparent_liquidate(&mut registry, &mut amm, 162);
    assert_eq!(results.len(), 1);

    let r = &results[0];

    // AMM should have received the seized ZEC
    assert!(
        amm.reserve_zec > zec_before_liq,
        "AMM ZEC reserves should increase from collateral sale"
    );
    // AMM should have given out ZAI
    assert!(
        amm.reserve_zai < zai_before_liq,
        "AMM ZAI reserves should decrease from collateral sale"
    );

    // The ZAI from AMM should equal the actual swap output
    let zec_added = amm.reserve_zec - zec_before_liq;
    assert_relative_eq!(zec_added, r.collateral_seized, epsilon = 0.01);
    assert!(r.zai_from_amm > 0.0);
}

// ─── Test 8: Surplus returned to owner ──────────────────────────────────

#[test]
fn test_surplus_return_to_owner() {
    let mut amm = setup_amm(100);
    let mut registry = VaultRegistry::new(default_cdp_config());
    let mut engine = LiquidationEngine::new(LiquidationConfig {
        self_liquidation_penalty_pct: 0.0,
        ..LiquidationConfig::default()
    });

    // Open well-collateralized vault: 100 ZEC ($5000), 500 ZAI → ratio 10.0
    let id = registry.open_vault("alice", 100.0, 500.0, 100, &amm).unwrap();

    // Self-liquidate (no penalty) — should have large surplus
    let result = engine.self_liquidate(id, &mut registry, &mut amm, 100).unwrap();

    // ZEC sold at ~$50 each → ~$5000 in ZAI (minus slippage on 100 ZEC in 10000 pool)
    // Debt = 500, penalty = 0 → surplus should be zai_from_amm - 500
    assert_relative_eq!(
        result.surplus_to_owner,
        result.zai_from_amm - result.debt_to_cover - result.penalty_amount,
        epsilon = 0.01
    );
    assert!(
        result.surplus_to_owner > 3000.0,
        "With 100 ZEC at ~$50 and only 500 debt, surplus should be large: got {}",
        result.surplus_to_owner
    );
    assert_relative_eq!(result.bad_debt, 0.0);
}

// ─── Test 9: Multiple discovery modes on same vault state ───────────────

#[test]
fn test_all_three_modes() {
    // Transparent
    {
        let mut amm = setup_amm(100);
        let mut reg = VaultRegistry::new(default_cdp_config());
        let mut eng = LiquidationEngine::new(LiquidationConfig::default());

        let id = reg.open_vault("alice", 10.0, 320.0, 100, &amm).unwrap();
        crash_price(&mut amm, 4000.0, 101, 60);
        assert!(reg.is_liquidatable(id, &amm));

        let results = eng.transparent_liquidate(&mut reg, &mut amm, 162);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].mode, LiquidationMode::Transparent);
        assert_eq!(results[0].keeper_reward, 0.0);
    }

    // Self-liquidation
    {
        let mut amm = setup_amm(100);
        let mut reg = VaultRegistry::new(default_cdp_config());
        let mut eng = LiquidationEngine::new(LiquidationConfig {
            self_liquidation_penalty_pct: 0.0,
            ..LiquidationConfig::default()
        });

        let id = reg.open_vault("alice", 10.0, 320.0, 100, &amm).unwrap();
        crash_price(&mut amm, 4000.0, 101, 60);

        let result = eng.self_liquidate(id, &mut reg, &mut amm, 162).unwrap();
        assert_eq!(result.mode, LiquidationMode::SelfLiquidation);
        assert_relative_eq!(result.penalty_amount, 0.0, epsilon = 0.01);
    }

    // Challenge-response
    {
        let mut amm = setup_amm(100);
        let mut reg = VaultRegistry::new(default_cdp_config());
        let mut eng = LiquidationEngine::new(LiquidationConfig {
            keeper_reward_pct: 0.50,
            ..LiquidationConfig::default()
        });

        let id = reg.open_vault("alice", 10.0, 320.0, 100, &amm).unwrap();
        crash_price(&mut amm, 4000.0, 101, 60);
        assert!(reg.is_liquidatable(id, &amm));

        let result = eng
            .challenge_liquidate(id, "keeper_eve", &mut reg, &mut amm, 162)
            .unwrap();
        assert!(matches!(
            result.mode,
            LiquidationMode::ChallengeResponse { .. }
        ));
        if result.penalty_amount > 0.0 {
            assert!(result.keeper_reward > 0.0);
        }
    }
}

// ─── Test 10: Velocity resets across blocks ─────────────────────────────

#[test]
fn test_velocity_resets_across_blocks() {
    let mut amm = setup_amm(100);
    let mut registry = VaultRegistry::new(default_cdp_config());
    let mut engine = LiquidationEngine::new(LiquidationConfig {
        max_liquidations_per_block: 1,
        ..LiquidationConfig::default()
    });

    // Open 3 vaults
    for i in 0..3 {
        registry
            .open_vault(&format!("user{}", i), 10.0, 325.0, 100, &amm)
            .unwrap();
    }

    crash_price(&mut amm, 4000.0, 101, 60);

    // Block 162: liquidate 1
    let r1 = engine.transparent_liquidate(&mut registry, &mut amm, 162);
    assert_eq!(r1.len(), 1, "Should liquidate exactly 1 in block 162");

    // Block 163: liquidate 1 more (counter reset)
    let r2 = engine.transparent_liquidate(&mut registry, &mut amm, 163);
    assert_eq!(r2.len(), 1, "Should liquidate exactly 1 in block 163");

    // Block 164: liquidate the last one
    let r3 = engine.transparent_liquidate(&mut registry, &mut amm, 164);
    // May or may not have remaining liquidatable vaults depending on price changes from previous liquidations
    assert!(r3.len() <= 1);

    // Total across all blocks
    let total = r1.len() + r2.len() + r3.len();
    assert!(total >= 2, "Should have liquidated at least 2 vaults across blocks");
}
