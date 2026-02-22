use zai_sim::agents::*;
use zai_sim::controller::ControllerConfig;
use zai_sim::output;
use zai_sim::report;
use zai_sim::scenario::{Scenario, ScenarioConfig};
use zai_sim::scenarios::{generate_prices, ScenarioId};

use std::path::PathBuf;

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

/// Run scenario with CDP holders to analyze zombie vaults.
fn run_with_vaults(sid: ScenarioId, num_holders: usize) -> Scenario {
    let config = config_5m();
    let prices = generate_prices(sid, BLOCKS, SEED);
    let mut scenario = Scenario::new(&config);

    // Standard agents
    scenario.arbers.push(Arbitrageur::new(ArbitrageurConfig::default()));
    scenario.miners.push(MinerAgent::new(MinerAgentConfig::default()));

    // CDP holders with varying collateral ratios
    for i in 0..num_holders {
        let target = 2.0 + (i as f64) * 0.5; // 2.0, 2.5, 3.0, ...
        scenario.cdp_holders.push(CdpHolder::new(CdpHolderConfig {
            target_ratio: target,
            action_threshold_ratio: target - 0.3,
            reserve_zec: 200.0,
            initial_collateral: 100.0,
            initial_debt: 2000.0,
        }));
    }

    scenario.run(&prices);
    scenario
}

#[test]
fn zombie_vault_analysis() {
    let report_dir = PathBuf::from("reports/zombie_vaults");
    let _ = std::fs::create_dir_all(&report_dir);

    let scenarios = [
        ScenarioId::BlackThursday,
        ScenarioId::SustainedBear,
        ScenarioId::FlashCrash,
        ScenarioId::BankRun,
    ];

    println!("\n═══════════════════════════════════════════════════════════════════════════════════════════════════");
    println!("  ZOMBIE VAULT ANALYSIS — TWAP vs External Price Collateral Ratio Gap");
    println!("  Config: $5M AMM, 200% CR, Tick, 240-block TWAP, 5 CDP holders per scenario");
    println!("═══════════════════════════════════════════════════════════════════════════════════════════════════\n");

    for sid in &scenarios {
        let scenario = run_with_vaults(*sid, 5);
        let config = config_5m();
        let target = config.initial_redemption_price;

        // Save HTML report
        let html = report::generate_report(
            &scenario.metrics,
            &config,
            &format!("{}_zombie", sid.name()),
            target,
        );
        let html_path = report_dir.join(format!("{}.html", sid.name()));
        let _ = report::save_report(&html, &html_path);

        let summary = output::compute_summary(&scenario.metrics, target);
        let verdict = report::evaluate_pass_fail(&scenario.metrics, target);

        // Analyze zombie metrics across all blocks
        let mut max_zombie_count: u32 = 0;
        let mut max_zombie_gap: f64 = 0.0;
        let mut zombie_duration: u64 = 0; // blocks where any zombie exists
        let mut max_ratio_gap: f64 = 0.0; // max gap between mean TWAP ratio and mean ext ratio
        let mut would_liquidate_ext: u32 = 0; // peak zombie count (vaults that external says liquidate)

        for m in &scenario.metrics {
            if m.zombie_vault_count > 0 {
                zombie_duration += 1;
            }
            if m.zombie_vault_count > max_zombie_count {
                max_zombie_count = m.zombie_vault_count;
            }
            if m.max_zombie_gap > max_zombie_gap {
                max_zombie_gap = m.max_zombie_gap;
            }
            if m.mean_collateral_ratio_twap > 0.0 && m.mean_collateral_ratio_ext > 0.0 {
                let gap = m.mean_collateral_ratio_twap - m.mean_collateral_ratio_ext;
                if gap > max_ratio_gap {
                    max_ratio_gap = gap;
                }
            }
            if m.zombie_vault_count > would_liquidate_ext {
                would_liquidate_ext = m.zombie_vault_count;
            }
        }

        // Find the block with worst zombie gap
        let worst_block = scenario
            .metrics
            .iter()
            .max_by(|a, b| a.max_zombie_gap.partial_cmp(&b.max_zombie_gap).unwrap())
            .unwrap();

        println!("  ┌─ {} ─────────────────────────────────────────", sid.name());
        println!("  │ Verdict            : {}", verdict.overall.label());
        println!("  │ Mean peg deviation : {:.4}%", summary.mean_peg_deviation * 100.0);
        println!("  │ Vault count        : {}", scenario.metrics.last().unwrap().vault_count);
        println!("  │");
        println!("  │ Zombie Metrics:");
        println!("  │   Max zombie vaults     : {} (of {} total)", max_zombie_count, scenario.metrics.last().unwrap().vault_count);
        println!("  │   Max zombie gap (CR)   : {:.4}", max_zombie_gap);
        println!("  │   Zombie duration       : {} blocks ({:.1}h)", zombie_duration, zombie_duration as f64 * 75.0 / 3600.0);
        println!("  │   Max mean CR gap       : {:.4} (TWAP - external)", max_ratio_gap);
        println!("  │   Would-liquidate (ext) : {} vaults at peak", would_liquidate_ext);
        println!("  │");
        println!("  │ Worst block (#{}): TWAP CR={:.4}, Ext CR={:.4}, gap={:.4}",
            worst_block.block,
            worst_block.mean_collateral_ratio_twap,
            worst_block.mean_collateral_ratio_ext,
            worst_block.max_zombie_gap,
        );
        println!("  │   External price: ${:.2}, TWAP: ${:.2}, AMM spot: ${:.2}",
            worst_block.external_price,
            worst_block.twap_price,
            worst_block.amm_spot_price,
        );
        println!("  └─────────────────────────────────────────────────────────\n");
    }

    // Summary table
    println!("  {:<20} {:>8} {:>10} {:>12} {:>12} {:>10}",
        "Scenario", "Zombies", "Gap (CR)", "Duration(h)", "Max CR Gap", "Ext Liqs");
    println!("  {}", "─".repeat(76));

    for sid in &scenarios {
        let scenario = run_with_vaults(*sid, 5);
        let mut max_z = 0u32;
        let mut max_g = 0.0f64;
        let mut dur = 0u64;
        let mut max_cg = 0.0f64;

        for m in &scenario.metrics {
            if m.zombie_vault_count > 0 { dur += 1; }
            if m.zombie_vault_count > max_z { max_z = m.zombie_vault_count; }
            if m.max_zombie_gap > max_g { max_g = m.max_zombie_gap; }
            let cg = m.mean_collateral_ratio_twap - m.mean_collateral_ratio_ext;
            if cg > max_cg { max_cg = cg; }
        }

        println!("  {:<20} {:>8} {:>10.4} {:>11.1}h {:>12.4} {:>10}",
            sid.name(), max_z, max_g, dur as f64 * 75.0 / 3600.0, max_cg, max_z);
    }
    println!("  {}", "─".repeat(76));
}
