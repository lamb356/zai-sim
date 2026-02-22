/// Duration Honesty — Longer Simulation Runs
///
/// Current runs are 1000 blocks = 20.8 hours. This test runs:
///   - black_thursday at 1,152 blocks (24 hours)
///   - sustained_bear at 10,000 blocks (8.7 days)
///   - sustained_bear at 50,000 blocks (43 days)
///
/// Compares to the 1000-block baselines and reports wall-clock runtime.
/// Labels scenarios honestly by their simulated duration.
use zai_sim::controller::ControllerConfig;
use zai_sim::output;
use zai_sim::report;
use zai_sim::scenario::ScenarioConfig;
use zai_sim::scenarios::{run_stress, ScenarioId};

use std::time::Instant;

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

struct DurationResult {
    label: String,
    blocks: usize,
    duration_label: String,
    verdict: String,
    mean_peg: f64,
    max_peg: f64,
    final_peg: f64,
    breaker_triggers: u32,
    wall_clock_secs: f64,
}

fn run_duration(sid: ScenarioId, blocks: usize, label: &str, duration_label: &str) -> DurationResult {
    let config = config_5m();
    let target = config.initial_redemption_price;

    let start = Instant::now();
    let scenario = run_stress(sid, &config, blocks, SEED);
    let elapsed = start.elapsed();

    let summary = output::compute_summary(&scenario.metrics, target);
    let verdict = report::evaluate_pass_fail(&scenario.metrics, target);

    DurationResult {
        label: label.to_string(),
        blocks,
        duration_label: duration_label.to_string(),
        verdict: verdict.overall.label().to_string(),
        mean_peg: summary.mean_peg_deviation,
        max_peg: summary.max_peg_deviation,
        final_peg: summary.final_peg_deviation,
        breaker_triggers: summary.breaker_triggers,
        wall_clock_secs: elapsed.as_secs_f64(),
    }
}

#[test]
fn duration_honesty() {
    println!(
        "\n═══════════════════════════════════════════════════════════════════════════════════════════════════"
    );
    println!("  DURATION HONESTY — Longer Simulation Runs");
    println!("  Config: $5M AMM, 200% CR, Tick, 240-block TWAP, seed=42");
    println!("  Block time: 75s (1 block = 1.25 min)");
    println!(
        "═══════════════════════════════════════════════════════════════════════════════════════════════════\n"
    );

    let results = vec![
        // Black Thursday comparisons
        run_duration(ScenarioId::BlackThursday, 1000, "BT 1000b", "20.8 hours"),
        run_duration(ScenarioId::BlackThursday, 1152, "BT 1152b", "24 hours"),
        // Sustained Bear comparisons
        run_duration(ScenarioId::SustainedBear, 1000, "SB 1000b", "20.8 hours"),
        run_duration(ScenarioId::SustainedBear, 10000, "SB 10000b", "8.7 days"),
        run_duration(ScenarioId::SustainedBear, 50000, "SB 50000b", "43 days"),
    ];

    println!(
        "  {:<14} {:>8} {:>12} {:>8} {:>8} {:>8} {:>8} {:>8} {:>10}",
        "Label", "Blocks", "Duration", "Verdict", "MeanPeg", "MaxPeg", "FinalPeg", "Breaker", "WallClock"
    );
    println!("  {}", "─".repeat(96));

    for r in &results {
        println!(
            "  {:<14} {:>8} {:>12} {:>8} {:>7.4}% {:>7.4}% {:>7.4}% {:>8} {:>8.2}s",
            r.label,
            r.blocks,
            r.duration_label,
            r.verdict,
            r.mean_peg * 100.0,
            r.max_peg * 100.0,
            r.final_peg * 100.0,
            r.breaker_triggers,
            r.wall_clock_secs,
        );
    }

    // Comparison analysis
    println!("\n  ── Black Thursday: 1000 vs 1152 blocks ──");
    let bt_1k = &results[0];
    let bt_1152 = &results[1];
    let bt_peg_delta = bt_1152.mean_peg - bt_1k.mean_peg;
    println!(
        "  Mean peg delta: {:+.4}% ({:.4}% → {:.4}%)",
        bt_peg_delta * 100.0,
        bt_1k.mean_peg * 100.0,
        bt_1152.mean_peg * 100.0,
    );
    println!(
        "  Verdict: {} → {}",
        bt_1k.verdict, bt_1152.verdict,
    );

    println!("\n  ── Sustained Bear: 1000 vs 10000 vs 50000 blocks ──");
    let sb_1k = &results[2];
    let sb_10k = &results[3];
    let sb_50k = &results[4];
    println!(
        "  1000b → 10000b: mean peg {:+.4}% ({:.4}% → {:.4}%)",
        (sb_10k.mean_peg - sb_1k.mean_peg) * 100.0,
        sb_1k.mean_peg * 100.0,
        sb_10k.mean_peg * 100.0,
    );
    println!(
        "  1000b → 50000b: mean peg {:+.4}% ({:.4}% → {:.4}%)",
        (sb_50k.mean_peg - sb_1k.mean_peg) * 100.0,
        sb_1k.mean_peg * 100.0,
        sb_50k.mean_peg * 100.0,
    );
    println!(
        "  Verdicts: {} → {} → {}",
        sb_1k.verdict, sb_10k.verdict, sb_50k.verdict,
    );
    println!(
        "  Wall clock: {:.2}s → {:.2}s → {:.2}s",
        sb_1k.wall_clock_secs, sb_10k.wall_clock_secs, sb_50k.wall_clock_secs,
    );

    // Performance scaling
    let blocks_ratio_10k = 10000.0 / 1000.0;
    let time_ratio_10k = sb_10k.wall_clock_secs / sb_1k.wall_clock_secs;
    let blocks_ratio_50k = 50000.0 / 1000.0;
    let time_ratio_50k = sb_50k.wall_clock_secs / sb_1k.wall_clock_secs;
    println!(
        "\n  Performance scaling: 10x blocks = {:.1}x time, 50x blocks = {:.1}x time",
        time_ratio_10k, time_ratio_50k,
    );
    println!(
        "  (Linear would be {:.0}x and {:.0}x)",
        blocks_ratio_10k, blocks_ratio_50k,
    );

    println!(
        "\n═══════════════════════════════════════════════════════════════════════════════════════════════════\n"
    );
}
