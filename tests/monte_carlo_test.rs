/// Monte Carlo Statistical Analysis (F-041)
///
/// Runs each crash scenario with 100 different seeds (stochastic=true) to get
/// statistical confidence on bad debt, peg deviation, and liquidation distributions.
/// Transforms "seed 42 works" into "the system works with 99%+ confidence."
///
/// Stochastic mode enables three sources of per-seed variation:
/// 1. Price noise: multiplicative N(0, 0.02) per block
/// 2. Arber activity: 80% chance of acting each block
/// 3. Demand/miner timing: stochastic skip/batch patterns
///
/// Sweep: 4 scenarios × 100 seeds = 400 runs.
use zai_sim::agents::*;
use zai_sim::controller::ControllerConfig;
use zai_sim::output;
use zai_sim::report;
use zai_sim::scenario::{Scenario, ScenarioConfig};
use zai_sim::scenarios::{apply_price_noise, generate_prices, ScenarioId};

use std::path::PathBuf;

const BLOCKS: usize = 1000;
const NUM_SEEDS: u64 = 100;

// ═══════════════════════════════════════════════════════════════════════
// Statistical Helpers
// ═══════════════════════════════════════════════════════════════════════

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }
    let idx = p * (sorted.len() - 1) as f64;
    let lo = idx.floor() as usize;
    let hi = (lo + 1).min(sorted.len() - 1);
    let frac = idx - lo as f64;
    sorted[lo] * (1.0 - frac) + sorted[hi] * frac
}

