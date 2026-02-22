use approx::assert_relative_eq;
use zai_sim::amm::Amm;

#[test]
fn test_constant_product_invariant() {
    let mut amm = Amm::new(1000.0, 50000.0, 0.003);
    let initial_k = amm.k;

    // Single swap
    let _out = amm.swap_zec_for_zai(10.0, 1).unwrap();
    assert!(
        amm.reserve_zec * amm.reserve_zai >= initial_k,
        "k must not decrease after swap (fees increase k)"
    );

    // Multiple swaps in both directions
    let mut prev_k = amm.k;
    for i in 2..10 {
        let _out = amm.swap_zai_for_zec(500.0, i).unwrap();
        assert!(
            amm.reserve_zec * amm.reserve_zai >= prev_k - 1e-6,
            "k must not decrease: block {}",
            i
        );
        prev_k = amm.k;
    }

    for i in 10..20 {
        let _out = amm.swap_zec_for_zai(5.0, i).unwrap();
        assert!(
            amm.reserve_zec * amm.reserve_zai >= prev_k - 1e-6,
            "k must not decrease: block {}",
            i
        );
        prev_k = amm.k;
    }
}

#[test]
fn test_swap_price_impact() {
    // Small swap: 0.1% of reserves → price impact < 0.2%
    let mut amm = Amm::new(1000.0, 50000.0, 0.003);
    let spot_before = amm.spot_price();
    let zec_in_small = 1.0; // 0.1% of 1000
    let zai_out = amm.swap_zec_for_zai(zec_in_small, 1).unwrap();

    // Pure slippage = compare actual output to zero-slippage output (accounting for fee)
    // Zero-slippage output = input * (1 - fee) * spot_price
    let ideal_output_small = zec_in_small * (1.0 - 0.003) * spot_before;
    let slippage_small = 1.0 - zai_out / ideal_output_small;
    assert!(
        slippage_small < 0.002,
        "Small swap slippage should be < 0.2%, got {:.4}%",
        slippage_small * 100.0
    );

    // Large swap: 10% of reserves → price impact ~19%
    let mut amm2 = Amm::new(1000.0, 50000.0, 0.003);
    let spot_before2 = amm2.spot_price();
    let zec_in_large = 100.0; // 10% of 1000
    let zai_out_large = amm2.swap_zec_for_zai(zec_in_large, 1).unwrap();

    let ideal_output_large = zec_in_large * (1.0 - 0.003) * spot_before2;
    let slippage_large = 1.0 - zai_out_large / ideal_output_large;

    // For constant product: ~9% slippage for 10% of reserves
    // (effective input 99.7 ZEC into 1000 ZEC pool → ~9% slippage)
    assert!(
        slippage_large > 0.05,
        "Large swap should have significant slippage, got {:.4}%",
        slippage_large * 100.0
    );
    assert!(
        slippage_large < 0.20,
        "Slippage should be reasonable, got {:.4}%",
        slippage_large * 100.0
    );
}

#[test]
fn test_twap_steady_state() {
    let mut amm = Amm::new(1000.0, 50000.0, 0.003);
    let spot = amm.spot_price();

    // Record price for 100 blocks with no trades
    for block in 1..=100 {
        amm.record_price(block);
    }

    let twap = amm.get_twap(100);
    assert_relative_eq!(twap, spot, epsilon = 1e-10);
}

#[test]
fn test_twap_after_price_change() {
    let mut amm = Amm::new(1000.0, 50000.0, 0.003);
    let p1 = amm.spot_price();

    // Record price for 50 blocks at P1
    for block in 1..=50 {
        amm.record_price(block);
    }

    // Large swap to change price
    let _out = amm.swap_zec_for_zai(200.0, 51).unwrap();
    let p2 = amm.spot_price();

    // Record price for 50 more blocks at P2
    for block in 52..=100 {
        amm.record_price(block);
    }

    let twap = amm.get_twap(100);
    // The TWAP should be a weighted average of P1 and P2
    // P1 held for blocks 0→50 (50 blocks), P2 held for blocks 51→100 (49 blocks)
    // Plus 1 block at block 51 where the swap happened (recorded at P1 before swap)
    // Expected: (P1 * 51 + P2 * 49) / 100
    let expected = (p1 * 51.0 + p2 * 49.0) / 100.0;

    assert_relative_eq!(twap, expected, epsilon = 0.01);
}

