use crate::circuit_breaker::BreakerAction;
use crate::scenario::{BlockMetrics, Scenario, ScenarioConfig};
use crate::sweep::SweepResult;
use std::path::Path;

/// A discrete event extracted from simulation metrics.
#[derive(Debug)]
pub struct Event {
    pub block: u64,
    pub event_type: String,
    pub details: String,
}

/// Summary statistics for a simulation run.
#[derive(Debug)]
pub struct SummaryMetrics {
    pub total_blocks: u64,
    pub mean_peg_deviation: f64,
    pub max_peg_deviation: f64,
    pub final_peg_deviation: f64,
    pub total_liquidations: u32,
    pub total_bad_debt: f64,
    pub breaker_triggers: u32,
    pub halt_blocks: u64,
    pub pause_blocks: u64,
    pub mean_amm_price: f64,
    pub min_amm_price: f64,
    pub max_amm_price: f64,
    pub final_amm_price: f64,
    pub final_redemption_price: f64,
    pub final_debt_ceiling: f64,
}

/// Extract discrete events from simulation metrics.
pub fn extract_events(metrics: &[BlockMetrics]) -> Vec<Event> {
    let mut events = Vec::new();

    for m in metrics {
        if m.liquidation_count > 0 {
            events.push(Event {
                block: m.block,
                event_type: "liquidation".to_string(),
                details: format!(
                    "count={},bad_debt={:.2}",
                    m.liquidation_count, m.bad_debt
                ),
            });
        }

        for action in &m.breaker_actions {
            match action {
                BreakerAction::None => {}
                BreakerAction::PauseMinting { blocks, reason } => {
                    events.push(Event {
                        block: m.block,
                        event_type: "pause_minting".to_string(),
                        details: format!("blocks={},{}", blocks, reason),
                    });
                }
                BreakerAction::ReduceDebtCeiling { new_ceiling, reason } => {
                    events.push(Event {
                        block: m.block,
                        event_type: "reduce_ceiling".to_string(),
                        details: format!("ceiling={:.0},{}", new_ceiling, reason),
                    });
                }
                BreakerAction::EmergencyHalt { reason } => {
                    events.push(Event {
                        block: m.block,
                        event_type: "emergency_halt".to_string(),
                        details: reason.clone(),
                    });
                }
            }
        }
    }

    events
}

/// Compute summary statistics from simulation metrics.
pub fn compute_summary(metrics: &[BlockMetrics], target_price: f64) -> SummaryMetrics {
    if metrics.is_empty() {
        return SummaryMetrics {
            total_blocks: 0,
            mean_peg_deviation: 0.0,
            max_peg_deviation: 0.0,
            final_peg_deviation: 0.0,
            total_liquidations: 0,
            total_bad_debt: 0.0,
            breaker_triggers: 0,
            halt_blocks: 0,
            pause_blocks: 0,
            mean_amm_price: 0.0,
            min_amm_price: 0.0,
            max_amm_price: 0.0,
            final_amm_price: 0.0,
            final_redemption_price: 0.0,
            final_debt_ceiling: 0.0,
        };
    }

    let n = metrics.len() as f64;

    let deviations: Vec<f64> = metrics
        .iter()
        .map(|m| ((m.amm_spot_price - target_price) / target_price).abs())
        .collect();

    let amm_prices: Vec<f64> = metrics.iter().map(|m| m.amm_spot_price).collect();

    let total_liqs: u32 = metrics.iter().map(|m| m.liquidation_count).sum();

    let trigger_count: u32 = metrics
        .iter()
        .map(|m| {
            m.breaker_actions
                .iter()
                .filter(|a| **a != BreakerAction::None)
                .count() as u32
        })
        .sum();

    let last = metrics.last().unwrap();

    SummaryMetrics {
        total_blocks: metrics.len() as u64,
        mean_peg_deviation: deviations.iter().sum::<f64>() / n,
        max_peg_deviation: deviations.iter().cloned().fold(0.0_f64, f64::max),
        final_peg_deviation: *deviations.last().unwrap(),
        total_liquidations: total_liqs,
        total_bad_debt: last.bad_debt,
        breaker_triggers: trigger_count,
        halt_blocks: metrics.iter().filter(|m| m.halted).count() as u64,
        pause_blocks: metrics.iter().filter(|m| m.minting_paused).count() as u64,
        mean_amm_price: amm_prices.iter().sum::<f64>() / n,
        min_amm_price: amm_prices.iter().cloned().fold(f64::INFINITY, f64::min),
        max_amm_price: amm_prices
            .iter()
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max),
        final_amm_price: last.amm_spot_price,
        final_redemption_price: last.redemption_price,
        final_debt_ceiling: last.debt_ceiling,
    }
}

/// Save events to CSV.
pub fn save_events_csv(
    events: &[Event],
    path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut wtr = csv::Writer::from_path(path)?;
    wtr.write_record(["block", "event_type", "details"])?;

    for e in events {
        wtr.write_record(&[e.block.to_string(), e.event_type.clone(), e.details.clone()])?;
    }
    wtr.flush()?;
    Ok(())
}