fn median(sorted: &[f64]) -> f64 {
    percentile(sorted, 0.5)
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

fn stddev(values: &[f64]) -> f64 {
    if values.len() < 2 {
        return 0.0;
    }
    let m = mean(values);
    let variance = values.iter().map(|v| (v - m).powi(2)).sum::<f64>() / values.len() as f64;
    variance.sqrt()
}

// ═══════════════════════════════════════════════════════════════════════
// Config & Agents
// ═══════════════════════════════════════════════════════════════════════

fn config_5m_stochastic() -> ScenarioConfig {
    let mut c = ScenarioConfig::default();
    c.amm_initial_zec = 100_000.0;
    c.amm_initial_zai = 5_000_000.0;
    c.cdp_config.min_ratio = 2.0;
    c.cdp_config.twap_window = 240;
    c.controller_config = ControllerConfig::default_tick();
    c.liquidation_config.max_liquidations_per_block = 50;
    c.stochastic = true;
    c.noise_sigma = 0.02;
    c
}

fn add_agents(scenario: &mut Scenario) {
    scenario
        .arbers
        .push(Arbitrageur::new(ArbitrageurConfig::default()));
    scenario
        .miners
        .push(MinerAgent::new(MinerAgentConfig::default()));
    scenario
        .demand_agents
        .push(DemandAgent::new(DemandAgentConfig::default()));
    scenario
        .cdp_holders
        .push(CdpHolder::new(CdpHolderConfig::default()));
}

// ═══════════════════════════════════════════════════════════════════════
// Per-Run & Per-Scenario Structs
// ═══════════════════════════════════════════════════════════════════════

struct RunResult {
    bad_debt: f64,
    mean_peg: f64,
    max_peg: f64,
    liqs: u32,
    max_zombie_count: u32,
    verdict: String,
}

struct ScenarioStats {
    scenario_name: String,
    num_seeds: usize,
    pass_count: usize,
    soft_fail_count: usize,
    hard_fail_count: usize,
    bd_min: f64,
    bd_max: f64,
    bd_mean: f64,
    bd_median: f64,
    bd_p95: f64,
    bd_p99: f64,
    bd_stddev: f64,
    mp_min: f64,
    mp_max: f64,
    mp_mean: f64,
    mp_median: f64,
    mp_p95: f64,
    mp_p99: f64,
    xp_min: f64,
    xp_max: f64,
    xp_mean: f64,
    xp_p95: f64,
    liq_min: u32,
    liq_max: u32,
    liq_mean: f64,
    zmb_min: u32,
    zmb_max: u32,
    zmb_mean: f64,
}

fn compute_stats(scenario_name: &str, results: &[RunResult]) -> ScenarioStats {
    let n = results.len();

    let pass_count = results.iter().filter(|r| r.verdict == "PASS").count();
    let soft_fail_count = results.iter().filter(|r| r.verdict == "SOFT_FAIL").count();
    let hard_fail_count = results.iter().filter(|r| r.verdict == "HARD_FAIL").count();

    let mut bd: Vec<f64> = results.iter().map(|r| r.bad_debt).collect();
    bd.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let mut mp: Vec<f64> = results.iter().map(|r| r.mean_peg).collect();
    mp.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let mut xp: Vec<f64> = results.iter().map(|r| r.max_peg).collect();
    xp.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let mut liqs: Vec<u32> = results.iter().map(|r| r.liqs).collect();
    liqs.sort();

    let mut zmbs: Vec<u32> = results.iter().map(|r| r.max_zombie_count).collect();
    zmbs.sort();

    ScenarioStats {
        scenario_name: scenario_name.to_string(),
        num_seeds: n,
        pass_count,
        soft_fail_count,
        hard_fail_count,
        bd_min: bd[0],
        bd_max: bd[n - 1],
        bd_mean: mean(&bd),
        bd_median: median(&bd),
        bd_p95: percentile(&bd, 0.95),
        bd_p99: percentile(&bd, 0.99),
        bd_stddev: stddev(&bd),
        mp_min: mp[0],
        mp_max: mp[n - 1],
        mp_mean: mean(&mp),
        mp_median: median(&mp),
        mp_p95: percentile(&mp, 0.95),
        mp_p99: percentile(&mp, 0.99),
        xp_min: xp[0],
        xp_max: xp[n - 1],
        xp_mean: mean(&xp),
        xp_p95: percentile(&xp, 0.95),
        liq_min: liqs[0],
        liq_max: liqs[n - 1],
        liq_mean: liqs.iter().map(|&x| x as f64).sum::<f64>() / n as f64,
        zmb_min: zmbs[0],
        zmb_max: zmbs[n - 1],
        zmb_mean: zmbs.iter().map(|&x| x as f64).sum::<f64>() / n as f64,
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Run Single
// ═══════════════════════════════════════════════════════════════════════

fn run_single(sid: ScenarioId, seed: u64) -> RunResult {
    let config = config_5m_stochastic();
    let target = 50.0;

    let mut prices = generate_prices(sid, BLOCKS, seed);
    apply_price_noise(&mut prices, config.noise_sigma, seed);

    let mut scenario = Scenario::new_with_seed(&config, seed);
    add_agents(&mut scenario);
    scenario.run(&prices);

    let summary = output::compute_summary(&scenario.metrics, target);
    let verdict = report::evaluate_pass_fail(&scenario.metrics, target);

    let max_zombie_count = scenario
        .metrics
        .iter()
        .map(|m| m.zombie_vault_count)
        .max()
        .unwrap_or(0);

    RunResult {
        bad_debt: summary.total_bad_debt,
        mean_peg: summary.mean_peg_deviation,
        max_peg: summary.max_peg_deviation,
        liqs: summary.total_liquidations,
        max_zombie_count,
        verdict: verdict.overall.label().to_string(),
    }
}

// ═══════════════════════════════════════════════════════════════════════
// HTML Report
// ═══════════════════════════════════════════════════════════════════════

fn generate_monte_carlo_html(all_stats: &[ScenarioStats]) -> String {
    let mut rows = String::new();
    for s in all_stats {
        let pass_pct = s.pass_count as f64 / s.num_seeds as f64 * 100.0;
        let badge_cls = if pass_pct >= 99.0 {
            "pass"
        } else if pass_pct >= 90.0 {
            "soft-fail"
        } else {
            "hard-fail"
        };
        rows.push_str(&format!(
            "<tr>\
             <td>{name}</td>\
             <td>{seeds}</td>\
             <td><span class=\"badge {cls}\">{pass:.0}%</span></td>\
             <td>{bd_mean:.2}</td><td>{bd_p95:.2}</td><td>{bd_p99:.2}</td><td>{bd_max:.2}</td>\
             <td>{mp_mean:.2}%</td><td>{mp_p95:.2}%</td>\
             <td>{xp_mean:.2}%</td><td>{xp_p95:.2}%</td>\
             <td>{liq_mean:.1}</td><td>{liq_max}</td>\
             </tr>\n",
            name = s.scenario_name,
            seeds = s.num_seeds,
            cls = badge_cls,
            pass = pass_pct,
            bd_mean = s.bd_mean,
            bd_p95 = s.bd_p95,
            bd_p99 = s.bd_p99,
            bd_max = s.bd_max,
            mp_mean = s.mp_mean * 100.0,
            mp_p95 = s.mp_p95 * 100.0,
            xp_mean = s.xp_mean * 100.0,
            xp_p95 = s.xp_p95 * 100.0,
            liq_mean = s.liq_mean,
            liq_max = s.liq_max,
        ));
    }

    let mut detail_sections = String::new();
    for s in all_stats {
        let pass_pct = s.pass_count as f64 / s.num_seeds as f64 * 100.0;
        let sf_pct = s.soft_fail_count as f64 / s.num_seeds as f64 * 100.0;
        let hf_pct = s.hard_fail_count as f64 / s.num_seeds as f64 * 100.0;
        detail_sections.push_str(&format!(
            r#"<section>
<h3>{name} — {seeds} seeds</h3>
<table>
<tr><th>Metric</th><th>Min</th><th>Mean</th><th>Median</th><th>P95</th><th>P99</th><th>Max</th><th>StdDev</th></tr>
<tr><td>Bad Debt ($)</td><td>{bd_min:.2}</td><td>{bd_mean:.2}</td><td>{bd_med:.2}</td><td>{bd_p95:.2}</td><td>{bd_p99:.2}</td><td>{bd_max:.2}</td><td>{bd_sd:.2}</td></tr>
<tr><td>Mean Peg Dev (%)</td><td>{mp_min:.2}</td><td>{mp_mean:.2}</td><td>{mp_med:.2}</td><td>{mp_p95:.2}</td><td>{mp_p99:.2}</td><td>{mp_max:.2}</td><td>-</td></tr>
<tr><td>Max Peg Dev (%)</td><td>{xp_min:.2}</td><td>{xp_mean:.2}</td><td>-</td><td>{xp_p95:.2}</td><td>-</td><td>{xp_max:.2}</td><td>-</td></tr>
<tr><td>Liquidations</td><td>{liq_min}</td><td>{liq_mean:.1}</td><td>-</td><td>-</td><td>-</td><td>{liq_max}</td><td>-</td></tr>
<tr><td>Peak Zombies</td><td>{zmb_min}</td><td>{zmb_mean:.1}</td><td>-</td><td>-</td><td>-</td><td>{zmb_max}</td><td>-</td></tr>
</table>
<p>Verdict: <strong>{pass:.0}% PASS</strong>, {sf:.0}% SOFT_FAIL, {hf:.0}% HARD_FAIL</p>
</section>
"#,
            name = s.scenario_name,
            seeds = s.num_seeds,
            bd_min = s.bd_min, bd_mean = s.bd_mean, bd_med = s.bd_median,
            bd_p95 = s.bd_p95, bd_p99 = s.bd_p99, bd_max = s.bd_max, bd_sd = s.bd_stddev,
            mp_min = s.mp_min * 100.0, mp_mean = s.mp_mean * 100.0, mp_med = s.mp_median * 100.0,
            mp_p95 = s.mp_p95 * 100.0, mp_p99 = s.mp_p99 * 100.0, mp_max = s.mp_max * 100.0,
            xp_min = s.xp_min * 100.0, xp_mean = s.xp_mean * 100.0,
            xp_p95 = s.xp_p95 * 100.0, xp_max = s.xp_max * 100.0,
            liq_min = s.liq_min, liq_mean = s.liq_mean, liq_max = s.liq_max,
            zmb_min = s.zmb_min, zmb_mean = s.zmb_mean, zmb_max = s.zmb_max,
            pass = pass_pct, sf = sf_pct, hf = hf_pct,
        ));
    }

    let total_runs: usize = all_stats.iter().map(|s| s.num_seeds).sum();
    let total_pass: usize = all_stats.iter().map(|s| s.pass_count).sum();

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<title>ZAI Simulation — Monte Carlo Analysis (F-041)</title>
<style>
*{{margin:0;padding:0;box-sizing:border-box}}
body{{font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif;background:#f5f5f5;color:#333}}
header{{background:#1a1a2e;color:#fff;padding:24px 32px}}
header h1{{font-size:1.4em;font-weight:500}}
.summary-line{{margin-top:8px;font-size:1em;opacity:0.9}}
main{{max-width:1400px;margin:0 auto;padding:24px}}
section{{background:#fff;border-radius:8px;box-shadow:0 1px 3px rgba(0,0,0,0.1);padding:24px;margin-bottom:20px}}
h3{{margin-bottom:12px;color:#1a1a2e}}
table{{width:100%;border-collapse:collapse;font-size:0.9em}}
th,td{{padding:10px 14px;text-align:left;border-bottom:1px solid #e0e0e0}}
th{{background:#f8f9fa;font-weight:600}}
.badge{{padding:3px 10px;border-radius:3px;font-weight:700;font-size:0.8em}}
.badge.pass{{background:#34a853;color:#fff}}
.badge.soft-fail{{background:#ea8c00;color:#fff}}
.badge.hard-fail{{background:#ea4335;color:#fff}}
p{{margin-top:12px;font-size:0.95em}}
footer{{text-align:center;padding:16px;color:#999;font-size:0.8em}}
</style>
</head>
<body>
<header>
 <h1>ZAI Simulation — Monte Carlo Analysis (F-041)</h1>
 <div class="summary-line">{total_pass} / {total_runs} runs passed across 4 scenarios x {num_seeds} seeds (stochastic=true, noise=2%)</div>
</header>
<main>
<section>
<h3>Summary</h3>
<table>
<tr>
 <th>Scenario</th><th>Seeds</th><th>Pass%</th>
 <th>Mean BD</th><th>P95 BD</th><th>P99 BD</th><th>Max BD</th>
 <th>Mean Peg</th><th>P95 Peg</th>
 <th>Mean MaxPeg</th><th>P95 MaxPeg</th>
 <th>Mean Liqs</th><th>Max Liqs</th>
</tr>
{rows}
</table>
</section>
{details}
</main>
<footer>Generated by zai-sim — Monte Carlo: {num_seeds} seeds, stochastic=true, noise_sigma=0.02</footer>
</body>
</html>"#,
        total_pass = total_pass,
        total_runs = total_runs,
        num_seeds = NUM_SEEDS,
        rows = rows,
        details = detail_sections,
    )
}

// ═══════════════════════════════════════════════════════════════════════
// Test
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn monte_carlo_sweep() {
    let scenarios: Vec<(&str, ScenarioId)> = vec![
        ("black_thursday", ScenarioId::BlackThursday),
        ("sustained_bear", ScenarioId::SustainedBear),
        ("flash_crash", ScenarioId::FlashCrash),
        ("bank_run", ScenarioId::BankRun),
    ];

    let report_dir = PathBuf::from("reports/monte_carlo");
    let _ = std::fs::create_dir_all(&report_dir);

    let mut all_stats: Vec<ScenarioStats> = Vec::new();

    println!("\n  Running Monte Carlo analysis...");
    println!("  Config: $5M AMM, 200% CR, Tick controller, 240-block TWAP, stochastic=true");
    println!("  Noise: 2% per-block multiplicative, 80% arber activity rate");
    println!(
        "  Sweep: 4 scenarios x {} seeds = {} runs\n",
        NUM_SEEDS,
        4 * NUM_SEEDS
    );

    for &(scenario_name, sid) in &scenarios {
        print!("  Running {} (seeds 1-{})...", scenario_name, NUM_SEEDS);
        let mut results: Vec<RunResult> = Vec::with_capacity(NUM_SEEDS as usize);

        for seed in 1..=NUM_SEEDS {
            results.push(run_single(sid, seed));
        }

        let stats = compute_stats(scenario_name, &results);
        let pass_pct = stats.pass_count as f64 / stats.num_seeds as f64 * 100.0;
        println!(
            " done. {:.0}% PASS, bad_debt: mean=${:.2} max=${:.2}",
            pass_pct, stats.bd_mean, stats.bd_max
        );

        all_stats.push(stats);
    }

    // ═══════════════════════════════════════════════════════════════════
    // Console Output
    // ═══════════════════════════════════════════════════════════════════

    println!("\n═══════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════");
    println!("  ZAI SIMULATOR — MONTE CARLO ANALYSIS (F-041)");
    println!(
        "  Config: $5M AMM, 200% CR, stochastic=true, noise_sigma=0.02, {} seeds per scenario",
        NUM_SEEDS
    );
    println!("═══════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════");

    // Summary table
    println!(
        "\n  {:<16} {:>6} {:>8} {:>10} {:>10} {:>10} {:>10} {:>10} {:>10}",
        "Scenario", "Seeds", "Pass%", "Mean BD", "P95 BD", "P99 BD", "Max BD", "Mean Peg",
        "P95 Peg"
    );
    println!("  {}", "─".repeat(100));

    for s in &all_stats {
        let pass_pct = s.pass_count as f64 / s.num_seeds as f64 * 100.0;
        println!(
            "  {:<16} {:>6} {:>7.0}% {:>9.2} {:>9.2} {:>9.2} {:>9.2} {:>9.2}% {:>9.2}%",
            s.scenario_name,
            s.num_seeds,
            pass_pct,
            s.bd_mean,
            s.bd_p95,
            s.bd_p99,
            s.bd_max,
            s.mp_mean * 100.0,
            s.mp_p95 * 100.0,
        );
    }
    println!("  {}", "─".repeat(100));

    // Per-scenario detail tables
    for s in &all_stats {
        let pass_pct = s.pass_count as f64 / s.num_seeds as f64 * 100.0;
        let sf_pct = s.soft_fail_count as f64 / s.num_seeds as f64 * 100.0;
        let hf_pct = s.hard_fail_count as f64 / s.num_seeds as f64 * 100.0;

        println!(
            "\n  {} — {} seeds",
            s.scenario_name.to_uppercase(),
            s.num_seeds
        );
        println!(
            "  Verdict: {:.0}% PASS, {:.0}% SOFT_FAIL, {:.0}% HARD_FAIL",
            pass_pct, sf_pct, hf_pct
        );
        println!(
            "  {:<20} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8}",
            "Metric", "Min", "Mean", "Median", "P95", "P99", "Max", "StdDev"
        );
        println!("  {}", "─".repeat(76));
        println!(
            "  {:<20} {:>8.2} {:>8.2} {:>8.2} {:>8.2} {:>8.2} {:>8.2} {:>8.2}",
            "Bad Debt ($)",
            s.bd_min,
            s.bd_mean,
            s.bd_median,
            s.bd_p95,
            s.bd_p99,
            s.bd_max,
            s.bd_stddev
        );
        println!(
            "  {:<20} {:>7.2}% {:>7.2}% {:>7.2}% {:>7.2}% {:>7.2}% {:>7.2}%",
            "Mean Peg Dev",
            s.mp_min * 100.0,
            s.mp_mean * 100.0,
            s.mp_median * 100.0,
            s.mp_p95 * 100.0,
            s.mp_p99 * 100.0,
            s.mp_max * 100.0
        );
        println!(
            "  {:<20} {:>7.2}% {:>7.2}% {:>8} {:>7.2}% {:>8} {:>7.2}%",
            "Max Peg Dev",
            s.xp_min * 100.0,
            s.xp_mean * 100.0,
            "-",
            s.xp_p95 * 100.0,
            "-",
            s.xp_max * 100.0
        );
        println!(
            "  {:<20} {:>8} {:>8.1} {:>8} {:>8} {:>8} {:>8}",
            "Liquidations", s.liq_min, s.liq_mean, "-", "-", "-", s.liq_max
        );
        println!(
            "  {:<20} {:>8} {:>8.1} {:>8} {:>8} {:>8} {:>8}",
            "Peak Zombies", s.zmb_min, s.zmb_mean, "-", "-", "-", s.zmb_max
        );
    }

    println!("\n═══════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════");
    println!("  Reports saved to: reports/monte_carlo/");
    println!("  Summary:          reports/monte_carlo/index.html\n");

    // ═══════════════════════════════════════════════════════════════════
    // HTML Report
    // ═══════════════════════════════════════════════════════════════════

    let html = generate_monte_carlo_html(&all_stats);
    let html_path = report_dir.join("index.html");
    report::save_report(&html, &html_path).expect("save monte carlo report");

    // ═══════════════════════════════════════════════════════════════════
    // Assertions
    // ═══════════════════════════════════════════════════════════════════

    assert!(html_path.exists(), "Monte Carlo report should exist");

    for s in &all_stats {
        assert!(
            s.bd_p99 == 0.0,
            "{}: p99 bad debt = {:.2}, expected $0",
            s.scenario_name, s.bd_p99
        );
    }
}
