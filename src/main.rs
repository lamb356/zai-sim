use chrono::NaiveDate;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use zai_sim::agents::*;
use zai_sim::output;
use zai_sim::report;
use zai_sim::scenario::{Scenario, ScenarioConfig};
use zai_sim::scenarios::ScenarioId;
use zai_sim::sweep::SweepEngine;

#[derive(Parser)]
#[command(name = "zai-sim", about = "Oracle-free CDP flatcoin simulator for Zcash")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Fetch historical kline data from Binance
    Fetch {
        /// Trading pair (e.g., ZECUSDT)
        #[arg(long, default_value = "ZECUSDT")]
        pair: String,

        /// Start date (YYYY-MM-DD)
        #[arg(long)]
        start: String,

        /// End date (YYYY-MM-DD)
        #[arg(long)]
        end: String,

        /// Candle interval (e.g., 1m, 5m, 1h)
        #[arg(long, default_value = "1m")]
        interval: String,

        /// Output directory for CSV files
        #[arg(long, default_value = "data")]
        output_dir: String,
    },

    /// Run a single simulation scenario
    Run {
        /// Price data CSV file
        #[arg(long)]
        prices: String,

        /// Output metrics CSV
        #[arg(long, default_value = "output/metrics.csv")]
        output: String,

        /// Number of arbitrageurs
        #[arg(long, default_value = "1")]
        arbers: usize,

        /// Number of miners
        #[arg(long, default_value = "1")]
        miners: usize,
    },

    /// Run a parameter sweep
    Sweep {
        /// Price data CSV file
        #[arg(long)]
        prices: String,

        /// Output directory for sweep results
        #[arg(long, default_value = "output/sweep")]
        output_dir: String,

        /// Parameter to sweep (e.g., "min_ratio")
        #[arg(long)]
        param: String,

        /// Comma-separated values to sweep
        #[arg(long)]
        values: String,
    },

    /// Run a stress scenario (1-13, or "all")
    Stress {
        /// Scenario ID (1-13) or 0 for all
        #[arg(long)]
        id: u8,

        /// Number of blocks to simulate
        #[arg(long, default_value = "1000")]
        blocks: usize,

        /// Output directory
        #[arg(long, default_value = "output/stress")]
        output_dir: String,

        /// Random seed
        #[arg(long, default_value = "42")]
        seed: u64,
    },

    /// Run the full 4-stage parameter sweep
    FullSweep {
        /// Number of blocks per scenario run
        #[arg(long, default_value = "500")]
        blocks: usize,

        /// Output directory
        #[arg(long, default_value = "output/full_sweep")]
        output_dir: String,

        /// Random seed
        #[arg(long, default_value = "42")]
        seed: u64,
    },
}

fn load_prices_from_csv(path: &str) -> Result<Vec<f64>, Box<dyn std::error::Error>> {
    let klines = zai_sim::data_fetcher::load_csv(std::path::Path::new(path))?;
    Ok(klines.iter().map(|k| k.close).collect())
}

fn run_scenario(
    prices: &[f64],
    config: &ScenarioConfig,
    arber_count: usize,
    miner_count: usize,
) -> Scenario {
    let mut scenario = Scenario::new(config);

    for _ in 0..arber_count {
        scenario
            .arbers
            .push(Arbitrageur::new(ArbitrageurConfig::default()));
    }
    for _ in 0..miner_count {
        scenario
            .miners
            .push(MinerAgent::new(MinerAgentConfig::default()));
    }

    scenario.run(prices);
    scenario
}

fn id_to_scenario(id: u8) -> Option<ScenarioId> {
    match id {
        1 => Some(ScenarioId::SteadyState),
        2 => Some(ScenarioId::BlackThursday),
        3 => Some(ScenarioId::FlashCrash),
        4 => Some(ScenarioId::SustainedBear),
        5 => Some(ScenarioId::TwapManipulation),
        6 => Some(ScenarioId::LiquidityCrisis),
        7 => Some(ScenarioId::BankRun),
        8 => Some(ScenarioId::BullMarket),
        9 => Some(ScenarioId::OracleComparison),
        10 => Some(ScenarioId::CombinedStress),
        11 => Some(ScenarioId::DemandShock),
        12 => Some(ScenarioId::MinerCapitulation),
        13 => Some(ScenarioId::SequencerDowntime),
        _ => None,
    }
}

