use approx::assert_relative_eq;
use zai_sim::agents::*;
use zai_sim::amm::Amm;
use zai_sim::cdp::{CdpConfig, VaultRegistry};

fn setup_amm(block: u64) -> Amm {
    let mut amm = Amm::new(10000.0, 500000.0, 0.003); // spot = $50
    for b in 1..=block {
        amm.record_price(b);
    }
    amm
}

// ═══════════════════════════════════════════════════════════════════════
// Arbitrageur tests
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_arber_buys_when_amm_cheap() {
    let mut amm = setup_amm(50);

    // Crash AMM price: sell lots of ZEC → AMM price drops below external
    let _out = amm.swap_zec_for_zai(2000.0, 51).unwrap();
    amm.record_price(51);
    let amm_price = amm.spot_price();

    let external_price = 50.0; // external is higher than AMM
    assert!(amm_price < external_price, "AMM should be cheaper");

    let mut arber = Arbitrageur::new(ArbitrageurConfig {
        arb_latency_buy_blocks: 0, // instant buys
        arb_threshold_pct: 0.5,
        ..ArbitrageurConfig::default()
    });

    let initial_zai = arber.zai_balance;
    let actions = arber.act(&mut amm, external_price, 52);

    // Should buy ZEC (sell ZAI) since AMM is cheap
    let bought = actions.iter().any(|a| matches!(a, AgentAction::BuyZec { .. }));
    assert!(bought, "Arber should buy ZEC when AMM is cheap");
    assert!(arber.zai_balance < initial_zai, "ZAI balance should decrease");
}

#[test]
fn test_arber_sells_when_amm_expensive() {
    let mut amm = setup_amm(50);

    // Pump AMM price: buy lots of ZEC → AMM price rises above external
    let _out = amm.swap_zai_for_zec(100000.0, 51).unwrap();
    amm.record_price(51);
    let amm_price = amm.spot_price();

    let external_price = 50.0;
    assert!(amm_price > external_price, "AMM should be more expensive");

    let mut arber = Arbitrageur::new(ArbitrageurConfig {
        arb_latency_sell_blocks: 0, // instant sells
        arb_threshold_pct: 0.5,
        ..ArbitrageurConfig::default()
    });

    let initial_zec = arber.zec_balance;
    let actions = arber.act(&mut amm, external_price, 52);

    let sold = actions.iter().any(|a| matches!(a, AgentAction::SellZec { .. }));
    assert!(sold, "Arber should sell ZEC when AMM is expensive");
    assert!(arber.zec_balance < initial_zec, "ZEC balance should decrease");
}

#[test]
fn test_arber_zai_depletion() {
    let mut amm = setup_amm(50);

    // Crash AMM price so arber keeps buying
    let _out = amm.swap_zec_for_zai(3000.0, 51).unwrap();
    amm.record_price(51);

    let mut arber = Arbitrageur::new(ArbitrageurConfig {
        initial_zai_balance: 500.0, // small balance
        initial_zec_balance: 100.0,
        arb_latency_buy_blocks: 0,
        arb_threshold_pct: 0.1,
        capital_replenish_rate: 0.0,
        ..ArbitrageurConfig::default()
    });

    let external_price = 50.0;
    let mut total_bought = 0;

    // Keep trading until ZAI is depleted
    for block in 52..=200 {
        let actions = arber.act(&mut amm, external_price, block);
        if actions.iter().any(|a| matches!(a, AgentAction::BuyZec { .. })) {
            total_bought += 1;
        }
        amm.record_price(block);
    }

    // Should have bought some times
    assert!(total_bought > 0, "Should have made some buys");

    // ZAI should be nearly depleted (each buy uses 10% of balance)
    assert!(
        arber.zai_balance < 50.0,
        "ZAI should be mostly depleted: {}",
        arber.zai_balance
    );
}

