use serde::Deserialize;
use std::path::Path;
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, Deserialize)]
pub struct Kline {
    pub timestamp_ms: u64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

/// Fetch a single batch of klines from Binance (max 1000 candles).
pub fn fetch_klines(
    symbol: &str,
    interval: &str,
    start_ms: u64,
    end_ms: u64,
) -> Result<Vec<Kline>, Box<dyn std::error::Error>> {
    let url = format!(
        "https://api.binance.com/api/v3/klines?symbol={}&interval={}&startTime={}&endTime={}&limit=1000",
        symbol, interval, start_ms, end_ms
    );

    let client = reqwest::blocking::Client::new();
    let resp = client.get(&url).send()?;
    let raw: Vec<Vec<serde_json::Value>> = resp.json()?;

    let klines = raw
        .into_iter()
        .map(|row| Kline {
            timestamp_ms: row[0].as_u64().unwrap_or(0),
            open: row[1].as_str().unwrap_or("0").parse().unwrap_or(0.0),
            high: row[2].as_str().unwrap_or("0").parse().unwrap_or(0.0),
            low: row[3].as_str().unwrap_or("0").parse().unwrap_or(0.0),
            close: row[4].as_str().unwrap_or("0").parse().unwrap_or(0.0),
            volume: row[5].as_str().unwrap_or("0").parse().unwrap_or(0.0),
        })
        .collect();

    Ok(klines)
}

/// Fetch klines across a full date range, paginating in batches of 1000.
pub fn fetch_range(
    symbol: &str,
    interval: &str,
    start_ms: u64,
    end_ms: u64,
) -> Result<Vec<Kline>, Box<dyn std::error::Error>> {
    let mut all_klines = Vec::new();
    let mut cursor = start_ms;

    loop {
        if cursor >= end_ms {
            break;
        }

        let batch = fetch_klines(symbol, interval, cursor, end_ms)?;
        if batch.is_empty() {
            break;
        }

        let last_ts = batch.last().unwrap().timestamp_ms;
        all_klines.extend(batch);

        // Move cursor past the last received candle
        cursor = last_ts + 1;

        // Rate limiting: Binance allows 1200 requests/min, be conservative
        thread::sleep(Duration::from_millis(250));
    }

    Ok(all_klines)
}

/// Save klines to a CSV file.
pub fn save_csv(klines: &[Kline], path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut wtr = csv::Writer::from_path(path)?;
    wtr.write_record(["timestamp_ms", "open", "high", "low", "close", "volume"])?;

    for k in klines {
        wtr.write_record(&[
            k.timestamp_ms.to_string(),
            k.open.to_string(),
            k.high.to_string(),
            k.low.to_string(),
            k.close.to_string(),
            k.volume.to_string(),
        ])?;
    }

    wtr.flush()?;
    Ok(())
}

/// Load klines from a CSV file.
pub fn load_csv(path: &Path) -> Result<Vec<Kline>, Box<dyn std::error::Error>> {
    let mut rdr = csv::Reader::from_path(path)?;
    let mut klines = Vec::new();

    for result in rdr.records() {
        let record = result?;
        klines.push(Kline {
            timestamp_ms: record[0].parse()?,
            open: record[1].parse()?,
            high: record[2].parse()?,
            low: record[3].parse()?,
            close: record[4].parse()?,
            volume: record[5].parse()?,
        });
    }

    Ok(klines)
}
