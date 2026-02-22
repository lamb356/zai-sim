/// High Liquidity Death Spiral Test — F-031
///
/// Tests whether F-028's defense mechanism (arber exhaustion → AMM price inertia)
/// breaks when arbers have proportionally enough capital to fully reprice the AMM.
///
/// Four configs at Black Thursday (1000 blocks):
///   1. $5M / 0.2% arber (baseline)
///   2. $25M / 0.2% arber (isolate pool depth)
///   3. $25M / 2% arber (isolate arber capitalization)
///   4. $50M / 2% arber (maximum arber pressure)
///
/// F-031: Document whether death spiral returns at high arber capitalization.
use zai_sim::agents::*;
use zai_sim::controller::ControllerConfig;
use zai_sim::output;
use zai_sim::report;
use zai_sim::scenario::{Scenario, ScenarioConfig};
use zai_sim::scenarios::{generate_prices, ScenarioId};

const BLOCKS: usize = 1000;
const SEED: u64 = 42;

struct HighLiqConfig {
    label: &'static str,
    amm_zec: f64,
    amm_zai: f64,
    arber_zec: f64,
    arber_zai: f64,
}

fn base_config(amm_zec: f64, amm_zai: f64) -> ScenarioConfig {
    let mut config = ScenarioConfig::default();
    config.amm_initial_zec = amm_zec;
    config.amm_initial_zai = amm_zai;
    config.cdp_config.min_ratio = 2.0;
    config.cdp_config.twap_window = 240;
    config.controller_config = ControllerConfig::default_tick();
    config
}

#[test]
fn high_liq_death_spiral() {
    println!(
        "\n═══════════════════════════════════════════════════════════════════════════════════════════════════"
    );
    println!("  HIGH LIQUIDITY DEATH SPIRAL TEST — F-031");
    println!("  Does F-028's defense mechanism break with well-capitalized arbers?");
    println!(
        "═══════════════════════════════════════════════════════════════════════════════════════════════════\n"
    );

    let configs = vec![
        HighLiqConfig {
            label: "$5M / 0.2% arber",
            amm_zec: 100_000.0,
            amm_zai: 5_000_000.0,
            arber_zec: 2_000.0,
            arber_zai: 100_000.0,
        },
        HighLiqConfig {
            label: "$25M / 0.2% arber",
            amm_zec: 500_000.0,
            amm_zai: 25_000_000.0,
            arber_zec: 5_000.0,
            arber_zai: 250_000.0,
        },
        HighLiqConfig {
            label: "$25M / 2% arber",
            amm_zec: 500_000.0,
            amm_zai: 25_000_000.0,
            arber_zec: 10_000.0,
            arber_zai: 500_000.0,
        },
        HighLiqConfig {
            label: "$50M / 2% arber",
            amm_zec: 1_000_000.0,
            amm_zai: 50_000_000.0,
            arber_zec: 20_000.0,
            arber_zai: 1_000_000.0,
        },
    ];

    println!(
        "  {:<22} {:>8} {:>8} {:>8} {:>6} {:>8} {:>8} {:>10} {:>10} {:>7}",
        "Config", "Verdict", "MeanPeg", "MaxPeg", "Liqs", "BadDebt", "Zombies", "FinalSpot", "FinalExt", "CRgap"
    );
    println!("  {}", "─".repeat(110));

    let prices = generate_prices(ScenarioId::BlackThursday, BLOCKS, SEED);

    for c in &configs {
        let config = base_config(c.amm_zec, c.amm_zai);
        let target = config.initial_redemption_price;
        let mut scenario = Scenario::new(&config);

        // Custom arber
        scenario.arbers.push(Arbitrageur::new(ArbitrageurConfig {
            initial_zec_balance: c.arber_zec,
            initial_zai_balance: c.arber_zai,
            ..ArbitrageurConfig::default()
        }));

        // Miner
        scenario
            .miners
            .push(MinerAgent::new(MinerAgentConfig::default()));

        // 5 CDP holders at 200% CR
        for _ in 0..5 {
            scenario.cdp_holders.push(CdpHolder::new(CdpHolderConfig {
                target_ratio: 2.5,
                action_threshold_ratio: 1.8,
                reserve_zec: 100.0,
                initial_collateral: 50.0,
                initial_debt: 1000.0,
            }));
        }

        scenario.run(&prices);

        let summary = output::compute_summary(&scenario.metrics, target);
        let verdict = report::evaluate_pass_fail(&scenario.metrics, target);

        // Compute death spiral indicators
        let total_liqs: u32 = scenario.metrics.iter().map(|m| m.liquidation_count).sum();
        let final_bad_debt = scenario.metrics.last().map(|m| m.bad_debt).unwrap_or(0.0);
        let max_zombies = scenario.metrics.iter().map(|m| m.zombie_vault_count).max().unwrap_or(0);
        let final_spot = scenario.metrics.last().map(|m| m.amm_spot_price).unwrap_or(0.0);
        let final_ext = scenario.metrics.last().map(|m| m.external_price).unwrap_or(0.0);
        let final_cr_gap = scenario
            .metrics
            .last()
            .map(|m| m.mean_collateral_ratio_twap - m.mean_collateral_ratio_ext)
            .unwrap_or(0.0);

        println!(
            "  {:<22} {:>8} {:>7.2}% {:>7.2}% {:>6} {:>8.2} {:>8} {:>10.2} {:>10.2} {:>7.2}",
            c.label,
            verdict.overall.label(),
            summary.mean_peg_deviation * 100.0,
            summary.max_peg_deviation * 100.0,
            total_liqs,
            final_bad_debt,
            max_zombies,
            final_spot,
            final_ext,
            final_cr_gap,
        );

        // Check for death spiral: did AMM price track external closely enough for liquidations?
        let amm_tracked_external = scenario.metrics.iter().any(|m| {
            let gap_pct = ((m.amm_spot_price - m.external_price) / m.external_price).abs();
            gap_pct < 0.10 && m.external_price < 30.0 // AMM within 10% of crashed external
        });

        let had_cascading_liqs = scenario.metrics.windows(10).any(|w| {
            w.iter().map(|m| m.liquidation_count).sum::<u32>() > 3
        });

        if amm_tracked_external {
            println!("    → AMM TRACKED external during crash (defense bypassed)");
        }
        if had_cascading_liqs {
            println!("    → CASCADING LIQUIDATIONS detected (death spiral risk)");
        }
        if total_liqs == 0 && !amm_tracked_external {
            println!("    → Defense held: AMM inertia prevented death spiral");
        }
    }

    // Interpretation
    println!("\n  ── Interpretation Matrix ──");
    println!("  If $25M/0.2% still prevents death spiral → pool depth alone doesn't enable it");
    println!("  If $25M/2% triggers death spiral → it's the arber ratio that breaks the defense");
    println!("  If $25M/2% still prevents → even 2% arber can't reprice a deep pool fast enough");
    println!("  Comparing $25M/2% vs $50M/2% → tests if pool size at constant ratio matters");

    println!(
        "\n═══════════════════════════════════════════════════════════════════════════════════════════════════\n"
    );
}