#[test]
fn test_arber_asymmetric_latency() {
    let mut amm = setup_amm(50);

    // Set AMM price above external so arber wants to sell ZEC
    let _out = amm.swap_zai_for_zec(100000.0, 51).unwrap();
    amm.record_price(51);

    let external_price = 50.0;

    let mut arber = Arbitrageur::new(ArbitrageurConfig {
        arb_latency_buy_blocks: 0,  // instant buys
        arb_latency_sell_blocks: 10, // 10-block delay for sells
        arb_threshold_pct: 0.5,
        ..ArbitrageurConfig::default()
    });

    // Act at block 52: should queue a sell (not execute immediately)
    let actions = arber.act(&mut amm, external_price, 52);
    let queued = actions.iter().any(|a| matches!(a, AgentAction::Queued { .. }));
    let sold = actions.iter().any(|a| matches!(a, AgentAction::SellZec { .. }));
    assert!(queued, "Sell should be queued due to latency");
    assert!(!sold, "Should NOT execute sell immediately");

    let zec_before = arber.zec_balance;

    // Act at blocks 53-61: trade should still be pending
    for block in 53..=61 {
        amm.record_price(block);
        arber.act(&mut amm, external_price, block);
    }
    // At block 61, pending trade execute_at = 62, so still pending
    assert_relative_eq!(arber.zec_balance, zec_before, epsilon = 1.0);

    // Act at block 62: pending trade should execute
    amm.record_price(62);
    let actions62 = arber.act(&mut amm, external_price, 62);
    let sold_now = actions62.iter().any(|a| matches!(a, AgentAction::SellZec { .. }));
    assert!(
        sold_now,
        "Sell should execute at block 62 (10 blocks after queuing at 52)"
    );
    assert!(arber.zec_balance < zec_before, "ZEC should decrease after sell");

    // Test that buys are instant (no latency)
    let mut amm2 = setup_amm(50);
    let _out = amm2.swap_zec_for_zai(3000.0, 51).unwrap();
    amm2.record_price(51);

    let mut arber2 = Arbitrageur::new(ArbitrageurConfig {
        arb_latency_buy_blocks: 0,
        arb_latency_sell_blocks: 10,
        arb_threshold_pct: 0.5,
        ..ArbitrageurConfig::default()
    });

    let zai_before = arber2.zai_balance;
    let actions_buy = arber2.act(&mut amm2, external_price, 52);
    let bought = actions_buy.iter().any(|a| matches!(a, AgentAction::BuyZec { .. }));
    assert!(bought, "Buy should be instant (0 latency)");
    assert!(arber2.zai_balance < zai_before, "ZAI should decrease immediately");
}

