use crate::output::SummaryMetrics;
use crate::scenario::{BlockMetrics, ScenarioConfig};
use std::path::Path;

const BLOCKS_PER_HOUR: u64 = 48;

// ═══════════════════════════════════════════════════════════════════════
// Pass / Fail types
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Pass,
    SoftFail,
    HardFail,
}

impl Verdict {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::SoftFail => "SOFT FAIL",
            Self::HardFail => "HARD FAIL",
        }
    }

    pub fn css_class(&self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::SoftFail => "soft-fail",
            Self::HardFail => "hard-fail",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CriterionResult {
    pub name: String,
    pub passed: bool,
    pub severity: Verdict,
    pub details: String,
}

#[derive(Debug, Clone)]
pub struct PassFailResult {
    pub overall: Verdict,
    pub criteria: Vec<CriterionResult>,
}

// ═══════════════════════════════════════════════════════════════════════
// Pass / Fail evaluation
// ═══════════════════════════════════════════════════════════════════════

pub fn evaluate_pass_fail(metrics: &[BlockMetrics], target_price: f64) -> PassFailResult {
    let mut criteria = Vec::new();
    let mut worst = Verdict::Pass;

    // --- Hard fail: insolvency ---
    let insolvent = metrics.iter().any(|m| {
        if m.total_debt > 0.0 {
            let collateral_value = m.total_collateral * m.twap_price;
            collateral_value < m.total_debt
        } else {
            false
        }
    });
    criteria.push(CriterionResult {
        name: "Solvency".into(),
        passed: !insolvent,
        severity: Verdict::HardFail,
        details: if insolvent {
            "System became insolvent (collateral value < total debt)".into()
        } else {
            "System remained solvent throughout".into()
        },
    });
    if insolvent {
        worst = Verdict::HardFail;
    }

    // --- Hard fail: bad debt > 5% ---
    let max_debt = metrics
        .iter()
        .map(|m| m.total_debt)
        .fold(1.0_f64, f64::max);
    let final_bad_debt = metrics.last().map(|m| m.bad_debt).unwrap_or(0.0);
    let bad_debt_pct = final_bad_debt / max_debt * 100.0;
    let bad_debt_fail = bad_debt_pct > 5.0;
    criteria.push(CriterionResult {
        name: "Bad debt < 5%".into(),
        passed: !bad_debt_fail,
        severity: Verdict::HardFail,
        details: format!("Bad debt ratio: {:.2}% of peak debt", bad_debt_pct),
    });
    if bad_debt_fail {
        worst = Verdict::HardFail;
    }

    // --- Hard fail: death spiral ---
    let death_spiral = if metrics.len() > 200 {
        let initial = metrics[0].amm_spot_price;
        let final_price = metrics.last().unwrap().amm_spot_price;
        // Price dropped >90% and last 100 blocks show no recovery
        let dropped = final_price < initial * 0.1;
        let last_100: Vec<f64> = metrics[metrics.len().saturating_sub(100)..]
            .iter()
            .map(|m| m.amm_spot_price)
            .collect();
        let no_recovery = last_100.iter().all(|&p| p < initial * 0.15);
        dropped && no_recovery
    } else {
        false
    };
    criteria.push(CriterionResult {
        name: "No death spiral".into(),
        passed: !death_spiral,
        severity: Verdict::HardFail,
        details: if death_spiral {
            "Price collapsed >90% with no recovery".into()
        } else {
            "No death spiral detected".into()
        },
    });
    if death_spiral {
        worst = Verdict::HardFail;
    }

    // --- Soft fail: peg deviation >20% for >1 hour (48 blocks) ---
    let mut consecutive_deviation = 0u64;
    let mut max_consecutive = 0u64;
    for m in metrics {
        let dev = ((m.amm_spot_price - target_price) / target_price).abs();
        if dev > 0.20 {
            consecutive_deviation += 1;
            max_consecutive = max_consecutive.max(consecutive_deviation);
        } else {
            consecutive_deviation = 0;
        }
    }
    let sustained_deviation = max_consecutive > BLOCKS_PER_HOUR;
    criteria.push(CriterionResult {
        name: "Peg deviation < 20% sustained".into(),
        passed: !sustained_deviation,
        severity: Verdict::SoftFail,
        details: format!(
            "Max consecutive blocks with >20% deviation: {} (limit: {})",
            max_consecutive, BLOCKS_PER_HOUR
        ),
    });
    if sustained_deviation && worst == Verdict::Pass {
        worst = Verdict::SoftFail;
    }

    // --- Soft fail: recovery > 72 hours (3456 blocks) ---
    let recovery_blocks = compute_recovery_blocks(metrics, target_price, 0.10);
    let slow_recovery = recovery_blocks > BLOCKS_PER_HOUR * 72;
    criteria.push(CriterionResult {
        name: "Recovery < 72 hours".into(),
        passed: !slow_recovery,
        severity: Verdict::SoftFail,
        details: format!(
            "Recovery time: {} blocks ({:.1} hours)",
            recovery_blocks,
            recovery_blocks as f64 / BLOCKS_PER_HOUR as f64
        ),
    });
    if slow_recovery && worst == Verdict::Pass {
        worst = Verdict::SoftFail;
    }

    // --- Pass criteria: recovery < 24h ---
    let fast_recovery = recovery_blocks <= BLOCKS_PER_HOUR * 24;
    criteria.push(CriterionResult {
        name: "Recovery < 24 hours".into(),
        passed: fast_recovery,
        severity: Verdict::SoftFail,
        details: format!(
            "Recovery: {} blocks ({:.1}h)",
            recovery_blocks,
            recovery_blocks as f64 / BLOCKS_PER_HOUR as f64
        ),
    });

    // --- Pass criteria: volatility ratio < 0.3 ---
    let (mean_price, std_price) = price_stats(metrics);
    let vol_ratio = if mean_price > 0.0 {
        std_price / mean_price
    } else {
        0.0
    };
    let low_vol = vol_ratio < 0.3;
    criteria.push(CriterionResult {
        name: "Volatility ratio < 0.3".into(),
        passed: low_vol,
        severity: Verdict::SoftFail,
        details: format!("Volatility ratio: {:.4} (std/mean)", vol_ratio),
    });
    if !low_vol && worst == Verdict::Pass {
        worst = Verdict::SoftFail;
    }

    PassFailResult {
        overall: worst,
        criteria,
    }
}

fn compute_recovery_blocks(metrics: &[BlockMetrics], target: f64, threshold: f64) -> u64 {
    let mut first_deviation: Option<u64> = None;
    let mut last_deviation: Option<u64> = None;

    for m in metrics {
        let dev = ((m.amm_spot_price - target) / target).abs();
        if dev > threshold {
            if first_deviation.is_none() {
                first_deviation = Some(m.block);
            }
            last_deviation = Some(m.block);
        }
    }

    match (first_deviation, last_deviation) {
        (Some(first), Some(last)) => last - first,
        _ => 0,
    }
}

fn price_stats(metrics: &[BlockMetrics]) -> (f64, f64) {
    if metrics.is_empty() {
        return (0.0, 0.0);
    }
    let n = metrics.len() as f64;
    let mean = metrics.iter().map(|m| m.amm_spot_price).sum::<f64>() / n;
    let variance = metrics
        .iter()
        .map(|m| (m.amm_spot_price - mean).powi(2))
        .sum::<f64>()
        / n;
    (mean, variance.sqrt())
}

// ═══════════════════════════════════════════════════════════════════════
// HTML helpers
// ═══════════════════════════════════════════════════════════════════════

fn js_array_f64(data: &[f64]) -> String {
    let items: Vec<String> = data.iter().map(|v| format!("{:.4}", v)).collect();
    format!("[{}]", items.join(","))
}

fn js_array_u64(data: &[u64]) -> String {
    let items: Vec<String> = data.iter().map(|v| v.to_string()).collect();
    format!("[{}]", items.join(","))
}

fn js_array_u32(data: &[u32]) -> String {
    let items: Vec<String> = data.iter().map(|v| v.to_string()).collect();
    format!("[{}]", items.join(","))
}

// ═══════════════════════════════════════════════════════════════════════
// Main report generation
// ═══════════════════════════════════════════════════════════════════════

pub fn generate_report(
    metrics: &[BlockMetrics],
    config: &ScenarioConfig,
    scenario_name: &str,
    target_price: f64,
) -> String {
    let verdict = evaluate_pass_fail(metrics, target_price);
    let summary = crate::output::compute_summary(metrics, target_price);

    // Extract data series
    let blocks: Vec<u64> = metrics.iter().map(|m| m.block).collect();
    let ext_prices: Vec<f64> = metrics.iter().map(|m| m.external_price).collect();
    let spot_prices: Vec<f64> = metrics.iter().map(|m| m.amm_spot_price).collect();
    let twap_prices: Vec<f64> = metrics.iter().map(|m| m.twap_price).collect();
    let redemption_prices: Vec<f64> = metrics.iter().map(|m| m.redemption_price).collect();
    let redemption_rates: Vec<f64> = metrics.iter().map(|m| m.redemption_rate).collect();
    let total_debt: Vec<f64> = metrics.iter().map(|m| m.total_debt).collect();
    let reserve_zec: Vec<f64> = metrics.iter().map(|m| m.amm_reserve_zec).collect();
    let reserve_zai: Vec<f64> = metrics.iter().map(|m| m.amm_reserve_zai).collect();
    let liq_counts: Vec<u32> = metrics.iter().map(|m| m.liquidation_count).collect();
    let bad_debt: Vec<f64> = metrics.iter().map(|m| m.bad_debt).collect();
    let total_collateral: Vec<f64> = metrics.iter().map(|m| m.total_collateral).collect();
    let total_lp: Vec<f64> = metrics.iter().map(|m| m.total_lp_shares).collect();
    let arber_zai: Vec<f64> = metrics.iter().map(|m| m.arber_zai_total).collect();
    let arber_zec: Vec<f64> = metrics.iter().map(|m| m.arber_zec_total).collect();
    let cum_fees: Vec<f64> = metrics.iter().map(|m| m.cumulative_fees_zai).collect();
    let cum_il: Vec<f64> = metrics.iter().map(|m| m.cumulative_il_pct * 100.0).collect();
    let zombie_counts: Vec<u32> = metrics.iter().map(|m| m.zombie_vault_count).collect();
    let cr_ext: Vec<f64> = metrics
        .iter()
        .map(|m| {
            if m.total_debt > 0.0 {
                m.total_collateral * m.external_price / m.total_debt
            } else {
                0.0
            }
        })
        .collect();

    // Derived series
    let amm_k: Vec<f64> = metrics
        .iter()
        .map(|m| m.amm_reserve_zec * m.amm_reserve_zai)
        .collect();
    let coll_ratio: Vec<f64> = metrics
        .iter()
        .map(|m| {
            if m.total_debt > 0.0 {
                m.total_collateral * m.twap_price / m.total_debt
            } else {
                0.0
            }
        })
        .collect();

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>ZAI Report — {scenario_name}</title>
<script src="https://cdn.jsdelivr.net/npm/chart.js@4"></script>
<style>
*{{margin:0;padding:0;box-sizing:border-box}}
body{{font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif;background:#f5f5f5;color:#333}}
header{{background:#1a1a2e;color:#fff;padding:24px 32px;display:flex;align-items:center;gap:20px}}
header h1{{font-size:1.4em;font-weight:500}}
header h2{{font-size:1.1em;font-weight:300;opacity:0.8}}
.badge{{padding:6px 16px;border-radius:4px;font-weight:700;font-size:0.9em;letter-spacing:0.5px}}
.badge.pass{{background:#34a853;color:#fff}}
.badge.soft-fail{{background:#ea8c00;color:#fff}}
.badge.hard-fail{{background:#ea4335;color:#fff}}
main{{max-width:1400px;margin:0 auto;padding:24px}}
section{{background:#fff;border-radius:8px;box-shadow:0 1px 3px rgba(0,0,0,0.1);padding:24px;margin-bottom:20px}}
section h3{{font-size:1.1em;margin-bottom:16px;color:#1a1a2e;border-bottom:2px solid #e0e0e0;padding-bottom:8px}}
.metrics-grid{{display:grid;grid-template-columns:repeat(auto-fill,minmax(180px,1fr));gap:12px}}
.metric{{background:#f8f9fa;border-radius:6px;padding:12px;text-align:center}}
.metric .label{{display:block;font-size:0.75em;color:#666;text-transform:uppercase;letter-spacing:0.5px}}
.metric .value{{display:block;font-size:1.3em;font-weight:600;margin-top:4px}}
table{{width:100%;border-collapse:collapse;font-size:0.9em}}
th,td{{padding:8px 12px;text-align:left;border-bottom:1px solid #e0e0e0}}
th{{background:#f8f9fa;font-weight:600}}
.chart-row{{display:grid;grid-template-columns:1fr 1fr;gap:20px;margin-bottom:20px}}
@media(max-width:900px){{.chart-row{{grid-template-columns:1fr}}}}
.chart-box{{background:#fff;border-radius:8px;box-shadow:0 1px 3px rgba(0,0,0,0.1);padding:16px}}
.chart-box h4{{font-size:0.95em;margin-bottom:8px;color:#555}}
canvas{{width:100%!important;height:300px!important}}
.criterion-row td:first-child{{font-weight:600}}
.crit-pass{{color:#34a853}}
.crit-fail{{color:#ea4335}}
.crit-warn{{color:#ea8c00}}
footer{{text-align:center;padding:16px;color:#999;font-size:0.8em}}
</style>
</head>
<body>
<header>
 <div>
  <h1>ZAI Simulation Report</h1>
  <h2>{scenario_name}</h2>
 </div>
 <span class="badge {verdict_class}">{verdict_label}</span>
</header>
<main>

<section>
<h3>Executive Summary</h3>
<div class="metrics-grid">
 <div class="metric"><span class="label">Total Blocks</span><span class="value">{total_blocks}</span></div>
 <div class="metric"><span class="label">Mean Peg Dev</span><span class="value">{mean_dev:.2}%</span></div>
 <div class="metric"><span class="label">Max Peg Dev</span><span class="value">{max_dev:.2}%</span></div>
 <div class="metric"><span class="label">Total Liquidations</span><span class="value">{total_liqs}</span></div>
 <div class="metric"><span class="label">Bad Debt</span><span class="value">{bad_debt_total:.2}</span></div>
 <div class="metric"><span class="label">Breaker Triggers</span><span class="value">{breaker_triggers}</span></div>
 <div class="metric"><span class="label">Halt Blocks</span><span class="value">{halt_blocks}</span></div>
 <div class="metric"><span class="label">Final AMM Price</span><span class="value">{final_price:.2}</span></div>
</div>
</section>

<section>
<h3>Parameters</h3>
<table>
<tr><th>Parameter</th><th>Value</th></tr>
<tr><td>AMM Initial ZEC</td><td>{amm_zec:.0}</td></tr>
<tr><td>AMM Initial ZAI</td><td>{amm_zai:.0}</td></tr>
<tr><td>Swap Fee</td><td>{swap_fee:.4}</td></tr>
<tr><td>Min Collateral Ratio</td><td>{min_ratio:.2}</td></tr>
<tr><td>Liquidation Penalty</td><td>{liq_penalty:.2}</td></tr>
<tr><td>Stability Fee Rate</td><td>{stab_fee:.4}</td></tr>
<tr><td>Debt Floor</td><td>{debt_floor:.0}</td></tr>
<tr><td>TWAP Breaker Threshold</td><td>{twap_thresh:.2}%</td></tr>
<tr><td>Cascade Max Liquidations</td><td>{cascade_max}</td></tr>
<tr><td>Debt Ceiling</td><td>{debt_ceil:.0}</td></tr>
<tr><td>Target Price</td><td>{target_price:.2}</td></tr>
</table>
</section>

<div class="chart-row">
 <div class="chart-box"><h4>Price Comparison</h4><canvas id="c1"></canvas></div>
 <div class="chart-box"><h4>System Health</h4><canvas id="c2"></canvas></div>
</div>
<div class="chart-row">
 <div class="chart-box"><h4>Liquidation Activity</h4><canvas id="c3"></canvas></div>
 <div class="chart-box"><h4>AMM State</h4><canvas id="c4"></canvas></div>
</div>
<div class="chart-row">
 <div class="chart-box"><h4>Controller Response</h4><canvas id="c5"></canvas></div>
 <div class="chart-box"><h4>Agent Activity</h4><canvas id="c6"></canvas></div>
</div>
<div class="chart-row">
 <div class="chart-box"><h4>AMM vs External Price Gap</h4><canvas id="c7"></canvas></div>
 <div class="chart-box"><h4>Zombie Vault CR Gap</h4><canvas id="c8"></canvas></div>
</div>
<div class="chart-row">
 <div class="chart-box"><h4>Arber Capital</h4><canvas id="c9"></canvas></div>
 <div class="chart-box"><h4>LP Economics</h4><canvas id="c10"></canvas></div>
</div>

<section>
<h3>Pass / Fail Criteria</h3>
<table>
<tr><th>Criterion</th><th>Result</th><th>Severity</th><th>Details</th></tr>
{criteria_rows}
</table>
</section>

<section>
<h3>Data Export</h3>
<div style="display:flex;gap:12px;flex-wrap:wrap">
<button onclick="downloadCSV()" style="padding:8px 20px;background:#4285f4;color:#fff;border:none;border-radius:4px;cursor:pointer;font-size:0.9em">Download CSV</button>
<button onclick="downloadConfig()" style="padding:8px 20px;background:#34a853;color:#fff;border:none;border-radius:4px;cursor:pointer;font-size:0.9em">Download Config JSON</button>
<button onclick="downloadSummary()" style="padding:8px 20px;background:#9c27b0;color:#fff;border:none;border-radius:4px;cursor:pointer;font-size:0.9em">Download Summary JSON</button>
</div>
</section>

</main>
<footer>Generated by zai-sim</footer>

<script>
const B={js_blocks};
const D={{
 ext:{js_ext},
 spot:{js_spot},
 twap:{js_twap},
 redp:{js_redp},
 redr:{js_redr},
 debt:{js_debt},
 rzec:{js_rzec},
 rzai:{js_rzai},
 liqs:{js_liqs},
 bd:{js_bd},
 coll:{js_coll},
 cr:{js_cr},
 k:{js_k},
 lp:{js_lp},
 arb:{js_arb},
 arbzec:{js_arb_zec},
 fees:{js_fees},
 il:{js_il},
 crext:{js_cr_ext},
 zombies:{js_zombies}
}};
const mkDs=(l,c,d,o)=>{{let s={{label:l,data:d,borderColor:c,backgroundColor:c+'22',borderWidth:1.5,pointRadius:0,fill:false,tension:0.1}};if(o)Object.assign(s,o);return s}};
const lineOpts=(title,yLabel,extra)=>{{let o={{responsive:true,maintainAspectRatio:false,plugins:{{title:{{display:true,text:title}},legend:{{position:'bottom',labels:{{boxWidth:12,font:{{size:11}}}}}}}},scales:{{x:{{title:{{display:true,text:'Block'}},ticks:{{maxTicksLimit:10}}}},y:{{title:{{display:true,text:yLabel}},beginAtZero:false}}}}}};if(extra)Object.assign(o.scales,extra);return o}};
const y2={{y2:{{position:'right',grid:{{drawOnChartArea:false}},title:{{display:true,text:''}}}}}};

// 1. Price Comparison
new Chart(document.getElementById('c1'),{{type:'line',data:{{labels:B,datasets:[
 mkDs('External','#4285f4',D.ext),
 mkDs('Spot','#ea8c00',D.spot),
 mkDs('TWAP','#34a853',D.twap),
 mkDs('Redemption','#ea4335',D.redp,{{borderDash:[6,3]}})
]}},options:lineOpts('Price Comparison','ZAI/ZEC Price')}});

// 2. System Health
new Chart(document.getElementById('c2'),{{type:'line',data:{{labels:B,datasets:[
 mkDs('Collateral Ratio','#9c27b0',D.cr),
 mkDs('Total Debt','#009688',D.debt,{{yAxisID:'y2'}}),
 mkDs('AMM ZAI Reserve','#ff9800',D.rzai,{{yAxisID:'y2'}})
]}},options:lineOpts('System Health','Collateral Ratio',{{y2:{{position:'right',grid:{{drawOnChartArea:false}},title:{{display:true,text:'ZAI'}}}}}})
}});

// 3. Liquidation Activity
new Chart(document.getElementById('c3'),{{type:'line',data:{{labels:B,datasets:[
 mkDs('Liquidations','#e91e63',D.liqs,{{type:'bar',backgroundColor:'#e91e6366'}}),
 mkDs('Bad Debt','#ea4335',D.bd,{{yAxisID:'y2'}})
]}},options:lineOpts('Liquidation Activity','Count',{{y2:{{position:'right',grid:{{drawOnChartArea:false}},title:{{display:true,text:'Cumulative Bad Debt'}}}}}})
}});

// 4. AMM State
new Chart(document.getElementById('c4'),{{type:'line',data:{{labels:B,datasets:[
 mkDs('Reserve ZEC','#4285f4',D.rzec),
 mkDs('Reserve ZAI','#ea8c00',D.rzai),
 mkDs('k','#757575',D.k,{{yAxisID:'y2',borderDash:[4,2]}})
]}},options:lineOpts('AMM State','Reserves',{{y2:{{position:'right',grid:{{drawOnChartArea:false}},title:{{display:true,text:'k (ZEC*ZAI)'}}}}}})
}});

// 5. Controller Response
new Chart(document.getElementById('c5'),{{type:'line',data:{{labels:B,datasets:[
 mkDs('Redemption Price','#ea4335',D.redp),
 mkDs('Redemption Rate','#9c27b0',D.redr,{{yAxisID:'y2'}})
]}},options:lineOpts('Controller Response','Price',{{y2:{{position:'right',grid:{{drawOnChartArea:false}},title:{{display:true,text:'Rate (per block)'}}}}}})
}});

// 6. Agent Activity
new Chart(document.getElementById('c6'),{{type:'line',data:{{labels:B,datasets:[
 mkDs('Arber ZAI Balance','#4285f4',D.arb),
 mkDs('Total Collateral','#34a853',D.coll,{{yAxisID:'y2'}}),
 mkDs('LP Shares','#ff9800',D.lp,{{yAxisID:'y2',borderDash:[4,2]}})
]}},options:lineOpts('Agent Activity','ZAI Balance',{{y2:{{position:'right',grid:{{drawOnChartArea:false}},title:{{display:true,text:'Collateral / LP'}}}}}})
}});

// 7. AMM vs External Price Gap
(()=>{{
 const gap=D.ext.map((e,i)=>Math.abs(D.spot[i]-e)/(e||1)*100);
 new Chart(document.getElementById('c7'),{{type:'line',data:{{labels:B,datasets:[
  mkDs('External','#4285f4',D.ext),
  mkDs('AMM Spot','#ea8c00',D.spot),
  mkDs('Gap %','#e91e63',gap,{{yAxisID:'y2',fill:true,backgroundColor:'#e91e6333'}})
 ]}},options:lineOpts('AMM vs External Price','Price',{{y2:{{position:'right',grid:{{drawOnChartArea:false}},title:{{display:true,text:'Gap %'}}}}}})}});
}})();

// 8. Zombie Vault CR Gap
new Chart(document.getElementById('c8'),{{type:'line',data:{{labels:B,datasets:[
 mkDs('CR (TWAP-based)','#4285f4',D.cr),
 mkDs('CR (External-based)','#ea4335',D.crext),
 mkDs('Zombie Count','#e91e63',D.zombies,{{yAxisID:'y2',type:'bar',backgroundColor:'#e91e6344'}})
]}},options:lineOpts('Zombie Vault CR Gap','Collateral Ratio',{{y2:{{position:'right',grid:{{drawOnChartArea:false}},title:{{display:true,text:'Zombie Count'}}}}}})
}});

// 9. Arber Capital
(()=>{{
 const arbTotal=D.arbzec.map((z,i)=>z*D.spot[i]+D.arb[i]);
 new Chart(document.getElementById('c9'),{{type:'line',data:{{labels:B,datasets:[
  mkDs('Arber ZAI','#4285f4',D.arb,{{fill:true,backgroundColor:'#4285f433'}}),
  mkDs('Arber ZEC (spot value)','#34a853',D.arbzec.map((z,i)=>z*D.spot[i]),{{fill:true,backgroundColor:'#34a85333'}}),
  mkDs('Total Capital','#ea4335',arbTotal,{{borderDash:[6,3]}})
 ]}},options:lineOpts('Arber Capital','ZAI Value')}});
}})();

// 10. LP Economics
(()=>{{
 const netPnl=D.fees.map((f,i)=>f+D.il[i]);
 new Chart(document.getElementById('c10'),{{type:'line',data:{{labels:B,datasets:[
  mkDs('Cumulative Fees (ZAI)','#34a853',D.fees),
  mkDs('Impermanent Loss %','#ea4335',D.il),
  mkDs('Net (Fees + IL)','#9c27b0',netPnl,{{borderDash:[6,3]}})
 ]}},options:lineOpts('LP Economics','ZAI / %')}});
}})();

// Config and summary data for downloads
const CONFIG_JSON={js_config_json};
const SUMMARY_JSON={js_summary_json};

function downloadBlob(data,filename,mime){{
 const blob=new Blob([data],{{type:mime}});
 const url=URL.createObjectURL(blob);
 const a=document.createElement('a');
 a.href=url;a.download=filename;a.click();
 URL.revokeObjectURL(url);
}}

function downloadCSV(){{
 const headers=['block','external_price','amm_spot_price','twap_price','redemption_price','redemption_rate','total_debt','reserve_zec','reserve_zai','liquidations','bad_debt','total_collateral','collateral_ratio','k','lp_shares','arber_zai','arber_zec','cumulative_fees','il_pct','cr_ext','zombies'];
 let csv=headers.join(',')+'\n';
 for(let i=0;i<B.length;i++){{
  csv+=[B[i],D.ext[i],D.spot[i],D.twap[i],D.redp[i],D.redr[i],D.debt[i],D.rzec[i],D.rzai[i],D.liqs[i],D.bd[i],D.coll[i],D.cr[i],D.k[i],D.lp[i],D.arb[i],D.arbzec[i],D.fees[i],D.il[i],D.crext[i],D.zombies[i]].join(',')+'\n';
 }}
 downloadBlob(csv,'{scenario_name}.csv','text/csv');
}}

function downloadConfig(){{
 downloadBlob(JSON.stringify(CONFIG_JSON,null,2),'{scenario_name}_config.json','application/json');
}}

function downloadSummary(){{
 downloadBlob(JSON.stringify(SUMMARY_JSON,null,2),'{scenario_name}_summary.json','application/json');
}}
</script>
</body>
</html>"#,
        scenario_name = scenario_name,
        verdict_class = verdict.overall.css_class(),
        verdict_label = verdict.overall.label(),
        total_blocks = summary.total_blocks,
        mean_dev = summary.mean_peg_deviation * 100.0,
        max_dev = summary.max_peg_deviation * 100.0,
        total_liqs = summary.total_liquidations,
        bad_debt_total = summary.total_bad_debt,
        breaker_triggers = summary.breaker_triggers,
        halt_blocks = summary.halt_blocks,
        final_price = summary.final_amm_price,
        amm_zec = config.amm_initial_zec,
        amm_zai = config.amm_initial_zai,
        swap_fee = config.amm_swap_fee,
        min_ratio = config.cdp_config.min_ratio,
        liq_penalty = config.cdp_config.liquidation_penalty,
        stab_fee = config.cdp_config.stability_fee_rate,
        debt_floor = config.cdp_config.debt_floor,
        twap_thresh = config.twap_breaker_config.max_twap_change_pct * 100.0,
        cascade_max = config.cascade_breaker_config.max_liquidations_in_window,
        debt_ceil = config.debt_ceiling_config.initial_ceiling,
        target_price = target_price,
        criteria_rows = criteria_html(&verdict),
        js_blocks = js_array_u64(&blocks),
        js_ext = js_array_f64(&ext_prices),
        js_spot = js_array_f64(&spot_prices),
        js_twap = js_array_f64(&twap_prices),
        js_redp = js_array_f64(&redemption_prices),
        js_redr = js_array_f64(&redemption_rates),
        js_debt = js_array_f64(&total_debt),
        js_rzec = js_array_f64(&reserve_zec),
        js_rzai = js_array_f64(&reserve_zai),
        js_liqs = js_array_u32(&liq_counts),
        js_bd = js_array_f64(&bad_debt),
        js_coll = js_array_f64(&total_collateral),
        js_cr = js_array_f64(&coll_ratio),
        js_k = js_array_f64(&amm_k),
        js_lp = js_array_f64(&total_lp),
        js_arb = js_array_f64(&arber_zai),
        js_arb_zec = js_array_f64(&arber_zec),
        js_fees = js_array_f64(&cum_fees),
        js_il = js_array_f64(&cum_il),
        js_cr_ext = js_array_f64(&cr_ext),
        js_zombies = js_array_u32(&zombie_counts),
        js_config_json = config_to_json(config),
        js_summary_json = summary_to_json(&summary),
    )
}

fn criteria_html(result: &PassFailResult) -> String {
    let mut rows = String::new();
    for c in &result.criteria {
        let icon_class = if c.passed { "crit-pass" } else if c.severity == Verdict::HardFail { "crit-fail" } else { "crit-warn" };
        let icon = if c.passed { "PASS" } else { "FAIL" };
        rows.push_str(&format!(
            "<tr class=\"criterion-row\"><td>{}</td><td class=\"{}\">{}</td><td>{}</td><td>{}</td></tr>\n",
            c.name, icon_class, icon, c.severity.label(), c.details
        ));
    }
    rows
}

fn config_to_json(config: &ScenarioConfig) -> String {
    format!(
        r#"{{"amm_initial_zec":{:.1},"amm_initial_zai":{:.1},"swap_fee":{:.4},"min_ratio":{:.2},"liquidation_penalty":{:.2},"stability_fee_rate":{:.4},"debt_floor":{:.0},"twap_window":{},"initial_redemption_price":{:.2},"stochastic":{},"noise_sigma":{:.4}}}"#,
        config.amm_initial_zec,
        config.amm_initial_zai,
        config.amm_swap_fee,
        config.cdp_config.min_ratio,
        config.cdp_config.liquidation_penalty,
        config.cdp_config.stability_fee_rate,
        config.cdp_config.debt_floor,
        config.cdp_config.twap_window,
        config.initial_redemption_price,
        config.stochastic,
        config.noise_sigma,
    )
}

fn summary_to_json(summary: &crate::output::SummaryMetrics) -> String {
    format!(
        r#"{{"total_blocks":{},"mean_peg_deviation":{:.6},"max_peg_deviation":{:.6},"final_peg_deviation":{:.6},"total_liquidations":{},"total_bad_debt":{:.2},"breaker_triggers":{},"halt_blocks":{},"final_amm_price":{:.4},"final_redemption_price":{:.6}}}"#,
        summary.total_blocks,
        summary.mean_peg_deviation,
        summary.max_peg_deviation,
        summary.final_peg_deviation,
        summary.total_liquidations,
        summary.total_bad_debt,
        summary.breaker_triggers,
        summary.halt_blocks,
        summary.final_amm_price,
        summary.final_redemption_price,
    )
}

// ═══════════════════════════════════════════════════════════════════════
// Master summary (sweep / multi-scenario)
// ═══════════════════════════════════════════════════════════════════════

pub fn generate_master_summary(
    entries: &[(String, PassFailResult, SummaryMetrics)],
) -> String {
    let pass_count = entries
        .iter()
        .filter(|(_, r, _)| r.overall == Verdict::Pass)
        .count();
    let total = entries.len();

    let mut rows = String::new();
    for (name, result, summary) in entries {
        rows.push_str(&format!(
            "<tr>\
             <td><a href=\"{name}.html\">{name}</a></td>\
             <td><span class=\"badge {cls}\">{label}</span></td>\
             <td>{dev:.2}%</td>\
             <td>{bd:.2}</td>\
             <td>{liqs}</td>\
             <td>{halts}</td>\
             <td>{price:.2}</td>\
             </tr>\n",
            name = name,
            cls = result.overall.css_class(),
            label = result.overall.label(),
            dev = summary.mean_peg_deviation * 100.0,
            bd = summary.total_bad_debt,
            liqs = summary.total_liquidations,
            halts = summary.halt_blocks,
            price = summary.final_amm_price,
        ));
    }

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<title>ZAI Simulation — Master Summary</title>
<style>
*{{margin:0;padding:0;box-sizing:border-box}}
body{{font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif;background:#f5f5f5;color:#333}}
header{{background:#1a1a2e;color:#fff;padding:24px 32px}}
header h1{{font-size:1.4em;font-weight:500}}
.summary-line{{margin-top:8px;font-size:1em;opacity:0.9}}
main{{max-width:1200px;margin:0 auto;padding:24px}}
section{{background:#fff;border-radius:8px;box-shadow:0 1px 3px rgba(0,0,0,0.1);padding:24px;margin-bottom:20px}}
table{{width:100%;border-collapse:collapse;font-size:0.9em}}
th,td{{padding:10px 14px;text-align:left;border-bottom:1px solid #e0e0e0}}
th{{background:#f8f9fa;font-weight:600}}
a{{color:#4285f4;text-decoration:none}}
a:hover{{text-decoration:underline}}
.badge{{padding:3px 10px;border-radius:3px;font-weight:700;font-size:0.8em}}
.badge.pass{{background:#34a853;color:#fff}}
.badge.soft-fail{{background:#ea8c00;color:#fff}}
.badge.hard-fail{{background:#ea4335;color:#fff}}
footer{{text-align:center;padding:16px;color:#999;font-size:0.8em}}
</style>
</head>
<body>
<header>
 <h1>ZAI Simulation — Master Summary</h1>
 <div class="summary-line">{pass_count} / {total} scenarios passed</div>
</header>
<main>
<section>
<table>
<tr>
 <th>Scenario</th><th>Verdict</th><th>Mean Peg Dev</th>
 <th>Bad Debt</th><th>Liquidations</th><th>Halt Blocks</th><th>Final Price</th>
</tr>
{rows}
</table>
</section>
<section>
<h3>Data Export</h3>
<button onclick="downloadAll()" style="padding:8px 20px;background:#4285f4;color:#fff;border:none;border-radius:4px;cursor:pointer;font-size:0.9em">Download All (CSV)</button>
</section>
<script>
const SCENARIOS={js_all_scenarios};
function downloadAll(){{
 let csv='scenario,verdict,mean_peg_deviation,max_peg_deviation,total_bad_debt,total_liquidations,halt_blocks,final_amm_price\n';
 for(const s of SCENARIOS){{
  csv+=s.name+','+s.verdict+','+s.mean_dev+','+s.max_dev+','+s.bad_debt+','+s.liqs+','+s.halts+','+s.price+'\n';
 }}
 const blob=new Blob([csv],{{type:'text/csv'}});
 const url=URL.createObjectURL(blob);
 const a=document.createElement('a');
 a.href=url;a.download='zai_all_scenarios.csv';a.click();
 URL.revokeObjectURL(url);
}}
</script>
</main>
<footer>Generated by zai-sim</footer>
</body>
</html>"#,
        pass_count = pass_count,
        total = total,
        rows = rows,
        js_all_scenarios = scenarios_to_js(entries),
    )
}

fn scenarios_to_js(entries: &[(String, PassFailResult, SummaryMetrics)]) -> String {
    let items: Vec<String> = entries
        .iter()
        .map(|(name, result, summary)| {
            format!(
                r#"{{name:"{}",verdict:"{}",mean_dev:{:.6},max_dev:{:.6},bad_debt:{:.2},liqs:{},halts:{},price:{:.4}}}"#,
                name,
                result.overall.label(),
                summary.mean_peg_deviation * 100.0,
                summary.max_peg_deviation * 100.0,
                summary.total_bad_debt,
                summary.total_liquidations,
                summary.halt_blocks,
                summary.final_amm_price,
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}

// ═══════════════════════════════════════════════════════════════════════
// File I/O
// ═══════════════════════════════════════════════════════════════════════

pub fn save_report(html: &str, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, html)?;
    Ok(())
}
