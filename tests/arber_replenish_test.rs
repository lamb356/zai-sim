/// Arber Capital Replenishment — Sustained Bear at 43 Days
///
/// The sustained bear fails at 50,000 blocks because arbers run out of capital.
/// In reality, arbers replenish — sell ZEC externally, buy ZAI OTC, return to arb.
///
/// Sweeps capital_replenish_rate at 0/5/10/50/100 ZAI/block.
/// Finds the minimum rate where sustained_bear passes at 43 days.
///
/// F-028: Arber Capital Replenishment
use zai_sim::agents::*;
use zai_sim::controller::ControllerConfig;
use zai_sim::output;
use zai_sim::report;
use zai_sim::scenario::{Scenario, ScenarioConfig};
use zai_sim::scenarios::{generate_prices, ScenarioId};

const BLOCKS: usize = 50_000;
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

struct ReplenishResult {
    label: String,
    replenish_rate: f64,
    verdict: String,
    mean_peg: f64,
    max_peg: f64,
    final_peg: f64,
    _breaker_triggers: u32,
    arber_final_zai: f64,
    arber_final_zec: f64,
}

fn run_with_replenish(rate: f64, label: &str) -> ReplenishResult {
    let config = config_5m();
    let target = config.initial_redemption_price;

    let prices = generate_prices(ScenarioId::SustainedBear, BLOCKS, SEED);
    let mut scenario = Scenario::new(&config);

    // Arber with configurable replenishment rate
    let arber_config = ArbitrageurConfig {
        capital_replenish_rate: rate,
        ..ArbitrageurConfig::default()
    };
    scenario.arbers.push(Arbitrageur::new(arber_config));
    scenario
        .miners
        .push(MinerAgent::new(MinerAgentConfig::default()));

    scenario.run(&prices);

    let summary = output::compute_summary(&scenario.metrics, target);
    let verdict = report::evaluate_pass_fail(&scenario.metrics, target);
    let arber = &scenario.arbers[0];

    ReplenishResult {
        label: label.to_string(),
        replenish_rate: rate,
        verdict: verdict.overall.label().to_string(),
        mean_peg: summary.mean_peg_deviation,
        max_peg: summary.max_peg_deviation,
        final_peg: summary.final_peg_deviation,
        _breaker_triggers: summary.breaker_triggers,
        arber_final_zai: arber.zai_balance,
        arber_final_zec: arber.zec_balance,
    }
}

#[test]
fn arber_replenish() {
    println!(
        "\n═══════════════════════════════════════════════════════════════════════════════════════════════════"
    );
    println!("  ARBER CAPITAL REPLENISHMENT — Sustained Bear at 50,000 Blocks (43 Days)");
    println!("  Config: $5M AMM, 200% CR, Tick, 240-block TWAP");
    println!("  Arber: 2000 ZEC + 100K ZAI, variable ZAI replenish per block");
    println!(
        "═══════════════════════════════════════════════════════════════════════════════════════════════════\n"
    );

    let rates = [
        ("0_zai/blk", 0.0),
        ("5_zai/blk", 5.0),
        ("10_zai/blk", 10.0),
        ("50_zai/blk", 50.0),
        ("100_zai/blk", 100.0),
        ("500_zai/blk", 500.0),
        ("1000_zai/blk", 1000.0),
    ];

    println!(
        "  {:<14} {:>10} {:>8} {:>8} {:>8} {:>8} {:>12} {:>12}",
        "Config", "Rate", "Verdict", "MeanPeg", "MaxPeg", "FinalPeg", "ArberZAI", "ArberZEC"
    );
    println!("  {}", "─".repeat(90));

    let mut results: Vec<ReplenishResult> = Vec::new();
    for &(label, rate) in &rates {
        let r = run_with_replenish(rate, label);
        println!(
            "  {:<14} {:>10.1} {:>8} {:>7.4}% {:>7.4}% {:>7.4}% {:>12.2} {:>12.2}",
            r.label,
            r.replenish_rate,
            r.verdict,
            r.mean_peg * 100.0,
            r.max_peg * 100.0,
            r.final_peg * 100.0,
            r.arber_final_zai,
            r.arber_final_zec,
        );
        results.push(r);
    }

    // Comparison to baseline
    println!("\n  ── Comparison to no-replenishment baseline ──");
    let baseline = &results[0];
    for r in results.iter().skip(1) {
        let peg_delta = r.mean_peg - baseline.mean_peg;
        println!(
            "  {} → mean peg {:+.4}% ({:.4}% → {:.4}%), verdict: {} → {}",
            r.label,
            peg_delta * 100.0,
            baseline.mean_peg * 100.0,
            r.mean_peg * 100.0,
            baseline.verdict,
            r.verdict,
        );
    }

    // Find minimum rate for PASS
    println!("\n  ── Minimum Replenishment Rate for PASS ──");
    let min_pass = results.iter().find(|r| r.verdict == "PASS");
    match min_pass {
        Some(r) => println!(
            "  Minimum rate: {} ({:.1} ZAI/block = ${:.0}/day)",
            r.label,
            r.replenish_rate,
            r.replenish_rate * 48.0 * 24.0, // blocks/hour * hours/day
        ),
        None => println!("  No rate tested achieves PASS at 43 days"),
    }

    // Capital analysis
    println!("\n  ── Capital Analysis ──");
    for r in &results {
        let total_replenished = r.replenish_rate * BLOCKS as f64;
        let total_capital = 100_000.0 + total_replenished;
        println!(
            "  {}: replenished ${:.0}, total capital deployed ${:.0}, final ZAI ${:.0}",
            r.label, total_replenished, total_capital, r.arber_final_zai,
        );
    }

    println!(
        "\n═══════════════════════════════════════════════════════════════════════════════════════════════════\n"
    );
}
