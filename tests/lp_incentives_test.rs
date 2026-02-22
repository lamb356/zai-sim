/// LP Incentive Mechanisms — Three approaches to make LP economics viable
///
/// Track 1A: Stability fee redistribution — route CDP fees to LPs
/// Track 1B: Protocol-owned liquidity — permanent LP that never withdraws
/// Track 1C: IL-aware LP withdrawal dynamics — realistic LP behavior over 50K blocks
///
/// F-027: Combined findings on LP incentive viability
use zai_sim::agents::*;
use zai_sim::controller::ControllerConfig;
use zai_sim::output;
use zai_sim::report;
use zai_sim::scenario::{Scenario, ScenarioConfig};
use zai_sim::scenarios::{generate_prices, ScenarioId};

const SEED: u64 = 42;

fn config_5m() -> ScenarioConfig {
    let mut config = ScenarioConfig::default();
    config.amm_initial_zec = 100_000.0;
    config.amm_initial_zai = 5_000_000.0;
    config.cdp_config.min_ratio = 2.0;
    config.cdp_config.twap_window = 240;
    config.controller_config = ControllerConfig::default_tick();
    config
}

// ═══════════════════════════════════════════════════════════════════════
// Track 1A: Stability Fee Redistribution
// ═══════════════════════════════════════════════════════════════════════

struct StabilityFeeResult {
    _label: String,
    _stability_fee_to_lps: bool,
    lp_fee_share: f64,
    _lp_initial_value: f64,
    _lp_pool_value: f64,
    lp_net_pnl: f64,
    cumulative_fees: f64,
    _stability_fees_routed: f64,
}