/// Save summary metrics to JSON.
pub fn save_metrics_json(
    summary: &SummaryMetrics,
    path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let json = format!(
        r#"{{
  "total_blocks": {},
  "peg": {{
    "mean_deviation": {:.6},
    "max_deviation": {:.6},
    "final_deviation": {:.6}
  }},
  "liquidations": {{
    "total": {},
    "bad_debt": {:.2}
  }},
  "breakers": {{
    "trigger_count": {},
    "halt_blocks": {},
    "pause_blocks": {}
  }},
  "amm_price": {{
    "mean": {:.4},
    "min": {:.4},
    "max": {:.4},
    "final": {:.4}
  }},
  "final_redemption_price": {:.6},
  "final_debt_ceiling": {:.0}
}}"#,
        summary.total_blocks,
        summary.mean_peg_deviation,
        summary.max_peg_deviation,
        summary.final_peg_deviation,
        summary.total_liquidations,
        summary.total_bad_debt,
        summary.breaker_triggers,
        summary.halt_blocks,
        summary.pause_blocks,
        summary.mean_amm_price,
        summary.min_amm_price,
        summary.max_amm_price,
        summary.final_amm_price,
        summary.final_redemption_price,
        summary.final_debt_ceiling,
    );

    std::fs::write(path, json)?;
    Ok(())
}

/// Save configuration to TOML format.
pub fn save_config_toml(
    config: &ScenarioConfig,
    path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let toml = format!(
        r#"[amm]
initial_zec = {:.1}
initial_zai = {:.1}
swap_fee = {:.4}

[cdp]
min_ratio = {:.2}
liquidation_penalty = {:.2}
debt_floor = {:.1}
stability_fee_rate = {:.4}
twap_window = {}

[controller]
initial_redemption_price = {:.2}

[circuit_breaker.twap]
max_twap_change_pct = {:.2}
short_window = {}
long_window = {}
pause_blocks = {}

[circuit_breaker.cascade]
max_liquidations_in_window = {}
window_blocks = {}
pause_blocks = {}

[circuit_breaker.debt_ceiling]
initial_ceiling = {:.0}
min_ceiling = {:.0}
reduction_factor = {:.2}
growth_rate_per_block = {:.4}
deviation_threshold = {:.2}
"#,
        config.amm_initial_zec,
        config.amm_initial_zai,
        config.amm_swap_fee,
        config.cdp_config.min_ratio,
        config.cdp_config.liquidation_penalty,
        config.cdp_config.debt_floor,
        config.cdp_config.stability_fee_rate,
        config.cdp_config.twap_window,
        config.initial_redemption_price,
        config.twap_breaker_config.max_twap_change_pct,
        config.twap_breaker_config.short_window,
        config.twap_breaker_config.long_window,
        config.twap_breaker_config.pause_blocks,
        config.cascade_breaker_config.max_liquidations_in_window,
        config.cascade_breaker_config.window_blocks,
        config.cascade_breaker_config.pause_blocks,
        config.debt_ceiling_config.initial_ceiling,
        config.debt_ceiling_config.min_ceiling,
        config.debt_ceiling_config.reduction_factor,
        config.debt_ceiling_config.growth_rate_per_block,
        config.debt_ceiling_config.deviation_threshold,
    );

    std::fs::write(path, toml)?;
    Ok(())
}

/// Save sweep results to CSV.
pub fn save_sweep_results(
    results: &[SweepResult],
    path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut wtr = csv::Writer::from_path(path)?;

    if let Some(first) = results.first() {
        let mut header: Vec<String> = first.params.iter().map(|(n, _)| n.clone()).collect();
        header.push("overall_score".to_string());
        for (sid, _) in &first.scores {
            header.push(format!("score_{}", sid.name()));
        }
        wtr.write_record(&header)?;
    }

    for r in results {
        let mut row: Vec<String> = r.params.iter().map(|(_, v)| format!("{:.6}", v)).collect();
        row.push(format!("{:.6}", r.overall_score));
        for (_, s) in &r.scores {
            row.push(format!("{:.6}", s));
        }
        wtr.write_record(&row)?;
    }

    wtr.flush()?;
    Ok(())
}

/// Save all outputs for a scenario run to a directory.
pub fn save_all(
    scenario: &Scenario,
    config: &ScenarioConfig,
    target_price: f64,
    output_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(output_dir)?;

    scenario.save_metrics_csv(&output_dir.join("timeseries.csv"))?;

    let events = extract_events(&scenario.metrics);
    save_events_csv(&events, &output_dir.join("events.csv"))?;

    let summary = compute_summary(&scenario.metrics, target_price);
    save_metrics_json(&summary, &output_dir.join("metrics.json"))?;

    save_config_toml(config, &output_dir.join("config.toml"))?;

    Ok(())
}
