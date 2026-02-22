use approx::assert_relative_eq;
use zai_sim::amm::Amm;
use zai_sim::cdp::{CdpConfig, VaultRegistry};

/// Helper: create an AMM at $50 ZEC/ZAI with TWAP recorded for sufficient blocks.
fn setup_amm(block: u64) -> Amm {
    let mut amm = Amm::new(10000.0, 500000.0, 0.003); // spot = 50.0
    // Record price across enough blocks to have a valid TWAP
    for b in 1..=block {
        amm.record_price(b);
    }
    amm
}

fn default_config() -> CdpConfig {
    CdpConfig {
        min_ratio: 1.5,
        liquidation_penalty: 0.13,
        debt_floor: 100.0,
        stability_fee_rate: 0.02,
        twap_window: 48,
    }
}

// ─── Test 1: Open and close vault ───────────────────────────────────────

#[test]
fn test_open_close_vault() {
    let amm = setup_amm(100);
    let mut registry = VaultRegistry::new(default_config());

    // Open vault: 10 ZEC ($500 value) with 200 ZAI debt → ratio = 2.5 (above 1.5)
    let id = registry
        .open_vault("alice", 10.0, 200.0, 100, &amm)
        .unwrap();

    let vault = registry.get_vault(id).unwrap();
    assert_eq!(vault.owner, "alice");
    assert_relative_eq!(vault.collateral_zec, 10.0);
    assert_relative_eq!(vault.debt_zai, 200.0);
    assert_eq!(vault.created_block, 100);
    assert_relative_eq!(registry.total_debt, 200.0);

    // Close vault at the same block (no fee accrual)
    let (collateral, debt_owed) = registry.close_vault(id, 100).unwrap();
    assert_relative_eq!(collateral, 10.0);
    assert_relative_eq!(debt_owed, 200.0);
    assert_relative_eq!(registry.total_debt, 0.0);
    assert!(registry.get_vault(id).is_none());
}

// ─── Test 2: Collateral ratio calculation using TWAP ────────────────────

#[test]
fn test_collateral_ratio_calculation() {
    let amm = setup_amm(100);
    let mut registry = VaultRegistry::new(default_config());

    // ZEC TWAP = $50. Vault: 10 ZEC, 200 ZAI debt
    // Expected ratio = (10 * 50) / 200 = 2.5
    let id = registry
        .open_vault("alice", 10.0, 200.0, 100, &amm)
        .unwrap();

    let vault = registry.get_vault(id).unwrap();
    let price = amm.get_twap(48);
    assert_relative_eq!(price, 50.0, epsilon = 0.01);
    assert_relative_eq!(vault.collateral_ratio(price), 2.5, epsilon = 0.01);

    // Not liquidatable at 2.5x ratio
    assert!(!registry.is_liquidatable(id, &amm));

    // Open vault right at the edge: ratio = 1.5 exactly
    // 10 ZEC at $50 = $500. Debt = 500/1.5 = 333.33
    let id2 = registry
        .open_vault("bob", 10.0, 333.33, 100, &amm)
        .unwrap();

    let vault2 = registry.get_vault(id2).unwrap();
    let ratio2 = vault2.collateral_ratio(price);
    assert!(
        ratio2 >= 1.5,
        "Ratio should be at or above min: {:.4}",
        ratio2
    );
}

// ─── Test 3: Min ratio enforcement ──────────────────────────────────────