fn run_stability_fee_test(
    sid: ScenarioId,
    stability_fee_to_lps: bool,
    label: &str,
) -> StabilityFeeResult {
    let mut config = config_5m();
    config.stability_fee_to_lps = stability_fee_to_lps;
    let entry_price = config.initial_redemption_price;

    let blocks = 1000;
    let prices = generate_prices(sid, blocks, SEED);
    let mut scenario = Scenario::new(&config);

    // LP deposits $100K: 1000 ZEC + 50000 ZAI
    let lp_zec = 1000.0;
    let lp_zai = 50000.0;
    let lp_shares = scenario
        .amm
        .add_liquidity(lp_zec, lp_zai, "test_lp")
        .expect("LP deposit");

    // Standard agents
    scenario
        .arbers
        .push(Arbitrageur::new(ArbitrageurConfig::default()));
    scenario
        .miners
        .push(MinerAgent::new(MinerAgentConfig::default()));

    // 10 CDP holders × $2000 ZAI debt
    for _i in 0..10 {
        let holder_config = CdpHolderConfig {
            initial_collateral: 200.0,
            initial_debt: 2000.0,
            target_ratio: 3.0,
            action_threshold_ratio: 2.2,
            reserve_zec: 50.0,
        };
        scenario.cdp_holders.push(CdpHolder::new(holder_config));
    }

    // Track AMM fees before run
    let fees_before = scenario.amm.cumulative_fees_zai;

    scenario.run(&prices);

    // Compute LP economics
    let total_shares = scenario.amm.total_lp_shares;
    let lp_fraction = lp_shares / total_shares;

    let zec_out = scenario.amm.reserve_zec * lp_fraction;
    let zai_out = scenario.amm.reserve_zai * lp_fraction;

    let last = scenario.metrics.last().unwrap();
    let final_ext = last.external_price;

    let initial_value = lp_zec * entry_price + lp_zai;
    let pool_value = zec_out * final_ext + zai_out;
    let cumulative_fees = scenario.amm.cumulative_fees_zai;
    let lp_fee_share = cumulative_fees * lp_fraction;
    let stability_fees_routed = cumulative_fees - fees_before;

    StabilityFeeResult {
        _label: label.to_string(),
        _stability_fee_to_lps: stability_fee_to_lps,
        lp_fee_share,
        _lp_initial_value: initial_value,
        _lp_pool_value: pool_value,
        lp_net_pnl: pool_value - initial_value,
        cumulative_fees,
        _stability_fees_routed: stability_fees_routed,
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Track 1B: Protocol-Owned Liquidity
// ═══════════════════════════════════════════════════════════════════════

struct ProtocolLpResult {
    label: String,
    _protocol_fraction: f64,
    total_pool_zai_initial: f64,
    pool_zai_after_withdrawal: f64,
    _pool_zec_after_withdrawal: f64,
    mvl_after: f64,
    above_2m: bool,
    verdict: String,
    mean_peg: f64,
}

fn run_protocol_lp_test(protocol_fraction: f64, label: &str) -> ProtocolLpResult {
    // Tiny genesis pool — LPs provide the real liquidity
    let mut config = ScenarioConfig::default();
    config.amm_initial_zec = 100.0;
    config.amm_initial_zai = 5000.0;
    config.amm_swap_fee = 0.003;
    config.cdp_config.min_ratio = 2.0;
    config.cdp_config.twap_window = 240;
    config.controller_config = ControllerConfig::default_tick();
    let target = config.initial_redemption_price;

    let blocks = 1000;
    let prices = generate_prices(ScenarioId::LiquidityCrisis, blocks, SEED);
    let mut scenario = Scenario::new(&config);

    // Total target: 100K ZEC + 5M ZAI ($5M pool)
    // Protocol LP: permanent, never withdraws
    let protocol_zec = 100_000.0 * protocol_fraction;
    let protocol_zai = 5_000_000.0 * protocol_fraction;
    if protocol_fraction > 0.0 {
        let _ = scenario
            .amm
            .add_liquidity(protocol_zec, protocol_zai, "protocol_lp");
    }

    // Private IL-aware LPs: withdraw when real P&L < -2%
    let private_total_fraction = 1.0 - protocol_fraction;
    let private_count = 5;
    if private_total_fraction > 0.01 {
        let per_lp_fraction = private_total_fraction / private_count as f64;
        for i in 0..private_count {
            let lp_config = IlAwareLpConfig {
                initial_zec: 100_000.0 * per_lp_fraction,
                initial_zai: 5_000_000.0 * per_lp_fraction,
                withdrawal_threshold: -0.02, // -2% P&L triggers withdrawal
                withdrawal_rate: 0.10,
            };
            scenario
                .il_aware_lps
                .push(IlAwareLpAgent::new(lp_config, &format!("private_lp_{}", i)));
        }
    }

    // Standard agents
    scenario
        .arbers
        .push(Arbitrageur::new(ArbitrageurConfig::default()));
    scenario
        .miners
        .push(MinerAgent::new(MinerAgentConfig::default()));

    let total_initial_zai = scenario.amm.reserve_zai;

    scenario.run(&prices);

    let summary = output::compute_summary(&scenario.metrics, target);
    let verdict = report::evaluate_pass_fail(&scenario.metrics, target);

    let pool_zec = scenario.amm.reserve_zec;
    let pool_zai = scenario.amm.reserve_zai;
    let last = scenario.metrics.last().unwrap();
    let mvl = pool_zec * last.external_price + pool_zai;

    // Count how many private LPs still providing
    let _private_providing = scenario
        .il_aware_lps
        .iter()
        .filter(|lp| lp.is_providing)
        .count();

    ProtocolLpResult {
        label: label.to_string(),
        _protocol_fraction: protocol_fraction,
        total_pool_zai_initial: total_initial_zai,
        pool_zai_after_withdrawal: pool_zai,
        _pool_zec_after_withdrawal: pool_zec,
        mvl_after: mvl,
        above_2m: mvl >= 2_000_000.0,
        verdict: verdict.overall.label().to_string(),
        mean_peg: summary.mean_peg_deviation,
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Track 1C: IL-Aware LP Withdrawal Dynamics
// ═══════════════════════════════════════════════════════════════════════

struct IlAwareLpResult {
    blocks: usize,
    blocks_until_below_2m: Option<u64>,
    lps_still_providing: u32,
    final_pool_zec: f64,
    final_pool_zai: f64,
    final_mvl: f64,
    total_withdrawn_zec: f64,
    total_withdrawn_zai: f64,
    mean_peg: f64,
    max_peg: f64,
    verdict: String,
}

fn run_il_aware_lp_test(blocks: usize) -> IlAwareLpResult {
    // Create scenario with minimal genesis, LPs provide the real liquidity
    let mut config = ScenarioConfig::default();
    // Small genesis so LPs are the real liquidity
    config.amm_initial_zec = 100.0;
    config.amm_initial_zai = 5000.0;
    config.amm_swap_fee = 0.003;
    config.cdp_config.min_ratio = 2.0;
    config.cdp_config.twap_window = 240;
    config.controller_config = ControllerConfig::default_tick();
    let target = config.initial_redemption_price;

    let prices = generate_prices(ScenarioId::SustainedBear, blocks, SEED);
    let mut scenario = Scenario::new(&config);

    // 10 IL-aware LPs × $500K each (10000 ZEC + 500000 ZAI at $50/ZEC)
    for i in 0..10 {
        let lp_config = IlAwareLpConfig {
            initial_zec: 10000.0,
            initial_zai: 500000.0,
            withdrawal_threshold: -0.02,
            withdrawal_rate: 0.10,
        };
        scenario
            .il_aware_lps
            .push(IlAwareLpAgent::new(lp_config, &format!("il_lp_{}", i)));
    }

    // Standard agents
    scenario
        .arbers
        .push(Arbitrageur::new(ArbitrageurConfig::default()));
    scenario
        .miners
        .push(MinerAgent::new(MinerAgentConfig::default()));

    scenario.run(&prices);

    // Find block where pool drops below $2M MVL
    let mut blocks_until_below_2m: Option<u64> = None;
    for m in &scenario.metrics {
        let mvl = m.amm_reserve_zec * m.external_price + m.amm_reserve_zai;
        if mvl < 2_000_000.0 && blocks_until_below_2m.is_none() {
            blocks_until_below_2m = Some(m.block);
        }
    }

    let lps_providing = scenario
        .il_aware_lps
        .iter()
        .filter(|lp| lp.is_providing)
        .count() as u32;

    let total_withdrawn_zec: f64 = scenario
        .il_aware_lps
        .iter()
        .map(|lp| lp.withdrawn_zec)
        .sum();
    let total_withdrawn_zai: f64 = scenario
        .il_aware_lps
        .iter()
        .map(|lp| lp.withdrawn_zai)
        .sum();

    let last = scenario.metrics.last().unwrap();
    let final_mvl =
        scenario.amm.reserve_zec * last.external_price + scenario.amm.reserve_zai;

    let summary = output::compute_summary(&scenario.metrics, target);
    let verdict = report::evaluate_pass_fail(&scenario.metrics, target);

    IlAwareLpResult {
        blocks,
        blocks_until_below_2m,
        lps_still_providing: lps_providing,
        final_pool_zec: scenario.amm.reserve_zec,
        final_pool_zai: scenario.amm.reserve_zai,
        final_mvl,
        total_withdrawn_zec,
        total_withdrawn_zai,
        mean_peg: summary.mean_peg_deviation,
        max_peg: summary.max_peg_deviation,
        verdict: verdict.overall.label().to_string(),
    }
}

#[test]
fn lp_incentives() {
    println!(
        "\n═══════════════════════════════════════════════════════════════════════════════════════════════════"
    );
    println!("  LP INCENTIVE MECHANISMS — Three Approaches to Viable LP Economics");
    println!("  Config: $5M AMM, 200% CR, Tick, 240-block TWAP");
    println!(
        "═══════════════════════════════════════════════════════════════════════════════════════════════════\n"
    );

    // ── Track 1A: Stability Fee Redistribution ──────────────────────────
    println!(
        "  ┌─ Track 1A: Stability Fee Redistribution ───────────────────────────"
    );
    println!("  │ 10 vaults × $2000 ZAI × 2% annual = $400/year in stability fees");
    println!("  │ LP deposit: 1000 ZEC + 50,000 ZAI = $100,000 at $50/ZEC\n");

    let scenarios = [ScenarioId::BlackThursday, ScenarioId::SteadyState];
    println!(
        "  │ {:<20} {:<10} {:>12} {:>12} {:>12} {:>12}",
        "Scenario", "FeesToLPs", "PoolFees", "LPFeeShare", "NetPnL", "Delta"
    );
    println!("  │ {}", "─".repeat(80));

    for &sid in &scenarios {
        let baseline = run_stability_fee_test(sid, false, "without");
        let with_fees = run_stability_fee_test(sid, true, "with");
        let delta = with_fees.lp_net_pnl - baseline.lp_net_pnl;

        println!(
            "  │ {:<20} {:<10} ${:>10.2} ${:>10.2} ${:>+10.2}",
            sid.name(),
            "false",
            baseline.cumulative_fees,
            baseline.lp_fee_share,
            baseline.lp_net_pnl,
        );
        println!(
            "  │ {:<20} {:<10} ${:>10.2} ${:>10.2} ${:>+10.2} {:>+10.2}",
            "",
            "true",
            with_fees.cumulative_fees,
            with_fees.lp_fee_share,
            with_fees.lp_net_pnl,
            delta,
        );
    }

    println!(
        "  │\n  │ Note: 10 vaults × $2000 × 2%/year over 1000 blocks (20.8h) ="
    );
    println!("  │ $400 × (20.8/8766) = $0.95 in stability fees — economically trivial.");
    println!(
        "  └──────────────────────────────────────────────────────────────────\n"
    );

    // ── Track 1B: Protocol-Owned Liquidity ──────────────────────────────
    println!(
        "  ┌─ Track 1B: Protocol-Owned Liquidity ─────────────────────────────"
    );
    println!("  │ liquidity_crisis scenario, 5 private LPs with 1% IL threshold");
    println!("  │ Question: what % protocol-owned keeps pool above $2M MVL?\n");

    let fractions = [
        ("0%", 0.0),
        ("25%", 0.25),
        ("50%", 0.50),
        ("75%", 0.75),
    ];

    println!(
        "  │ {:<8} {:>12} {:>12} {:>12} {:>8} {:>8} {:>8}",
        "Protocol", "InitPoolZAI", "FinalPoolZAI", "FinalMVL", "Above2M", "Verdict", "MeanPeg"
    );
    println!("  │ {}", "─".repeat(80));

    for &(label, fraction) in &fractions {
        let r = run_protocol_lp_test(fraction, label);
        println!(
            "  │ {:<8} ${:>10.0} ${:>10.0} ${:>10.0} {:>8} {:>8} {:>7.4}%",
            r.label,
            r.total_pool_zai_initial,
            r.pool_zai_after_withdrawal,
            r.mvl_after,
            if r.above_2m { "YES" } else { "NO" },
            r.verdict,
            r.mean_peg * 100.0,
        );
    }

    println!(
        "  └──────────────────────────────────────────────────────────────────\n"
    );

    // ── Track 1C: IL-Aware LP Withdrawal Dynamics ───────────────────────
    println!(
        "  ┌─ Track 1C: IL-Aware LP Withdrawal Dynamics ──────────────────────"
    );
    println!("  │ sustained_bear 50,000 blocks (43 days)");
    println!("  │ 10 LPs × $500K each, -2% P&L threshold, 10% withdrawal rate\n");

    let result = run_il_aware_lp_test(50000);

    println!(
        "  │ Blocks simulated:      {}  ({:.1} days)",
        result.blocks,
        result.blocks as f64 * 75.0 / 86400.0
    );
    match result.blocks_until_below_2m {
        Some(block) => println!(
            "  │ Pool < $2M MVL at:     block {}  ({:.1} hours / {:.1} days)",
            block,
            block as f64 * 75.0 / 3600.0,
            block as f64 * 75.0 / 86400.0,
        ),
        None => println!("  │ Pool < $2M MVL:        NEVER (stayed above $2M throughout)"),
    }
    println!(
        "  │ LPs still providing:   {} / 10",
        result.lps_still_providing
    );
    println!(
        "  │ Final pool:            {:.0} ZEC + {:.0} ZAI",
        result.final_pool_zec, result.final_pool_zai,
    );
    println!(
        "  │ Final MVL:             ${:.0}",
        result.final_mvl
    );
    println!(
        "  │ Total withdrawn:       {:.0} ZEC + {:.0} ZAI",
        result.total_withdrawn_zec, result.total_withdrawn_zai,
    );
    println!(
        "  │ Mean peg deviation:    {:.4}%",
        result.mean_peg * 100.0
    );
    println!(
        "  │ Max peg deviation:     {:.4}%",
        result.max_peg * 100.0
    );
    println!("  │ Verdict:               {}", result.verdict);

    println!(
        "  └──────────────────────────────────────────────────────────────────\n"
    );

    // ── Summary ─────────────────────────────────────────────────────────
    println!("  ── SUMMARY ──────────────────────────────────────────────────────");
    println!("  Track 1A: Stability fees add ~$0.95 over 20.8h — economically negligible.");
    println!("            Even at scale ($400/year on $5M pool), this is 0.008% APR.");
    println!("  Track 1B: Protocol-owned liquidity provides a guaranteed floor.");
    println!("  Track 1C: IL-aware LPs reveal the realistic failure timeline.");
    println!(
        "  ════════════════════════════════════════════════════════════════════\n"
    );
}
