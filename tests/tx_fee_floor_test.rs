/// Tx Fee Floor — Minimum Peg Deviation from Transaction Costs
///
/// Adds min_arb_profit to the arber config (default 0.50 ZAI ≈ $0.50).
/// The arber skips trades where expected profit < min_arb_profit.
///
/// This creates a peg deviation floor: below some threshold, arbing is
/// economically irrational because tx fees eat the profit.
///
/// Compares: no floor (0), $0.50 floor, $5.00 floor, $50 floor
/// at Black Thursday and sustained_bear, $5M config.
use zai_sim::agents::*;
use zai_sim::controller::ControllerConfig;
use zai_sim::output;
use zai_sim::report;
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

struct FeeFloorResult {
    label: String,
    min_arb_profit: f64,
    verdict: String,
    mean_peg: f64,
    max_peg: f64,
    _total_trades: u32,
    arber_final_zai: f64,
    arber_final_zec: f64,
}

fn run_with_fee_floor(
    sid: ScenarioId,
    min_arb_profit: f64,
    label: &str,
) -> FeeFloorResult {
    let config = config_5m();
    let target = config.initial_redemption_price;

    let prices = generate_prices(sid, BLOCKS, SEED);
    let mut scenario = Scenario::new(&config);

    // Arber with configurable min_arb_profit
    let arber_config = ArbitrageurConfig {
        min_arb_profit,
        ..ArbitrageurConfig::default()
    };
    scenario.arbers.push(Arbitrageur::new(arber_config));
    scenario
        .miners
        .push(MinerAgent::new(MinerAgentConfig::default()));

    scenario.run(&prices);

    let summary = output::compute_summary(&scenario.metrics, target);
    let verdict = report::evaluate_pass_fail(&scenario.metrics, target);

    // Count blocks where arber actually traded (non-zero ZAI balance changes)
    // Proxy: count arber actions from trade volume changes
    let arber = &scenario.arbers[0];

    FeeFloorResult {
        label: label.to_string(),
        min_arb_profit,
        verdict: verdict.overall.label().to_string(),
        mean_peg: summary.mean_peg_deviation,
        max_peg: summary.max_peg_deviation,
        _total_trades: 0,
        arber_final_zai: arber.zai_balance,
        arber_final_zec: arber.zec_balance,
    }
}

#[test]
fn tx_fee_floor() {
    println!(
        "\n═══════════════════════════════════════════════════════════════════════════════════════════════════"
    );
    println!("  TX FEE FLOOR — Minimum Peg Deviation from Transaction Costs");
    println!("  Config: $5M AMM, 200% CR, Tick, 240-block TWAP");
    println!("  Arber: 2000 ZEC + 100K ZAI, 0.5% threshold, 10-block sell latency");
    println!(
        "═══════════════════════════════════════════════════════════════════════════════════════════════════\n"
    );

    let scenarios = [ScenarioId::BlackThursday, ScenarioId::SustainedBear, ScenarioId::SteadyState];
    let fee_floors = [
        ("no_floor", 0.0),
        ("$0.50_floor", 0.50),
        ("$5_floor", 5.0),
        ("$50_floor", 50.0),
        ("$500_floor", 500.0),
    ];

    for &sid in &scenarios {
        println!(
            "  ┌─ {} ──────────────────────────────────────────────────────────",
            sid.name()
        );
        println!(
            "  │ {:<14} {:>10} {:>8} {:>8} {:>8} {:>12} {:>12}",
            "Config", "MinProfit", "Verdict", "MeanPeg", "MaxPeg", "ArberZAI", "ArberZEC"
        );
        println!("  │ {}", "─".repeat(80));

        let mut results: Vec<FeeFloorResult> = Vec::new();

        for &(label, floor) in &fee_floors {
            let r = run_with_fee_floor(sid, floor, label);

            println!(
                "  │ {:<14} {:>10.2} {:>8} {:>7.4}% {:>7.4}% {:>12.2} {:>12.2}",
                r.label,
                r.min_arb_profit,
                r.verdict,
                r.mean_peg * 100.0,
                r.max_peg * 100.0,
                r.arber_final_zai,
                r.arber_final_zec,
            );

            results.push(r);
        }

        // Delta analysis
        let baseline = &results[0];
        println!("  │");
        println!("  │ Comparison to no-floor baseline:");

        for r in results.iter().skip(1) {
            let peg_delta = r.mean_peg - baseline.mean_peg;
            println!(
                "  │   {} → mean peg {:+.4}% ({:.4}% → {:.4}%)",
                r.label,
                peg_delta * 100.0,
                baseline.mean_peg * 100.0,
                r.mean_peg * 100.0,
            );
        }

        // Find the peg deviation floor
        println!("  │");
        let floor_050 = &results[1];
        let floor_500 = &results[4];
        let peg_floor = floor_050.mean_peg;
        println!(
            "  │ Peg deviation floor at $0.50 tx fee: {:.4}%",
            peg_floor * 100.0,
        );
        println!(
            "  │ Peg deviation at $500 tx fee:        {:.4}%",
            floor_500.mean_peg * 100.0,
        );

        println!(
            "  └──────────────────────────────────────────────────────────────────\n"
        );
    }

    // Summary table
    println!(
        "  {:<20} {:<14} {:>8} {:>8}",
        "Scenario", "Fee Floor", "MeanPeg", "Verdict"
    );
    println!("  {}", "─".repeat(56));

    for &sid in &scenarios {
        for &(label, floor) in &fee_floors {
            let r = run_with_fee_floor(sid, floor, label);
            println!(
                "  {:<20} {:<14} {:>7.4}% {:>8}",
                if label == "no_floor" {
                    sid.name()
                } else {
                    ""
                },
                r.label,
                r.mean_peg * 100.0,
                r.verdict,
            );
        }
    }
    println!("  {}", "─".repeat(56));
}