fn run_stress_scenario(
    sid: ScenarioId,
    blocks: usize,
    seed: u64,
    output_dir: &str,
) -> Option<(String, report::PassFailResult, output::SummaryMetrics)> {
    let config = ScenarioConfig::default();
    let target = config.initial_redemption_price;
    println!(
        "  [{:>2}] {} — {}",
        sid as u8,
        sid.name(),
        sid.description()
    );

    let scenario =
        zai_sim::scenarios::run_stress(sid, &config, blocks, seed);

    let dir = PathBuf::from(output_dir).join(sid.name());
    let _ = output::save_all(&scenario, &config, target, &dir);

    // Generate HTML report
    let html = report::generate_report(&scenario.metrics, &config, sid.name(), target);
    let html_path = PathBuf::from(output_dir).join(format!("{}.html", sid.name()));
    let _ = report::save_report(&html, &html_path);

    let summary = output::compute_summary(&scenario.metrics, target);
    let verdict = report::evaluate_pass_fail(&scenario.metrics, target);

    println!(
        "       [{}] blocks={}, peg_dev={:.4}, liqs={}, bad_debt={:.2} -> {}",
        verdict.overall.label(),
        summary.total_blocks,
        summary.mean_peg_deviation,
        summary.total_liquidations,
        summary.total_bad_debt,
        dir.display()
    );

    Some((sid.name().to_string(), verdict, summary))
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Fetch {
            pair,
            start,
            end,
            interval,
            output_dir,
        } => {
            let start_date = NaiveDate::parse_from_str(&start, "%Y-%m-%d")
                .expect("Invalid start date (use YYYY-MM-DD)");
            let end_date = NaiveDate::parse_from_str(&end, "%Y-%m-%d")
                .expect("Invalid end date (use YYYY-MM-DD)");

            let start_ms = start_date
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc()
                .timestamp_millis() as u64;
            let end_ms = end_date
                .and_hms_opt(23, 59, 59)
                .unwrap()
                .and_utc()
                .timestamp_millis() as u64;

            println!(
                "Fetching {} {} from {} to {}...",
                pair, interval, start, end
            );

            match zai_sim::data_fetcher::fetch_range(&pair, &interval, start_ms, end_ms) {
                Ok(klines) => {
                    println!("Fetched {} candles", klines.len());

                    let filename = format!(
                        "{}_{}_{}_{}.csv",
                        pair.to_lowercase(),
                        interval,
                        start,
                        end
                    );
                    let path = PathBuf::from(&output_dir).join(&filename);

                    match zai_sim::data_fetcher::save_csv(&klines, &path) {
                        Ok(()) => println!("Saved to {}", path.display()),
                        Err(e) => eprintln!("Error saving CSV: {}", e),
                    }
                }
                Err(e) => eprintln!("Error fetching data: {}", e),
            }
        }

        Commands::Run {
            prices,
            output,
            arbers,
            miners,
        } => {
            let price_data = match load_prices_from_csv(&prices) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("Error loading prices: {}", e);
                    return;
                }
            };

            println!(
                "Running scenario: {} blocks, {} arbers, {} miners",
                price_data.len(),
                arbers,
                miners
            );

            let config = ScenarioConfig::default();
            let scenario = run_scenario(&price_data, &config, arbers, miners);

            let out_path = PathBuf::from(&output);
            match scenario.save_metrics_csv(&out_path) {
                Ok(()) => println!(
                    "Saved {} block metrics to {}",
                    scenario.metrics.len(),
                    out_path.display()
                ),
                Err(e) => eprintln!("Error saving metrics: {}", e),
            }
        }

        Commands::Sweep {
            prices,
            output_dir,
            param,
            values,
        } => {
            let price_data = match load_prices_from_csv(&prices) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("Error loading prices: {}", e);
                    return;
                }
            };

            let values: Vec<f64> = values
                .split(',')
                .map(|v| v.trim().parse::<f64>().expect("Invalid sweep value"))
                .collect();

            println!(
                "Sweeping {} over {:?} ({} blocks each)",
                param,
                values,
                price_data.len()
            );

            for val in &values {
                let mut config = ScenarioConfig::default();

                match param.as_str() {
                    "min_ratio" => config.cdp_config.min_ratio = *val,
                    "swap_fee" => config.amm_swap_fee = *val,
                    "liquidation_penalty" => config.cdp_config.liquidation_penalty = *val,
                    "stability_fee" => config.cdp_config.stability_fee_rate = *val,
                    _ => {
                        eprintln!("Unknown parameter: {}", param);
                        return;
                    }
                }

                let scenario = run_scenario(&price_data, &config, 1, 1);

                let filename = format!("{}_{:.4}.csv", param, val);
                let out_path = PathBuf::from(&output_dir).join(&filename);
                match scenario.save_metrics_csv(&out_path) {
                    Ok(()) => println!("  {}={:.4} -> {}", param, val, out_path.display()),
                    Err(e) => eprintln!("  Error: {}", e),
                }
            }
        }

        Commands::Stress {
            id,
            blocks,
            output_dir,
            seed,
        } => {
            if id == 0 {
                println!("Running all 13 stress scenarios ({} blocks each):", blocks);
                let mut entries = Vec::new();
                for sid in ScenarioId::all() {
                    if let Some(entry) =
                        run_stress_scenario(sid, blocks, seed, &output_dir)
                    {
                        entries.push(entry);
                    }
                }
                // Generate master summary
                let master = report::generate_master_summary(&entries);
                let master_path = PathBuf::from(&output_dir).join("index.html");
                match report::save_report(&master, &master_path) {
                    Ok(()) => println!("\nMaster summary: {}", master_path.display()),
                    Err(e) => eprintln!("Error saving master summary: {}", e),
                }
            } else {
                match id_to_scenario(id) {
                    Some(sid) => {
                        println!("Running stress scenario ({} blocks):", blocks);
                        run_stress_scenario(sid, blocks, seed, &output_dir);
                    }
                    None => eprintln!("Invalid scenario ID: {} (must be 1-13)", id),
                }
            }
        }

        Commands::FullSweep {
            blocks,
            output_dir,
            seed,
        } => {
            println!(
                "Running 4-stage parameter sweep ({} blocks per scenario)...",
                blocks
            );

            let engine = SweepEngine::new(blocks, seed, 50.0);
            let results = engine.run_full_sweep();

            let out_path = PathBuf::from(&output_dir);
            match output::save_sweep_results(&results, &out_path.join("sweep_results.csv")) {
                Ok(()) => println!("Saved sweep results to {}", out_path.display()),
                Err(e) => eprintln!("Error saving results: {}", e),
            }

            // Print top results
            println!("\nTop configurations:");
            for (i, r) in results.iter().take(3).enumerate() {
                let params_str: Vec<String> = r
                    .params
                    .iter()
                    .map(|(n, v)| format!("{}={:.4}", n, v))
                    .collect();
                println!(
                    "  #{}: score={:.6} [{}]",
                    i + 1,
                    r.overall_score,
                    params_str.join(", ")
                );
            }
        }
    }
}
