/// Slow Bleed Scenario — F-030
///
/// Tests a prolonged exponential decline where per-block moves are small
/// but cumulative effect is devastating (95% decline over 8.7 days).
///
/// Price path: $50 → $2.50 exponentially over 10,000 blocks
/// Per-block decline: ~0.03% (tiny enough that arbers can track each block)
/// Total decline: 95%
///
/// Key question: Can arbers keep up block-by-block with a slow decline?
/// Or does capital exhaust partway through?
use zai_sim::agents::*;
use zai_sim::controller::ControllerConfig;
use zai_sim::output;
use zai_sim::report;
use zai_sim::scenario::{Scenario, ScenarioConfig};

const TOTAL_BLOCKS: usize = 10_000;

fn slow_bleed_prices(blocks: usize) -> Vec<f64> {
    // $50 → $2.50 exponentially over `blocks`
    (0..blocks)
        .map(|i| 50.0 * (2.5_f64 / 50.0).powf(i as f64 / (blocks - 1) as f64))
        .collect()
}

fn config_5m() -> ScenarioConfig {
    let mut config = ScenarioConfig::default();
    config.amm_initial_zec = 100_000.0;
    config.amm_initial_zai = 5_000_000.0;
    config.cdp_config.min_ratio = 2.0;
    config.cdp_config.twap_window = 240;
    config.controller_config = ControllerConfig::default_tick();
    config
}

#[test]
fn slow_bleed_scenario() {
    println!(
        "\n═══════════════════════════════════════════════════════════════════════════════════════════════════"
    );
    println!("  SLOW BLEED SCENARIO — F-030");
    println!("  Exponential decline: $50 → $2.50 over 10,000 blocks (8.7 days)");
    println!("  Per-block decline: ~0.03% — can arbers keep up?");
    println!(
        "═══════════════════════════════════════════════════════════════════════════════════════════════════\n"
    );

    let config = config_5m();
    let target = config.initial_redemption_price;
    let prices = slow_bleed_prices(TOTAL_BLOCKS);
    let mut scenario = Scenario::new(&config);

    // Standard arber + miner
    scenario
        .arbers
        .push(Arbitrageur::new(ArbitrageurConfig::default()));
    scenario
        .miners
        .push(MinerAgent::new(MinerAgentConfig::default()));

    scenario.run(&prices);

    let summary = output::compute_summary(&scenario.metrics, target);
    let verdict = report::evaluate_pass_fail(&scenario.metrics, target);

    // Find when arber exhausts ZEC (balance < 1.0)
    let arber_exhaust_block = scenario
        .metrics
        .iter()
        .position(|m| m.arber_zec_total < 1.0)
        .map(|i| i + 1);

    // Track AMM vs external gap over time
    let gaps: Vec<f64> = scenario
        .metrics
        .iter()
        .map(|m| ((m.amm_spot_price - m.external_price) / m.external_price).abs() * 100.0)
        .collect();

    let mean_gap = gaps.iter().sum::<f64>() / gaps.len() as f64;
    let max_gap = gaps.iter().cloned().fold(0.0_f64, f64::max);
    let final_gap = *gaps.last().unwrap_or(&0.0);

    // Sample at key points
    let checkpoints = [100, 500, 1000, 2500, 5000, 7500, 9999];

    println!("  ── Overall ──");
    println!(
        "  Verdict: {}  |  Mean Peg: {:.2}%  |  Max Peg: {:.2}%",
        verdict.overall.label(),
        summary.mean_peg_deviation * 100.0,
        summary.max_peg_deviation * 100.0,
    );

    if let Some(exhaust) = arber_exhaust_block {
        let exhaust_price = prices[exhaust - 1];
        println!(
            "  Arber ZEC exhausted at block {} (ext price ${:.2}, {:.1}% of initial)",
            exhaust,
            exhaust_price,
            exhaust_price / 50.0 * 100.0,
        );
    } else {
        println!("  Arber ZEC never fully exhausted");
    }

    println!("\n  ── Timeline ──");
    println!(
        "  {:<8} {:>10} {:>10} {:>10} {:>8} {:>10} {:>10}",
        "Block", "Ext Price", "AMM Spot", "Gap %", "ArberZEC", "ArberZAI", "Breakers"
    );
    println!("  {}", "─".repeat(75));

    for &cp in &checkpoints {
        if cp < scenario.metrics.len() {
            let m = &scenario.metrics[cp];
            let gap = gaps[cp];
            // Count breaker triggers up to this point
            let breakers_so_far: u32 = scenario.metrics[..=cp]
                .iter()
                .map(|m| {
                    m.breaker_actions
                        .iter()
                        .filter(|a| {
                            !matches!(a, zai_sim::circuit_breaker::BreakerAction::None)
                        })
                        .count() as u32
                })
                .sum();
            println!(
                "  {:<8} {:>10.2} {:>10.2} {:>9.2}% {:>8.1} {:>10.0} {:>10}",
                m.block, m.external_price, m.amm_spot_price, gap, m.arber_zec_total,
                m.arber_zai_total, breakers_so_far,
            );
        }
    }

    println!("\n  ── AMM vs External Gap ──");
    println!("  Mean gap: {:.2}%", mean_gap);
    println!("  Max gap:  {:.2}%", max_gap);
    println!("  Final gap: {:.2}%", final_gap);
    println!(
        "  Final AMM spot: ${:.2} vs External: ${:.2}",
        scenario.metrics.last().unwrap().amm_spot_price,
        scenario.metrics.last().unwrap().external_price,
    );

    // Determine if arbers kept up
    println!("\n  ── Conclusion ──");
    if final_gap < 5.0 {
        println!(
            "  ARBERS KEPT UP: AMM tracked external within {:.2}% at end",
            final_gap
        );
    } else if arber_exhaust_block.is_some() {
        println!(
            "  ARBER EXHAUSTION: Capital depleted at block {}, then AMM diverged to {:.2}% gap",
            arber_exhaust_block.unwrap(),
            final_gap,
        );
    } else {
        println!(
            "  DIVERGED: AMM-external gap reached {:.2}% without full arber exhaustion",
            final_gap
        );
    }

    println!(
        "\n═══════════════════════════════════════════════════════════════════════════════════════════════════\n"
    );
}