#[test]
fn test_min_ratio_enforcement() {
    let amm = setup_amm(100);
    let mut registry = VaultRegistry::new(default_config());

    // Try to open undercollateralized vault: 1 ZEC ($50), 100 ZAI → ratio = 0.5
    let result = registry.open_vault("alice", 1.0, 100.0, 100, &amm);
    assert!(result.is_err(), "Should reject vault below min ratio");
    assert!(result.unwrap_err().contains("below minimum"));

    // Open valid vault: 10 ZEC ($500), 300 ZAI → ratio ≈ 1.67
    let id = registry
        .open_vault("alice", 10.0, 300.0, 100, &amm)
        .unwrap();

    // Try to withdraw too much collateral
    // Withdrawing 4 ZEC → 6 ZEC left → (6*50)/300 = 1.0 < 1.5
    let result = registry.withdraw_collateral(id, 4.0, 100, &amm);
    assert!(result.is_err(), "Should reject withdrawal below min ratio");

    // Try to borrow too much
    // Current: 10 ZEC, 300 ZAI. Try borrow 100 more → 400 ZAI → (10*50)/400 = 1.25 < 1.5
    let result = registry.borrow_zai(id, 100.0, 100, &amm);
    assert!(result.is_err(), "Should reject borrow below min ratio");

    // Valid withdrawal: 1 ZEC → 9 ZEC → (9*50)/300 = 1.5 (exactly min)
    let result = registry.withdraw_collateral(id, 1.0, 100, &amm);
    assert!(result.is_ok(), "Should allow withdrawal at exactly min ratio");
}

// ─── Test 4: Stability fee accrual ──────────────────────────────────────

#[test]
fn test_stability_fee_accrual() {
    let amm = setup_amm(100);
    let mut registry = VaultRegistry::new(default_config());

    let id = registry
        .open_vault("alice", 100.0, 1000.0, 100, &amm)
        .unwrap();

    let initial_debt = registry.get_vault(id).unwrap().debt_zai;
    assert_relative_eq!(initial_debt, 1000.0);

    // Advance ~1 year worth of blocks (420,768 blocks)
    let blocks_per_year = (365.25 * 24.0 * 3600.0 / 75.0) as u64;
    let block_after_1yr = 100 + blocks_per_year;

    registry.accrue_fees(id, block_after_1yr).unwrap();

    let debt_after = registry.get_vault(id).unwrap().debt_zai;

    // With 2% annual rate, compounded per-block:
    // debt ≈ 1000 * (1 + 0.02/420768)^420768 ≈ 1000 * e^0.02 ≈ 1020.20
    let expected = 1000.0 * (0.02_f64).exp(); // continuous compounding approximation
    assert_relative_eq!(debt_after, expected, epsilon = 0.1);

    // Debt should have increased
    assert!(debt_after > initial_debt);

    // total_debt should track
    assert_relative_eq!(registry.total_debt, debt_after, epsilon = 0.01);

    // Short period: 1000 blocks ≈ 20.8 hours
    let mut registry2 = VaultRegistry::new(default_config());
    let id2 = registry2
        .open_vault("bob", 100.0, 1000.0, 100, &amm)
        .unwrap();
    registry2.accrue_fees(id2, 1100).unwrap();

    let debt_short = registry2.get_vault(id2).unwrap().debt_zai;
    // 1000 blocks at 2% annual: tiny increase
    let rate_per_block = 0.02 / blocks_per_year as f64;
    let expected_short = 1000.0 * (1.0 + rate_per_block).powi(1000);
    assert_relative_eq!(debt_short, expected_short, epsilon = 0.001);
}

// ─── Test 5: Debt floor ─────────────────────────────────────────────────

#[test]
fn test_debt_floor() {
    let amm = setup_amm(100);
    let mut registry = VaultRegistry::new(default_config());

    // Can't open vault with debt below floor (100 ZAI)
    let result = registry.open_vault("alice", 10.0, 50.0, 100, &amm);
    assert!(result.is_err(), "Should reject debt below floor");
    assert!(result.unwrap_err().contains("below floor"));

    // Zero debt is fine (collateral-only vault)
    let id_zero = registry.open_vault("alice", 10.0, 0.0, 100, &amm).unwrap();
    assert_relative_eq!(registry.get_vault(id_zero).unwrap().debt_zai, 0.0);

    // Open vault at floor
    let id = registry
        .open_vault("bob", 10.0, 100.0, 100, &amm)
        .unwrap();

    // Can't partially repay below floor: repay 50 → leaves 50 < 100
    let result = registry.repay_zai(id, 50.0, 100);
    assert!(result.is_err(), "Partial repay below floor should fail");
    assert!(result.unwrap_err().contains("below floor"));

    // Full repayment to zero is always allowed
    let result = registry.repay_zai(id, 100.0, 100);
    assert!(result.is_ok(), "Full repayment to zero should work");
    assert_relative_eq!(registry.get_vault(id).unwrap().debt_zai, 0.0);
}

