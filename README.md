# ZAI Simulator — Oracle-Free CDP Flatcoin for Zcash

A block-level simulator for ZAI, a proposed oracle-free CDP flatcoin on Zcash that uses an on-chain AMM (constant-product, Uniswap V2 style) as its sole price oracle via TWAP. Across 31 findings from 124 tests, the central result is a fundamental tradeoff: the AMM's price inertia provides natural immunity to MakerDAO-style liquidation death spirals, but at the cost of "zombie vaults" — positions that appear solvent to the protocol while being underwater by external market standards. The system is viable at $5M+ AMM liquidity (12/13 scenarios pass) with zero bad debt across all runs.

## Quick Start

```bash
# Run all 124 tests
cargo test

# Generate HTML reports with interactive charts and CSV download
cargo test --test final_reports_test -- --nocapture

# Open the master report index
open reports/final/index.html
```

## Key Numbers

| Metric | Value |
|--------|-------|
| Tests | 124 (0 failures, 0 clippy warnings) |
| Findings | 31 (F-001 through F-031) |
| Stress scenarios | 13 (Black Thursday, sustained bear, flash crash, bank run, demand shock, etc.) |
| Agent types | 7 (arbitrageur, demand, miner, CDP holder, LP, IL-aware LP, attacker) |
| Pass rate at $5M AMM | 12/13 (92%) |
| Bad debt across all runs | $0 |
| Black Thursday peg deviation | 4.2% mean (vs DAI's 12% during March 2020) |

## The Novel Insight

Arber exhaustion during crashes is the stability mechanism, not a failure mode. The system works because it disconnects from external price reality — preventing death spirals at the cost of temporary zombie vaults that self-heal on recovery (F-029).

## Documentation

- **[RESEARCH_SUMMARY.md](RESEARCH_SUMMARY.md)** — Full analysis: methodology, 31 findings, core tradeoff, deployment prerequisites, open questions
- **[FINDINGS.md](FINDINGS.md)** — Complete findings log with data tables and root cause analysis
- **[reports/final/index.html](reports/final/index.html)** — Interactive HTML reports with 10 charts per scenario and CSV/JSON download

## Prerequisites

- Rust toolchain (rustc + cargo)
- No external dependencies — all crates are from crates.io and resolve via `cargo build`

```bash
cargo build
```

## Project Structure

```
src/
  amm.rs          — Constant-product AMM with TWAP accumulator
  agents.rs       — 7 agent types (arbitrageur, demand, miner, CDP, LP, IL-aware LP, attacker)
  scenario.rs     — Simulation engine and BlockMetrics
  scenarios.rs    — 13 stress scenario price generators
  controller.rs   — PI and Tick redemption price controllers
  cdp.rs          — Vault registry and debt management
  liquidation.rs  — Liquidation modes (transparent, cascade, zombie detection)
  circuit_breaker.rs — TWAP deviation, cascade, and dynamic debt ceiling breakers
  report.rs       — HTML report generation (10 charts, download buttons)
  output.rs       — Summary metrics and pass/fail evaluation
tests/
  26 test files covering unit tests, integration tests, parameter sweeps,
  Monte Carlo validation, and scenario-specific analysis
```

## License

MIT
