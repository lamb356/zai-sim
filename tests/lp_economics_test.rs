/// LP Economics — Impermanent Loss, Fees, and Net P&L
///
/// Tracks a $100K LP who provides liquidity at launch:
///   - Deposits 1000 ZEC + 50000 ZAI (proportional to $5M pool)
///   - Runs Black Thursday and sustained_bear scenarios
///   - Computes IL, cumulative fees, and net P&L
///
/// The question: does an LP who provides $100K at launch make or lose money?
use zai_sim::agents::*;
use zai_sim::controller::ControllerConfig;
use zai_sim::scenario::{Scenario, ScenarioConfig};
use zai_sim::scenarios::{generate_prices, ScenarioId};

const BLOCKS: usize = 1000;
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

struct LpEconomics {
    scenario_name: String,
    entry_price: f64,
    final_external_price: f64,
    final_amm_price: f64,
    // LP position
    _lp_zec_deposited: f64,
    _lp_zai_deposited: f64,
    lp_shares: f64,
    lp_share_fraction: f64,
    // Values at exit (in ZAI terms, ≈ USD)
    initial_value: f64,
    hold_value_at_external: f64,
    _hold_value_at_amm: f64,
    pool_value_at_external: f64,
    _pool_value_at_amm: f64,
    // Breakdown
    il_pct: f64,
    cumulative_fees: f64,
    lp_fee_share: f64,
    // Net P&L
    net_pnl_external: f64,
    net_pnl_amm: f64,
}

fn run_lp_economics(sid: ScenarioId) -> LpEconomics {
    let config = config_5m();
    let entry_price = config.initial_redemption_price; // $50

    let prices = generate_prices(sid, BLOCKS, SEED);
    let mut scenario = Scenario::new(&config);

    // LP deposits $100K: 1000 ZEC + 50000 ZAI (1% of pool)
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

    scenario.run(&prices);

    // Compute LP economics at end
    let last = scenario.metrics.last().unwrap();
    let final_ext = last.external_price;
    let final_amm = last.amm_spot_price;
    let total_shares = scenario.amm.total_lp_shares;
    let lp_fraction = lp_shares / total_shares;

    // What LP would get if they withdrew now
    let zec_out = scenario.amm.reserve_zec * lp_fraction;
    let zai_out = scenario.amm.reserve_zai * lp_fraction;

    let initial_value = lp_zec * entry_price + lp_zai; // 1000*50 + 50000 = 100000
    let hold_value_ext = lp_zec * final_ext + lp_zai;
    let hold_value_amm = lp_zec * final_amm + lp_zai;
    let pool_value_ext = zec_out * final_ext + zai_out;
    let pool_value_amm = zec_out * final_amm + zai_out;

    // Pure IL percentage (classic formula)
    let il_pct = scenario.amm.impermanent_loss(entry_price);

    // Fee share
    let cumulative_fees = scenario.amm.cumulative_fees_zai;
    let lp_fee_share = cumulative_fees * lp_fraction;

    LpEconomics {
        scenario_name: sid.name().to_string(),
        entry_price,
        final_external_price: final_ext,
        final_amm_price: final_amm,
        _lp_zec_deposited: lp_zec,
        _lp_zai_deposited: lp_zai,
        lp_shares,
        lp_share_fraction: lp_fraction,
        initial_value,
        hold_value_at_external: hold_value_ext,
        _hold_value_at_amm: hold_value_amm,
        pool_value_at_external: pool_value_ext,
        _pool_value_at_amm: pool_value_amm,
        il_pct,
        cumulative_fees,
        lp_fee_share,
        net_pnl_external: pool_value_ext - initial_value,
        net_pnl_amm: pool_value_amm - initial_value,
    }
}

#[test]
fn lp_economics() {
    println!(
        "\n═══════════════════════════════════════════════════════════════════════════════════════════════════"
    );
    println!("  LP ECONOMICS — Impermanent Loss, Fees, and Net P&L");
    println!("  Config: $5M AMM, 200% CR, Tick, 240-block TWAP");
    println!("  LP deposit: 1000 ZEC + 50,000 ZAI = $100,000 at $50/ZEC");
    println!(
        "═══════════════════════════════════════════════════════════════════════════════════════════════════\n"
    );

    let scenarios = [
        ScenarioId::BlackThursday,
        ScenarioId::SustainedBear,
        ScenarioId::SteadyState,
        ScenarioId::BullMarket,
    ];

    let mut results: Vec<LpEconomics> = Vec::new();
    for &sid in &scenarios {
        results.push(run_lp_economics(sid));
    }

    for r in &results {
        println!("  ┌─ {} ────────────────────────────────────────────", r.scenario_name);
        println!("  │ Entry price:      ${:.2}/ZEC", r.entry_price);
        println!(
            "  │ Final price:      ${:.2}/ZEC (external), ${:.2}/ZEC (AMM)",
            r.final_external_price, r.final_amm_price,
        );
        println!(
            "  │ LP share:         {:.6} / {:.2}%",
            r.lp_shares, r.lp_share_fraction * 100.0,
        );
        println!("  │");
        println!(
            "  │ Initial value:    ${:>12.2}",
            r.initial_value,
        );
        println!(
            "  │ Hold value (ext): ${:>12.2}  (just holding the original ZEC+ZAI)",
            r.hold_value_at_external,
        );
        println!(
            "  │ Pool value (ext): ${:>12.2}  (LP position valued at external price)",
            r.pool_value_at_external,
        );
        println!("  │");
        println!(
            "  │ IL (pure):        {:.4}%  (classic formula, excludes fees)",
            r.il_pct * 100.0,
        );
        println!(
            "  │ Pool fees total:  ${:>12.2}  (all LPs combined)",
            r.cumulative_fees,
        );
        println!(
            "  │ LP fee share:     ${:>12.2}  ({:.2}% of pool fees)",
            r.lp_fee_share,
            r.lp_share_fraction * 100.0,
        );
        println!("  │");
        println!(
            "  │ Net P&L (ext):    ${:>+12.2}  ({:+.4}%)",
            r.net_pnl_external,
            r.net_pnl_external / r.initial_value * 100.0,
        );
        println!(
            "  │ Net P&L (AMM):    ${:>+12.2}  ({:+.4}%)",
            r.net_pnl_amm,
            r.net_pnl_amm / r.initial_value * 100.0,
        );

        let lp_profitable = r.net_pnl_external > 0.0;
        println!("  │");
        println!(
            "  │ → LP {} at external prices (${:+.2})",
            if lp_profitable { "PROFITABLE" } else { "LOSES MONEY" },
            r.net_pnl_external,
        );
        println!("  └────────────────────────────────────────────────────────────\n");
    }

    // Summary table
    println!(
        "  {:<20} {:>10} {:>10} {:>10} {:>12} {:>12} {:>12}",
        "Scenario", "ExtPrice", "IL%", "Fees", "NetPnL(ext)", "NetPnL(AMM)", "Result"
    );
    println!("  {}", "─".repeat(90));

    for r in &results {
        println!(
            "  {:<20} ${:>8.2} {:>9.4}% ${:>9.2} ${:>+11.2} ${:>+11.2} {:>12}",
            r.scenario_name,
            r.final_external_price,
            r.il_pct * 100.0,
            r.lp_fee_share,
            r.net_pnl_external,
            r.net_pnl_amm,
            if r.net_pnl_external > 0.0 {
                "PROFITABLE"
            } else {
                "LOSS"
            },
        );
    }
    println!("  {}", "─".repeat(90));
}