// ─── Test 6: Liquidation penalty ────────────────────────────────────────

#[test]
fn test_liquidation_penalty() {
    let amm = setup_amm(100);
    let mut registry = VaultRegistry::new(default_config());

    // Vault with 1000 ZAI debt
    let id = registry
        .open_vault("alice", 100.0, 1000.0, 100, &amm)
        .unwrap();

    // Penalty = debt * liquidation_penalty = 1000 * 0.13 = 130
    let penalty = registry.liquidation_penalty_amount(id).unwrap();
    assert_relative_eq!(penalty, 130.0);

    // Larger debt
    let id2 = registry
        .open_vault("bob", 200.0, 5000.0, 100, &amm)
        .unwrap();
    let penalty2 = registry.liquidation_penalty_amount(id2).unwrap();
    assert_relative_eq!(penalty2, 650.0); // 5000 * 0.13

    // After fee accrual, penalty should increase
    let blocks_per_year = (365.25 * 24.0 * 3600.0 / 75.0) as u64;
    registry.accrue_fees(id, 100 + blocks_per_year).unwrap();
    let penalty_after = registry.liquidation_penalty_amount(id).unwrap();
    assert!(
        penalty_after > penalty,
        "Penalty should increase after fee accrual"
    );
}

// ─── Test 7: Deposit and withdraw collateral ────────────────────────────

#[test]
fn test_deposit_withdraw_collateral() {
    let amm = setup_amm(100);
    let mut registry = VaultRegistry::new(default_config());

    let id = registry
        .open_vault("alice", 10.0, 200.0, 100, &amm)
        .unwrap();

    // Deposit more collateral
    registry.deposit_collateral(id, 5.0).unwrap();
    assert_relative_eq!(registry.get_vault(id).unwrap().collateral_zec, 15.0);

    // Ratio improved: (15 * 50) / 200 = 3.75
    let price = amm.get_twap(48);
    let ratio = registry.get_vault(id).unwrap().collateral_ratio(price);
    assert_relative_eq!(ratio, 3.75, epsilon = 0.01);

    // Withdraw some (keeping above min ratio)
    // Withdraw 5 → 10 ZEC → (10*50)/200 = 2.5 > 1.5
    registry.withdraw_collateral(id, 5.0, 100, &amm).unwrap();
    assert_relative_eq!(registry.get_vault(id).unwrap().collateral_zec, 10.0);

    // Can't withdraw more than available
    let result = registry.withdraw_collateral(id, 20.0, 100, &amm);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Insufficient collateral"));

    // Can't deposit zero or negative
    assert!(registry.deposit_collateral(id, 0.0).is_err());
    assert!(registry.deposit_collateral(id, -1.0).is_err());
}

// ─── Test 8: Borrow and repay ───────────────────────────────────────────

#[test]
fn test_borrow_repay() {
    let amm = setup_amm(100);
    let mut registry = VaultRegistry::new(default_config());

    // Open with low initial debt, lots of collateral
    // 100 ZEC ($5000), 200 ZAI debt → ratio = 25.0
    let id = registry
        .open_vault("alice", 100.0, 200.0, 100, &amm)
        .unwrap();

    // Borrow more: 1000 ZAI → total 1200 → (100*50)/1200 ≈ 4.17
    registry.borrow_zai(id, 1000.0, 100, &amm).unwrap();
    assert_relative_eq!(registry.get_vault(id).unwrap().debt_zai, 1200.0);
    assert_relative_eq!(registry.total_debt, 1200.0);

    // Borrow up to the limit: max debt = (100*50)/1.5 = 3333.33
    // Currently at 1200, can borrow up to ~2133 more
    let result = registry.borrow_zai(id, 2000.0, 100, &amm);
    assert!(result.is_ok());
    let total_debt_now = registry.get_vault(id).unwrap().debt_zai;
    assert_relative_eq!(total_debt_now, 3200.0);

    // Can't borrow beyond limit
    let result = registry.borrow_zai(id, 500.0, 100, &amm);
    assert!(result.is_err());

    // Repay some: 1000 ZAI → 2200 remaining (above floor)
    registry.repay_zai(id, 1000.0, 100).unwrap();
    assert_relative_eq!(registry.get_vault(id).unwrap().debt_zai, 2200.0);

    // total_debt tracks
    assert_relative_eq!(registry.total_debt, 2200.0);

    // Can't repay more than owed
    let result = registry.repay_zai(id, 5000.0, 100);
    assert!(result.is_err());
}