#[test]
fn test_twap_manipulation_resistance() {
    let mut amm = Amm::new(10000.0, 500000.0, 0.003);
    let base_price = amm.spot_price();

    // Record for 47 blocks at base price
    for block in 1..=47 {
        amm.record_price(block);
    }

    // Manipulate: large swap to double the price
    // To double ZAI/ZEC price, we need to add enough ZEC to halve the ZAI reserves relative to ZEC
    // We'll do a large swap and then immediately revert
    let _saved_zec = amm.reserve_zec;
    let _saved_zai = amm.reserve_zai;
    let _saved_k = amm.k;

    // Big swap to move price
    let zai_received = amm.swap_zec_for_zai(5000.0, 48).unwrap();
    let manipulated_price = amm.spot_price();
    assert!(
        manipulated_price < base_price,
        "Price should drop after selling ZEC for ZAI"
    );

    // Record manipulated price for 1 block
    amm.record_price(48);

    // Revert: swap back
    let _zec_back = amm.swap_zai_for_zec(zai_received, 49).unwrap();
    amm.record_price(49);

    // Record for remaining blocks to fill 48-block window
    for block in 50..=96 {
        amm.record_price(block);
    }

    let twap = amm.get_twap(48);
    let current_price = amm.spot_price();

    // TWAP should be close to current spot, not the manipulated price
    // The manipulation lasted ~1 block out of 48, so impact should be < 1/48 ≈ 2.1%
    let twap_deviation = (twap - current_price).abs() / current_price;
    assert!(
        twap_deviation < 0.021 + 0.01, // small tolerance for rounding from swap fees
        "TWAP should resist manipulation. Deviation: {:.4}%, expected < 3.1%",
        twap_deviation * 100.0
    );
}

#[test]
fn test_add_remove_liquidity() {
    let mut amm = Amm::new(1000.0, 50000.0, 0.003);
    let price_before = amm.spot_price();

    // Add liquidity proportionally
    let shares = amm.add_liquidity(100.0, 5000.0, "alice").unwrap();
    assert!(shares > 0.0, "Should receive positive LP shares");

    // Spot price unchanged after proportional add
    let price_after_add = amm.spot_price();
    assert_relative_eq!(price_before, price_after_add, epsilon = 1e-10);

    // Reserves increased
    assert_relative_eq!(amm.reserve_zec, 1100.0, epsilon = 1e-10);
    assert_relative_eq!(amm.reserve_zai, 55000.0, epsilon = 1e-10);

    // Remove liquidity
    let (zec_out, zai_out) = amm.remove_liquidity(shares, "alice").unwrap();
    let price_after_remove = amm.spot_price();
    assert_relative_eq!(price_before, price_after_remove, epsilon = 1e-10);

    // Should get back approximately what was put in
    assert_relative_eq!(zec_out, 100.0, epsilon = 1e-6);
    assert_relative_eq!(zai_out, 5000.0, epsilon = 1e-6);

    // Multiple LPs
    let shares_bob = amm.add_liquidity(200.0, 10000.0, "bob").unwrap();
    let shares_carol = amm.add_liquidity(100.0, 5000.0, "carol").unwrap();

    // Bob added 2x Carol → should have ~2x shares
    assert_relative_eq!(shares_bob / shares_carol, 2.0, epsilon = 1e-6);
}

#[test]
fn test_swap_fee_collection() {
    // With fee
    let mut amm_fee = Amm::new(1000.0, 50000.0, 0.003);
    let k_before = amm_fee.k;
    let out_with_fee = amm_fee.swap_zec_for_zai(100.0, 1).unwrap();

    // k increases after swap (fee stays in pool)
    assert!(
        amm_fee.k > k_before,
        "k should increase due to fees: before={}, after={}",
        k_before,
        amm_fee.k
    );

    // Without fee
    let mut amm_no_fee = Amm::new(1000.0, 50000.0, 0.0);
    let out_no_fee = amm_no_fee.swap_zec_for_zai(100.0, 1).unwrap();

    // Output with fee should be less than without fee
    assert!(
        out_with_fee < out_no_fee,
        "Fee should reduce output: with_fee={}, no_fee={}",
        out_with_fee,
        out_no_fee
    );

    // Effective input should be 99.7 ZEC (0.3% fee on 100)
    // Verify the fee magnitude is correct
    let fee_impact = 1.0 - out_with_fee / out_no_fee;
    assert!(
        fee_impact > 0.002 && fee_impact < 0.005,
        "Fee impact should be ~0.3%, got {:.4}%",
        fee_impact * 100.0
    );
}