#[test]
fn test_arber_capital_replenish() {
    let mut amm = setup_amm(50);
    let _out = amm.swap_zec_for_zai(3000.0, 51).unwrap();
    amm.record_price(51);

    let mut arber = Arbitrageur::new(ArbitrageurConfig {
        initial_zai_balance: 100.0,
        arb_latency_buy_blocks: 0,
        arb_threshold_pct: 0.1,
        capital_replenish_rate: 10.0, // 10 ZAI per block
        ..ArbitrageurConfig::default()
    });

    // Drain ZAI
    for block in 52..=80 {
        arber.act(&mut amm, 50.0, block);
        amm.record_price(block);
    }

    // Balance should not reach zero because of replenishment
    assert!(
        arber.zai_balance > 0.0,
        "Capital replenishment should prevent full depletion: {}",
        arber.zai_balance
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Demand Agent tests
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_demand_base_buying() {
    let mut amm = setup_amm(50);
    let mut agent = DemandAgent::new(DemandAgentConfig::default());

    let initial_zec = agent.zec_balance;
    let redemption_price = 50.0;

    // Normal block: should buy ZAI at base rate
    let action = agent.act(&mut amm, redemption_price, 51);
    assert!(
        matches!(action, AgentAction::BuyZai { .. }),
        "Should buy ZAI each block"
    );
    assert!(agent.zec_balance < initial_zec, "ZEC should decrease");
    assert!(agent.zai_balance > 0.0, "ZAI should increase");
}

#[test]
fn test_demand_elasticity() {
    let mut amm = setup_amm(50);

    // Crash AMM price so ZAI appears cheap vs redemption_price
    let _out = amm.swap_zec_for_zai(2000.0, 51).unwrap();
    amm.record_price(51);
    let amm_price = amm.spot_price(); // < 50

    let redemption_price = 50.0;
    assert!(amm_price < redemption_price, "ZAI should be below par");

    let mut agent = DemandAgent::new(DemandAgentConfig {
        demand_elasticity: 0.1, // high elasticity
        demand_base_rate: 0.0,  // no base rate, only elasticity
        ..DemandAgentConfig::default()
    });

    let action = agent.act(&mut amm, redemption_price, 52);
    match action {
        AgentAction::BuyZai { zec_spent, .. } => {
            assert!(
                zec_spent > 0.0,
                "Should buy more when ZAI is below par"
            );
        }
        _ => panic!("Should have bought ZAI"),
    }
}

#[test]
fn test_demand_panic_sell() {
    let mut amm = setup_amm(50);

    let mut agent = DemandAgent::new(DemandAgentConfig {
        demand_elasticity: 0.0,   // disable elastic buying to isolate panic behavior
        demand_base_rate: 0.0,    // disable base buying
        demand_exit_threshold_pct: 5.0,
        demand_exit_window_blocks: 10,
        demand_panic_sell_fraction: 0.5,
        initial_zec_balance: 5000.0,
    });

    // Pre-fund agent with ZAI
    agent.zai_balance = 10000.0;

    // Crash AMM price hard: ZAI deviates > 5% from redemption price
    let _out = amm.swap_zec_for_zai(4000.0, 51).unwrap();
    amm.record_price(51);

    let redemption_price = 50.0;
    let amm_price = amm.spot_price();
    let deviation = ((redemption_price - amm_price) / redemption_price).abs() * 100.0;
    assert!(deviation > 5.0, "Need >5% deviation for test, got {:.1}%", deviation);

    // Record ZAI balance just before panic window starts
    let zai_before = agent.zai_balance;
    assert!(!agent.panicked, "Should not have panicked yet");

    // Run for exit window blocks with sustained deviation
    for block in 52..=62 {
        amm.record_price(block);
        agent.act(&mut amm, redemption_price, block);
    }

    // Panic should have triggered
    assert!(agent.panicked, "Agent should have panicked after sustained deviation");
    assert!(
        agent.zai_balance < zai_before,
        "Should have panic-sold ZAI: before={:.2}, after={:.2}",
        zai_before,
        agent.zai_balance
    );

    // Should have sold approximately 50% of balance
    let sold_fraction = 1.0 - agent.zai_balance / zai_before;
    assert!(
        sold_fraction > 0.4 && sold_fraction < 0.6,
        "Should sell ~50% of ZAI, sold {:.1}%",
        sold_fraction * 100.0
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Miner Agent tests
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_miner_immediate_sell() {
    let mut amm = setup_amm(50);
    let mut miner = MinerAgent::new(MinerAgentConfig {
        block_reward: 1.25,
        miner_sell_fraction: 0.5,
        miner_amm_fraction: 0.3,
        sell_immediately: true,
        ..MinerAgentConfig::default()
    });

    // Each block: receives 1.25 ZEC, sells 0.5*0.3 = 0.1875 on AMM
    let action = miner.act(&mut amm, 51);
    match action {
        AgentAction::MinerSell { zec_sold, zai_received } => {
            assert_relative_eq!(zec_sold, 0.1875, epsilon = 0.001);
            assert!(zai_received > 0.0);
        }
        _ => panic!("Should sell immediately"),
    }

    // ZEC balance = received - sold = 1.25 - 0.1875 = 1.0625
    assert_relative_eq!(miner.zec_balance, 1.0625, epsilon = 0.001);
}

#[test]
fn test_miner_batch_sell() {
    let mut amm = setup_amm(50);
    let mut miner = MinerAgent::new(MinerAgentConfig {
        block_reward: 1.25,
        miner_sell_fraction: 0.5,
        miner_amm_fraction: 1.0, // all through AMM
        sell_immediately: false,
        batch_interval: 10,
    });

    let mut sell_count = 0;
    for block in 51..=80 {
        let action = miner.act(&mut amm, block);
        if matches!(action, AgentAction::MinerSell { .. }) {
            sell_count += 1;
        }
        amm.record_price(block);
    }

    // 30 blocks with batch_interval=10 → should sell about 3 times
    assert!(
        sell_count >= 2 && sell_count <= 4,
        "Should batch sell 2-4 times in 30 blocks, got {}",
        sell_count
    );
}

// ═══════════════════════════════════════════════════════════════════════
// CDP Holder tests
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_cdp_holder_adds_collateral() {
    let amm = setup_amm(100);
    let mut registry = VaultRegistry::new(CdpConfig::default());
    let mut holder = CdpHolder::new(CdpHolderConfig {
        target_ratio: 2.5,
        action_threshold_ratio: 1.8,
        reserve_zec: 100.0,
        initial_collateral: 10.0,
        initial_debt: 300.0, // ratio = (10*50)/300 = 1.67
    });

    holder.open_vault(&mut registry, &amm, 100).unwrap();
    let vault_id = holder.vault_id.unwrap();

    // Ratio is 1.67 < 1.8 threshold → should add collateral
    let action = holder.act(&mut registry, &amm, 101);
    match action {
        AgentAction::CdpAction { description, .. } => {
            assert!(
                description.contains("added"),
                "Should add collateral: {}",
                description
            );
        }
        _ => panic!("Should take CDP action"),
    }

    // Ratio should be improved
    let price = amm.get_twap(48);
    let vault = registry.get_vault(vault_id).unwrap();
    let new_ratio = vault.collateral_ratio(price);
    assert!(
        new_ratio >= 2.0,
        "Ratio should have improved: {:.2}",
        new_ratio
    );
}

// ═══════════════════════════════════════════════════════════════════════
// LP Agent tests
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_lp_provide_and_withdraw() {
    let mut amm = setup_amm(50);
    let mut lp = LpAgent::new(LpAgentConfig {
        initial_zec: 500.0,
        initial_zai: 25000.0,
        il_threshold: 0.02, // low threshold → withdraws easily
        ..LpAgentConfig::default()
    });

    // Provide liquidity
    let action = lp.provide_liquidity(&mut amm);
    assert!(matches!(action, AgentAction::LpAdd { .. }));
    assert!(lp.is_providing);
    assert!(lp.shares > 0.0);

    // No withdrawal when price hasn't moved
    let action = lp.act(&mut amm);
    assert!(matches!(action, AgentAction::None));

    // Move price significantly to trigger IL
    let _out = amm.swap_zec_for_zai(3000.0, 51).unwrap();

    let action = lp.act(&mut amm);
    assert!(
        matches!(action, AgentAction::LpRemove { .. }),
        "LP should withdraw when IL exceeds threshold"
    );
    assert!(!lp.is_providing);
    assert!(lp.zec_balance > 0.0);
    assert!(lp.zai_balance > 0.0);
}

// ═══════════════════════════════════════════════════════════════════════
// Attacker tests
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_attacker_manipulate_and_revert() {
    let mut amm = setup_amm(99);
    let price_before = amm.spot_price();

    let mut attacker = Attacker::new(AttackerConfig {
        attack_capital_zec: 3000.0,
        hold_blocks: 3,
        attack_at_block: 100,
    });

    // Before attack: idle
    assert!(matches!(attacker.phase, AttackPhase::Idle));

    // Block 100: attack begins — dump ZEC
    let action = attacker.act(&mut amm, 100);
    assert!(matches!(action, AgentAction::AttackSwap { .. }));
    assert!(matches!(attacker.phase, AttackPhase::Manipulating { .. }));

    let price_during = amm.spot_price();
    assert!(
        price_during < price_before,
        "Price should drop during attack: before={}, during={}",
        price_before,
        price_during
    );

    // Blocks 101-102: holding
    amm.record_price(101);
    let action101 = attacker.act(&mut amm, 101);
    assert!(matches!(action101, AgentAction::None));

    amm.record_price(102);
    let action102 = attacker.act(&mut amm, 102);
    assert!(matches!(action102, AgentAction::None));

    // Block 103: revert — buy back ZEC
    amm.record_price(103);
    let action103 = attacker.act(&mut amm, 103);
    assert!(matches!(action103, AgentAction::AttackSwap { .. }));
    assert!(matches!(attacker.phase, AttackPhase::Done));

    // Price should recover (not exactly due to fees)
    let price_after = amm.spot_price();
    assert!(
        price_after > price_during,
        "Price should recover after revert: during={}, after={}",
        price_during,
        price_after
    );

    // Attacker loses money due to swap fees (round-trip cost)
    assert!(
        attacker.zec_balance < 3000.0,
        "Attacker should lose ZEC to fees: {}",
        attacker.zec_balance
    );
}

#[test]
fn test_attacker_does_nothing_before_trigger() {
    let mut amm = setup_amm(50);
    let mut attacker = Attacker::new(AttackerConfig {
        attack_at_block: 100,
        ..AttackerConfig::default()
    });

    for block in 51..=99 {
        let action = attacker.act(&mut amm, block);
        assert!(
            matches!(action, AgentAction::None),
            "Attacker should be idle before block 100"
        );
    }
    assert!(matches!(attacker.phase, AttackPhase::Idle));
}