// ─── Test 9: Liquidatable detection with price change ───────────────────

#[test]
fn test_liquidatable_after_price_drop() {
    let mut amm = Amm::new(10000.0, 500000.0, 0.003); // spot = $50
    for b in 1..=100 {
        amm.record_price(b);
    }

    let mut registry = VaultRegistry::new(default_config());

    // Vault at ratio ~1.67: 10 ZEC ($500), 300 ZAI
    let id = registry
        .open_vault("alice", 10.0, 300.0, 100, &amm)
        .unwrap();
    assert!(!registry.is_liquidatable(id, &amm));

    // Crash ZEC price via large swap: sell lots of ZEC for ZAI
    let _out = amm.swap_zec_for_zai(5000.0, 101).unwrap();
    // Record for enough blocks to shift TWAP
    for b in 102..=200 {
        amm.record_price(b);
    }

    // TWAP should have dropped significantly
    let new_twap = amm.get_twap(48);
    assert!(new_twap < 50.0, "TWAP should drop after ZEC sell-off");

    // Vault should now be liquidatable if TWAP dropped enough
    // New ratio = (10 * new_twap) / 300
    let vault = registry.get_vault(id).unwrap();
    let new_ratio = vault.collateral_ratio(new_twap);

    if new_ratio < 1.5 {
        assert!(registry.is_liquidatable(id, &amm));
    }
}

// ─── Test 10: Config variations ─────────────────────────────────────────

#[test]
fn test_configurable_parameters() {
    let amm = setup_amm(100);

    // Stricter config: 200% min ratio, higher penalty, higher floor
    let config = CdpConfig {
        min_ratio: 2.0,
        liquidation_penalty: 0.20,
        debt_floor: 500.0,
        stability_fee_rate: 0.05,
        twap_window: 96,
    };

    let mut registry = VaultRegistry::new(config);

    // 10 ZEC ($500), 200 ZAI → ratio 2.5 (above 2.0) but below debt floor
    let result = registry.open_vault("alice", 10.0, 200.0, 100, &amm);
    assert!(result.is_err(), "Debt below floor of 500");

    // 10 ZEC ($500), 250 ZAI → ratio 2.0 (exactly min), at floor
    // But 250 < 500 floor
    let result = registry.open_vault("alice", 10.0, 250.0, 100, &amm);
    assert!(result.is_err());

    // 20 ZEC ($1000), 500 ZAI → ratio 2.0 (exactly min), at floor
    let id = registry
        .open_vault("alice", 20.0, 500.0, 100, &amm)
        .unwrap();

    // Penalty = 500 * 0.20 = 100
    let penalty = registry.liquidation_penalty_amount(id).unwrap();
    assert_relative_eq!(penalty, 100.0);

    // Higher stability fee: 5% annual
    let blocks_per_year = (365.25 * 24.0 * 3600.0 / 75.0) as u64;
    registry.accrue_fees(id, 100 + blocks_per_year).unwrap();
    let debt_after = registry.get_vault(id).unwrap().debt_zai;
    // ≈ 500 * e^0.05 ≈ 525.64
    let expected = 500.0 * (0.05_f64).exp();
    assert_relative_eq!(debt_after, expected, epsilon = 0.1);
}
