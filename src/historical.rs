//! Historical price data loader and interpolation for replay testing.
//!
//! Loads hourly OHLCV CSV files (CryptoCompare format) and linearly
//! interpolates between hourly close prices to produce per-block prices
//! (48 blocks per hour at 75-second block time).

use crate::controller::ControllerConfig;
use crate::scenario::ScenarioConfig;
use std::path::Path;

/// Load hourly close prices from a CryptoCompare CSV file.
///
/// Expected columns: timestamp,datetime,open,high,low,close,volume_from,volume_to
/// Returns the `close` column as `Vec<f64>`.
pub fn load_hourly_prices(csv_path: &str) -> Vec<f64> {
    let path = Path::new(csv_path);
    let mut reader = csv::Reader::from_path(path)
        .unwrap_or_else(|e| panic!("Failed to open CSV {}: {}", csv_path, e));

    let mut prices = Vec::new();
    for result in reader.records() {
        let record = result.expect("Failed to parse CSV row");
        // close is column index 5 (0-indexed)
        let close: f64 = record
            .get(5)
            .expect("Missing close column")
            .parse()
            .expect("Failed to parse close price");
        prices.push(close);
    }

    assert!(!prices.is_empty(), "CSV {} contained no data rows", csv_path);
    prices
}

/// Linearly interpolate hourly prices to per-block prices.
///
/// For N hourly prices, produces (N-1) * blocks_per_hour block prices.
/// Each hour is divided into `blocks_per_hour` equal segments.
/// The first block of each hour equals the hourly close price.
pub fn interpolate_to_blocks(hourly_prices: &[f64], blocks_per_hour: usize) -> Vec<f64> {
    assert!(
        hourly_prices.len() >= 2,
        "Need at least 2 hourly prices to interpolate"
    );

    let mut block_prices = Vec::with_capacity((hourly_prices.len() - 1) * blocks_per_hour);

    for i in 0..hourly_prices.len() - 1 {
        let p0 = hourly_prices[i];
        let p1 = hourly_prices[i + 1];
        for j in 0..blocks_per_hour {
            let t = j as f64 / blocks_per_hour as f64;
            block_prices.push(p0 + (p1 - p0) * t);
        }
    }

    block_prices
}

/// Create a ScenarioConfig for historical replay.
///
/// Mirrors `config_5m_200cr` but adjusts AMM reserves so the starting
/// spot price matches the first hourly close price from the CSV.
///
/// - `amm_initial_zec = 100,000`
/// - `amm_initial_zai = 100,000 * first_price`
/// - `initial_redemption_price = first_price`
/// - 200% collateral ratio, 240-block TWAP, Tick controller
pub fn config_for_historical(first_price: f64) -> ScenarioConfig {
    let mut config = ScenarioConfig::default();
    config.amm_initial_zec = 100_000.0;
    config.amm_initial_zai = 100_000.0 * first_price;
    config.cdp_config.min_ratio = 2.0;
    config.cdp_config.twap_window = 240;
    config.controller_config = ControllerConfig::default_tick();
    config.initial_redemption_price = first_price;
    config
}
