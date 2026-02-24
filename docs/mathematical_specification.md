# ZAI Mathematical Specification

*A first-principles derivation of the oracle-free CDP flatcoin mechanics.*

This document provides the complete mathematical foundation for the ZAI protocol — an oracle-free collateralized debt position (CDP) system built on a constant-product AMM. Every formula is derived from the exact implementation in `zai-sim/src/` and cross-referenced to specific source lines. Simulation findings (F-xxx) provide empirical validation.

---

## Table of Contents

1. [Notation and Conventions](#1-notation-and-conventions)
2. [AMM Core: Constant-Product Invariant](#2-amm-core-constant-product-invariant)
3. [TWAP Oracle](#3-twap-oracle)
4. [CDP Mechanics](#4-cdp-mechanics)
5. [Liquidation Engine](#5-liquidation-engine)
6. [Redemption Rate Controller](#6-redemption-rate-controller)
7. [System Stability Analysis](#7-system-stability-analysis)
8. [Numerical Examples from Simulation](#8-numerical-examples-from-simulation)

---

## 1. Notation and Conventions

| Symbol | Meaning | Unit |
|--------|---------|------|
| x | AMM reserve of ZEC | ZEC |
| y | AMM reserve of ZAI | ZAI |
| k | Constant-product invariant (x · y) | ZEC·ZAI |
| p | Spot price (ZAI per ZEC) | ZAI/ZEC |
| f | Swap fee fraction | dimensionless |
| Δx, Δy | Swap input amounts | ZEC or ZAI |
| C(n) | Cumulative price accumulator at block n | ZAI·blocks/ZEC |
| T(n,w) | TWAP over window w ending at block n | ZAI/ZEC |
| c | Vault collateral | ZEC |
| d | Vault debt | ZAI |
| R | Collateral ratio | dimensionless |
| R_min | Minimum collateral ratio (default 1.5) | dimensionless |
| λ | Liquidation penalty fraction (default 0.13) | dimensionless |
| σ | Stability fee rate (annual, default 0.02) | per year |
| N | Blocks per year (≈ 420,768 at 75s/block) | blocks |
| r_p | Redemption price | ZAI/ZEC |
| r_r | Redemption rate (per-block) | per block |
| p_m | Market price from AMM | ZAI/ZEC |

**Block time**: 75 seconds (Zcash). All discrete-time formulas use block count as the time unit.

**Source files**: `amm.rs` (AMM), `cdp.rs` (CDP), `liquidation.rs` (liquidation), `controller.rs` (controller).

---

## 2. AMM Core: Constant-Product Invariant

### 2.1 The Invariant

The AMM holds reserves of ZEC (x) and ZAI (y). The fundamental invariant is:

```
k = x · y
```

**Source**: `amm.rs:29` — `let k = initial_zec * initial_zai`

This is the standard Uniswap v2 constant-product formula. After initialization, k is *recomputed* (not conserved) after every state change, allowing fees to monotonically increase k.

### 2.2 Spot Price

The marginal price of ZEC denominated in ZAI is the ratio of reserves:

```
p = y / x
```

**Source**: `amm.rs:57` — `self.reserve_zai / self.reserve_zec`

**Derivation**: In a constant-product AMM, the marginal exchange rate equals the reserve ratio. This follows from the implicit function theorem: if k = x·y is constant, then dy/dx = -y/x, so the price of one unit of x in terms of y is |dy/dx| = y/x.

### 2.3 Swap: ZEC → ZAI

A trader sends Δx ZEC to the AMM and receives Δy_out ZAI:

```
Δx_eff = Δx · (1 - f)                    fee-adjusted input
x_new  = x + Δx_eff                       new ZEC reserve (for pricing)
y_new  = k / x_new                         constant-product output
Δy_out = y - y_new                         ZAI output to trader
```

**Source**: `amm.rs:114-117`

After the swap, reserves update asymmetrically — the *full* input (including fee) enters reserves:

```
x ← x + Δx           (full input, not just effective)
y ← y - Δy_out
k ← x · y            (k grows because Δx > Δx_eff)
```

**Source**: `amm.rs:124-127`

**Key insight**: The fee is not extracted; it remains in the pool as additional ZEC. Since k is recomputed as x·y after the full Δx is added, k strictly increases with every swap. This fee accrual benefits LPs.

**Closed-form output**:

```
Δy_out = y - k/(x + Δx·(1-f))
       = y · [1 - x/(x + Δx·(1-f))]
       = y · Δx·(1-f) / (x + Δx·(1-f))
```

### 2.4 Post-Swap Spot Price

After a ZEC → ZAI swap of size Δx:

```
p' = y_new / x_new_actual
   = (y - Δy_out) / (x + Δx)
```

Where x_new_actual = x + Δx (full input) and y - Δy_out = k/(x + Δx·(1-f)).

For small fees (f → 0):

```
p' ≈ k / (x + Δx)²  =  y·x / (x + Δx)²
```

The price impact of a swap of size Δx relative to reserves x is approximately:

```
price_impact ≈ 2·Δx/x     (for Δx << x)
```

This quadratic relationship between swap size and price impact is the AMM's core defense against manipulation.

### 2.5 Swap: ZAI → ZEC (Mirror)

```
Δy_eff = Δy · (1 - f)
y_new  = y + Δy_eff
x_new  = k / y_new
Δx_out = x - x_new
```

**Source**: `amm.rs:143-146`

### 2.6 LP Shares

**Initial mint** (geometric mean):

```
S_0 = √(x_0 · y_0)
```

**Source**: `amm.rs:31`

**Subsequent deposits**:

```
S_new = min(Δx/x · S_total, Δy/y · S_total)
```

**Source**: `amm.rs:168-170`

**Withdrawal** (pro-rata):

```
fraction  = S_burn / S_total
Δx_out    = x · fraction
Δy_out    = y · fraction
```

**Source**: `amm.rs:196-198`

### 2.7 Impermanent Loss

For a price ratio r = p_current / p_entry:

```
IL = 2√r / (1 + r) - 1
```

**Source**: `amm.rs:217-218`

This is always ≤ 0. At r = 1 (no price change), IL = 0. At r = 4 (2× price), IL ≈ -5.7%.

---

## 3. TWAP Oracle

### 3.1 Price Accumulator

The AMM maintains a cumulative price accumulator. Each time the AMM state is observed at block n:

```
C(n) = C(n-1) + p(n) · Δb(n)
```

where Δb(n) = blocks elapsed since last observation and p(n) = spot price at block n.

**Source**: `amm.rs:66` — `self.cumulative_price += spot * blocks_elapsed as f64`

This is the standard Uniswap v2 TWAP accumulator design.

### 3.2 TWAP Computation

The time-weighted average price over a window of w blocks ending at block n:

```
T(n, w) = [C(n) - C(n - w)] / [block(n) - block(n - w)]
```

**Source**: `amm.rs:92-99`

If block_diff = 0 (degenerate case), falls back to current spot price (`amm.rs:96`).

### 3.3 TWAP as Manipulation Defense

The TWAP window (default: 48 blocks ≈ 1 hour) determines the cost of price manipulation. To move the TWAP by a fraction δ from its current value, an attacker must sustain the manipulated spot price for a significant fraction of the window.

**Lower bound on manipulation cost**: To shift TWAP by δ over w blocks, the attacker must hold the spot price at p·(1+δ) for approximately w blocks. Each block the position must be maintained costs the attacker impermanent loss and arbitrage extraction.

The quadratic price impact of the AMM means that moving the spot price by fraction δ requires capital proportional to:

```
capital_required ∝ x · δ     (in ZEC terms)
```

And the cost (from price impact and arb extraction) scales as:

```
manipulation_cost ∝ x · δ² · w
```

This makes TWAP manipulation quadratically expensive in the desired deviation and linearly expensive in the window length.

---

## 4. CDP Mechanics

### 4.1 Collateral Ratio

A vault with c ZEC collateral and d ZAI debt has collateral ratio:

```
R = (c · p_TWAP) / d
```

**Source**: `cdp.rs:50` — `(self.collateral_zec * zec_price) / self.debt_zai`

The price used is the TWAP (not spot), sourced via `amm.get_twap(self.config.twap_window)` at `cdp.rs:74`.

### 4.2 Vault Safety Invariant

All vault operations (open, withdraw, borrow) enforce:

```
R ≥ R_min     where R_min = 1.5 (default, 150%)
```

**Source**: `cdp.rs:130` (open), `cdp.rs:227` (withdraw), `cdp.rs:272` (borrow)

**Debt floor**: Additionally, vault debt must be either exactly zero or at least the debt floor:

```
d = 0  ∨  d ≥ d_floor     where d_floor = 100 ZAI (default)
```

**Source**: `cdp.rs:119, 262, 313`

### 4.3 Stability Fee (Debt Compounding)

Debt accrues interest via discrete per-block compounding:

```
r_block = σ / N                           per-block rate
m       = (1 + r_block)^Δb               multiplier over Δb blocks
d_new   = d_old · m                       compounded debt
```

**Source**: `cdp.rs:90-94`

where σ = annual stability fee rate (default 0.02 = 2%) and N = BLOCKS_PER_YEAR ≈ 420,768.

**Effective annual rate** (due to discrete compounding):

```
EAR = (1 + σ/N)^N - 1 ≈ σ + σ²/(2N) ≈ σ
```

For σ = 0.02, the EAR ≈ 2.0002% — essentially equal to the stated rate due to the high block count.

### 4.4 Liquidation Trigger

A vault is liquidatable when:

```
R < R_min  ⟺  c · p_TWAP < d · R_min
```

**Source**: `cdp.rs:349` — `vault.collateral_ratio(price) < self.config.min_ratio`

**Critical property**: Because price is sourced from TWAP (not an external oracle), a crash in external ZEC price does *not* immediately trigger liquidations. The TWAP must first decline, which requires the AMM spot price to decline, which requires arbers to sell ZEC into the AMM. This multi-step dependency is the core mechanism that prevents liquidation death spirals.

---

## 5. Liquidation Engine

### 5.1 Full Liquidation

When a vault with collateral c and debt d is liquidated:

**Step 1 — Collateral seizure and AMM sale**:

```
Δy_amm = AMM.swap_zec_for_zai(c)         all collateral sold atomically
```

**Source**: `liquidation.rs:172-175`

**Step 2 — Obligation computation**:

```
penalty    = d · λ                        penalty amount
obligation = d + penalty = d · (1 + λ)    total required to cover
```

**Source**: `liquidation.rs:178-181`

**Step 3 — Settlement** (three cases):

| Condition | Bad Debt | Surplus to Owner | Actual Penalty |
|-----------|----------|------------------|----------------|
| Δy_amm ≥ obligation | 0 | Δy_amm − obligation | d · λ |
| d ≤ Δy_amm < obligation | 0 | 0 | Δy_amm − d |
| Δy_amm < d | d − Δy_amm | 0 | 0 |

**Source**: `liquidation.rs:183-193`

**Bad debt** occurs in the third case: when AMM proceeds from selling all collateral cannot cover the vault's debt. This happens when the AMM price has dropped sufficiently (relative to where the debt was originated) that the collateral's AMM-denominated value is underwater.

### 5.2 Bad Debt Condition (Derivation)

Bad debt occurs when Δy_amm < d. Using the constant-product swap formula:

```
Δy_amm = y · c·(1-f) / (x + c·(1-f))
```

Bad debt occurs when:

```
y · c·(1-f) / (x + c·(1-f)) < d
```

Since p = y/x, this becomes:

```
p · x · c·(1-f) / (x + c·(1-f)) < d
```

For a vault opened at price p₀ with R = R_min (worst case), we have d = c·p₀/R_min. Substituting:

```
p · x · c·(1-f) / (x + c·(1-f)) < c·p₀/R_min
```

Simplifying (divide by c):

```
p · x·(1-f) / (x + c·(1-f)) < p₀/R_min
```

The left side represents the *effective price* received when dumping c ZEC into a pool with x ZEC reserves. This is always less than p due to price impact. With large c relative to x, the effective price drops dramatically.

### 5.3 Penalty Distribution

The collected penalty is distributed across three recipients:

```
keeper_reward    = actual_penalty · κ           keeper share
lp_share         = (actual_penalty − keeper_reward) · α    LP injection
protocol_share   = actual_penalty − keeper_reward − lp_share
```

**Source**: `liquidation.rs:196-209`

where κ = keeper_reward_pct (default 0.50 for challenge-response mode) and α = liquidation_penalty_to_lps_pct (default 0.0).

When lp_share > 0, it is injected directly into AMM reserves:

```
y ← y + lp_share
k ← x · y                  (k increases, benefiting LPs)
```

**Source**: `liquidation.rs:202-204`

### 5.4 Self-Liquidation Discount

Vault owners can self-liquidate at a reduced penalty:

```
λ_self = λ · α_self
```

**Source**: `liquidation.rs:271-272`

where α_self = self_liquidation_penalty_pct (default 0.0, meaning zero penalty for self-liquidation).

### 5.5 Graduated (Partial) Liquidation

Vaults in the "warning zone" (below R_min but above a floor) are partially liquidated:

**Eligibility**:

```
R_floor ≤ R(TWAP) < R_min
```

**Source**: `liquidation.rs:481-482`

**Per-block seizure**:

```
c_seized = c · γ           where γ = graduated_pct_per_block (default 0.10)
```

**Source**: `liquidation.rs:516`

**AMM sale and back-solve**:

```
Δy_amm     = AMM.swap_zec_for_zai(c_seized)
d_covered  = Δy_amm / (1 + λ)              debt portion
penalty    = Δy_amm − d_covered             penalty portion
```

**Source**: `liquidation.rs:525-527`

Note the inversion: in graduated liquidation, the proceeds are *back-solved* from the AMM output, rather than computing penalty on top of known debt. This ensures the penalty is always physically backed by AMM proceeds.

### 5.6 Zombie Vault Detection

A "zombie" vault appears safe by TWAP but is underwater at spot price:

```
zombie ⟺ R(TWAP) ≥ R_min  ∧  R(spot) < R_min  ∧  [R(TWAP) − R(spot)] > θ_gap
```

**Source**: `liquidation.rs:393-399`

where θ_gap is a configurable gap threshold. Zombie vaults are the characteristic failure mode of TWAP-based pricing — the TWAP lags behind spot during crashes, hiding true insolvency.

---

## 6. Redemption Rate Controller

The controller adjusts the *redemption price* r_p — the protocol's target price for ZAI — based on the market price p_m observed in the AMM.

### 6.1 Redemption Price Dynamics

Each block, the redemption price compounds by the current rate:

```
r_p(n+1) = r_p(n) · (1 + r_r)^Δb
```

**Source**: `controller.rs:99`

where r_r is the per-block redemption rate and Δb is blocks elapsed.

### 6.2 PI Controller Mode

**Error signal** (fractional deviation):

```
e = (p_m − r_p) / r_p
```

**Source**: `controller.rs:122`

**Proportional term** (negative feedback):

```
P = −k_p · e
```

**Source**: `controller.rs:125`

**Integral accumulation** (negative feedback with anti-windup):

```
I ← clamp(I − k_i · e, I_min, I_max)
```

**Source**: `controller.rs:128-133`

**Output** (clamped rate):

```
r_r = clamp(P + I, r_min, r_max)
```

**Source**: `controller.rs:136-137`

**Default parameters**:

| Parameter | Value | Meaning |
|-----------|-------|---------|
| k_p | 2 × 10⁻⁷ | Proportional gain |
| k_i | 5 × 10⁻⁹ | Integral gain |
| I_min, I_max | ±10⁻⁴ | Anti-windup bounds |
| r_min, r_max | ±10⁻⁴ per block | Rate bounds (≈ ±4.2%/year) |

**Source**: `controller.rs:40-51`

### 6.3 Tick Controller Mode (Rico-Style)

An alternative log-scale integral-only controller:

**Log-space error**:

```
e_log = ln(p_m / r_p)
```

**Source**: `controller.rs:147`

**Integral accumulation** (integral IS the rate):

```
I ← clamp(I − s · e_log, r_min, r_max)
r_r = I
```

**Source**: `controller.rs:150-155`

where s = sensitivity (default 10⁻⁷).

**Key difference from PI**: No proportional term. The integral accumulates slowly, providing smooth rate adjustments. The log-space error provides symmetric response to price ratios (a 2× overvaluation has the same magnitude error as a 0.5× undervaluation).

### 6.4 Stability Analysis of the Controller

**Negative feedback**: All feedback terms carry a negative sign (−k_p, −k_i, −s). When p_m > r_p (market overvalues ZAI relative to target):
- e > 0, so P < 0 and ΔI < 0
- r_r decreases, slowing or reversing redemption price growth
- This makes ZAI "cheaper" to mint (lower target), reducing demand pressure

When p_m < r_p (market undervalues ZAI):
- e < 0, so P > 0 and ΔI > 0
- r_r increases, accelerating redemption price growth
- This makes ZAI "more expensive" to mint, increasing incentive to buy

**Bounded rate**: The clamp to [r_min, r_max] = [−10⁻⁴, 10⁻⁴] per block prevents the controller from over-correcting. Annualized, this is approximately ±4.2%/year:

```
max_annual_rate = (1 + 10⁻⁴)^420768 − 1 ≈ 4.2%
```

---

## 7. System Stability Analysis

### 7.1 The Core Tradeoff: AMM Inertia vs. Price Accuracy

ZAI's oracle-free design exhibits a fundamental tradeoff discovered through simulation (F-028):

**AMM price inertia** — the AMM's reluctance to reprice during crashes — is simultaneously:
1. **The primary peg defense**: prevents liquidation death spirals
2. **The source of zombie vaults**: hides true insolvency behind lagging TWAP

**Why inertia prevents death spirals**:

In oracle-based CDPs (e.g., MakerDAO), a crash triggers this cascade:

```
price_crash → liquidation_trigger → collateral_dump → price_further_crash → more_liquidations
```

In ZAI, the AMM inserts friction at every step:

```
external_crash → arbers_sell_ZEC_into_AMM → AMM_absorbs_with_price_impact
→ TWAP_lags_spot → liquidation_delayed → no_collateral_dump_cascade
```

The AMM's quadratic price impact (Section 2.4) means that each arber trade has diminishing effect. When arber capital is exhausted, the AMM price stops moving regardless of external conditions.

### 7.2 Arber Exhaustion as Defense Mechanism

**Finding F-028**: Replenishing arber capital *worsens* peg stability during sustained crashes.

With replenishment rate R_arb (ZAI/block), the arber continuously sells ZEC into the AMM:

```
More arber capital → AMM price tracks external → TWAP drops → liquidations trigger
→ collateral dumped into AMM → AMM price drops further → more liquidations
```

Without replenishment:

```
Arber exhausts capital → AMM price freezes → TWAP plateaus → no new liquidations
→ system survives the crash
```

The simulation data confirms this monotonically:

| Arber Replenishment | Mean Peg Deviation |
|--------------------:|-------------------:|
| 0 ZAI/block | 11.79% |
| 5 ZAI/block | 16.25% |
| 50 ZAI/block | 31.09% |
| 1000 ZAI/block | 34.75% |

### 7.3 Quadratic Resistance to Manipulation

The constant-product AMM provides quadratic resistance to price manipulation. For an attacker selling Δx ZEC into a pool with reserves (x, y):

**Price displacement**:

```
δp/p = 1 − x²/(x + Δx)² ≈ 2Δx/x     for Δx << x
```

**Attacker's cost** (from buying back at inflated price):

```
cost ≈ Δx · p · (1 − x/(x + Δx))  =  Δx · p · Δx/(x + Δx)
     ≈ p · (Δx)²/x                     for Δx << x
```

This cost scales quadratically with displacement. To move the price by 20% in a $5M pool (x = 100K ZEC at $50):

```
Δx ≈ x · δp/(2p) = 100K · 0.20/2 = 10K ZEC ($500K)
cost ≈ 50 · (10K)²/100K = $50K
```

And the attacker must sustain this over the TWAP window to affect liquidations.

### 7.4 Griefing Cost Analysis

From F-043, the griefing ratio (attacker cost / system damage) quantifies economic security:

```
griefing_ratio = |attacker_loss| / bad_debt_generated
```

At the baseline configuration ($5M AMM, 200% CR, 240-block TWAP):

```
griefing_ratio = $16,899 / $3,145 ≈ 5.4:1
```

The attacker loses $5.40 for every $1 of bad debt created. This makes pure griefing prohibitively expensive for rational adversaries.

### 7.5 AMM Depth as the Primary Safety Parameter

From F-039 (collateral ratio sensitivity) and F-046 (griefing mitigation):

- **Min collateral ratio** (R_min): Zero bad debt from 125% to 300% at $5M AMM depth. CR is a capital efficiency parameter, not a safety parameter.
- **AMM depth**: $10M pool eliminates bad debt entirely even under sustained manipulation (F-046). At $10M, the griefing ratio becomes ∞ (zero damage).

The scaling relationship:

```
mean_peg_deviation ~ arber_capital / pool_size
```

A larger pool absorbs more selling pressure before moving, increasing both manipulation cost and arber exhaustion time.

### 7.6 Bad Debt Existence Theorem (Informal)

**Claim**: Bad debt can occur if and only if a vault's collateral, when sold atomically through the AMM, yields less ZAI than the vault's debt.

**Condition**:

```
bad_debt > 0  ⟺  y · c(1-f) / (x + c(1-f)) < d
```

**When this happens**:
1. The vault's collateral c is large relative to pool reserves x (large price impact)
2. The TWAP-reported price was much higher than the effective execution price
3. Specifically: the vault was opened or borrowed at a price the AMM can no longer deliver at execution time

**When this cannot happen**:
- If c << x (small vault relative to pool), price impact is negligible and the effective price ≈ spot price. As long as R ≥ R_min at the spot price, the vault is solvent.
- With deeper pools, even large vaults have acceptable price impact.

---

## 8. Numerical Examples from Simulation

### 8.1 Steady-State Swap

**Setup**: AMM with x = 100,000 ZEC, y = 5,000,000 ZAI, f = 0.3%.

Swap 100 ZEC for ZAI:

```
p_spot     = 5,000,000 / 100,000                    = 50.00 ZAI/ZEC
Δx_eff     = 100 · (1 − 0.003)                      = 99.70 ZEC
x_new      = 100,000 + 99.70                         = 100,099.70 ZEC
y_new      = (100,000 · 5,000,000) / 100,099.70      = 4,995,017.50 ZAI
Δy_out     = 5,000,000 − 4,995,017.50                = 4,982.50 ZAI
effective_price = 4,982.50 / 100                      = 49.825 ZAI/ZEC
slippage   = (50.00 − 49.825) / 50.00                = 0.35%
```

Post-swap spot: y_new / (x + 100) = 4,995,017.50 / 100,100 = 49.90 ZAI/ZEC (0.2% price impact).

### 8.2 Liquidation Example

**Setup**: Vault with c = 200 ZEC, d = 5,000 ZAI (CR = 200% at p = 50). AMM: x = 100,000 ZEC, y = 5,000,000 ZAI. TWAP drops to 37.50 → CR = (200 · 37.50)/5000 = 1.50 — exactly at threshold.

If TWAP drops one tick further to 37.49:

```
R = (200 · 37.49) / 5000 = 1.4996 < 1.5  →  LIQUIDATABLE
```

Sell all 200 ZEC collateral:

```
Δy_amm     = 5,000,000 · 200·0.997 / (100,000 + 200·0.997)
           = 5,000,000 · 199.40 / 100,199.40
           = 9,950.09 ZAI

obligation = 5,000 · (1 + 0.13) = 5,650 ZAI

Since 9,950.09 > 5,650:
  bad_debt = 0
  surplus  = 9,950.09 − 5,650 = 4,300.09 ZAI (returned to owner)
  penalty  = 650 ZAI
```

This vault liquidates cleanly because c (200 ZEC) is small relative to x (100,000 ZEC), so price impact is only ≈0.4%.

### 8.3 Bad Debt Example (Large Vault, Depressed AMM)

**Setup**: Vault with c = 20,000 ZEC, d = 400,000 ZAI (CR = 150% at original price p₀ = 40). AMM has been partially drained: x = 50,000 ZEC, y = 1,000,000 ZAI (p = 20).

Sell all 20,000 ZEC collateral:

```
Δy_amm     = 1,000,000 · 20,000·0.997 / (50,000 + 20,000·0.997)
           = 1,000,000 · 19,940 / 69,940
           = 285,102 ZAI

obligation = 400,000 · 1.13 = 452,000 ZAI

Since 285,102 < 400,000:
  bad_debt = 400,000 − 285,102 = 114,898 ZAI
  surplus  = 0
  penalty  = 0
```

The large vault (20K ZEC into a 50K ZEC pool = 40% of reserves) suffers massive price impact, and the depressed AMM price (p = 20 vs. original p₀ = 40) means the collateral is worth far less than the debt.

### 8.4 Economic Attack Cost (F-043 Validated)

**Scenario**: 100K ZEC whale attacks a $5M AMM ($50/ZEC, x = 100K ZEC). Sustained selling: 1K ZEC/block for 100 blocks, then buyback over 10 blocks.

From F-043 simulation results:

| Attack Strategy | Capital Used | Whale P&L | Bad Debt | Griefing Ratio |
|:---|:---:|:---:|:---:|:---:|
| Sustained dump (1K/blk × 100) | 100K ZEC | −$16,899 | $3,145 | 5.4:1 |
| Black Thursday + dump | 100K ZEC | −$100,764 | $3,067 | 32.8:1 |
| Repeated small dumps | 100K ZEC | −$163,974 | $0 | ∞ |
| Flash dump (all at once) | 100K ZEC | −$17,133 | $0 | ∞ |

**Key result**: Only the sustained dump creates any bad debt, and even then the attacker loses $5.40 for each $1 of damage.

### 8.5 Griefing Mitigation Configurations (F-046)

Defensive configurations tested against the sustained manipulation attack:

| Configuration | Pool Size | Min CR | TWAP | Bad Debt | Grief Ratio |
|:---|:---:|:---:|:---:|:---:|:---:|
| Baseline | $5M | 200% | 240 blk | $3,145 | 5.4:1 |
| Deep pool | $10M | 200% | 240 blk | $0 | ∞ |
| Short TWAP | $5M | 200% | 48 blk | $0 | ∞ |
| High CR | $5M | 300% | 240 blk | $117 | 128:1 |

**Critical insight**: Doubling AMM depth to $10M eliminates bad debt entirely. The whale loses $24,681 with zero system damage. Higher CR (300%) is counterproductive — it makes the attack *profitable* ($14,954 whale profit) because wider CR means more capital available for the attacker to extract.

### 8.6 Controller Dynamics Example

**PI Controller**: Market price p_m = 51 ZAI/ZEC, redemption price r_p = 50.

```
e        = (51 − 50) / 50                = 0.02 (2% overvalued)
P        = −2×10⁻⁷ · 0.02               = −4×10⁻⁹
ΔI       = −5×10⁻⁹ · 0.02               = −1×10⁻¹⁰
r_r      = P + I_old + ΔI
```

If sustained for 1000 blocks:
```
I ≈ −1×10⁻¹⁰ · 1000 = −1×10⁻⁷
r_r ≈ −4×10⁻⁹ + (−1×10⁻⁷) ≈ −1.04×10⁻⁷ per block
```

Annual rate ≈ −1.04×10⁻⁷ × 420,768 ≈ −4.4% — the controller gently pushes the redemption price down to reduce ZAI's overvaluation.

**Tick Controller**: Same scenario.

```
e_log    = ln(51/50)                      = 0.01980
ΔI       = −1×10⁻⁷ · 0.01980             = −1.98×10⁻⁹
r_r      = I_old + ΔI
```

The Tick controller responds more slowly (no proportional kick) but accumulates without the dual-term complexity.

---

## Appendix A: Default Parameter Table

All parameters with their source locations and default values:

| Parameter | Default | Unit | Source |
|-----------|---------|------|--------|
| AMM initial ZEC | 10,000 | ZEC | `scenario.rs:83` |
| AMM initial ZAI | 500,000 | ZAI | `scenario.rs:84` |
| Swap fee (f) | 0.003 (0.3%) | fraction | `scenario.rs:85` |
| Min collateral ratio (R_min) | 1.5 (150%) | ratio | `cdp.rs:25` |
| Liquidation penalty (λ) | 0.13 (13%) | fraction | `cdp.rs:26` |
| Stability fee rate (σ) | 0.02 (2%/yr) | per year | `cdp.rs:27` |
| Debt floor (d_floor) | 100 | ZAI | `cdp.rs:28` |
| TWAP window | 48 blocks (~1hr) | blocks | `cdp.rs:29` |
| Blocks per year (N) | 420,768 | blocks | `cdp.rs:6` |
| Keeper reward (κ) | 0.50 (50%) | fraction | `liquidation.rs:26` |
| Self-liq penalty pct (α_self) | 0.0 | fraction | `liquidation.rs:27` |
| LP penalty share (α) | 0.0 | fraction | `liquidation.rs:28` |
| Graduated seizure (γ) | 0.10 (10%) | per block | `liquidation.rs:30` |
| PI: k_p | 2×10⁻⁷ | per block | `controller.rs:42` |
| PI: k_i | 5×10⁻⁹ | per block | `controller.rs:43` |
| Tick: sensitivity (s) | 10⁻⁷ | per block | `controller.rs:58` |
| Rate bounds (r_min, r_max) | ±10⁻⁴ | per block | `controller.rs:47-48` |
| Integral bounds (I_min, I_max) | ±10⁻⁴ | — | `controller.rs:49-50` |
| Initial redemption price | 50.0 | ZAI/ZEC | `scenario.rs:92` |

## Appendix B: Formula Index

Quick reference mapping every formula to its source:

| # | Formula | Source | Section |
|---|---------|--------|---------|
| 1 | k = x · y | `amm.rs:29` | 2.1 |
| 2 | p = y/x | `amm.rs:57` | 2.2 |
| 3 | Δy = y·Δx(1-f)/(x+Δx(1-f)) | `amm.rs:114-117` | 2.3 |
| 4 | S₀ = √(x·y) | `amm.rs:31` | 2.6 |
| 5 | IL = 2√r/(1+r) − 1 | `amm.rs:217-218` | 2.7 |
| 6 | C(n) += p·Δb | `amm.rs:66` | 3.1 |
| 7 | T = ΔC/Δb | `amm.rs:92-99` | 3.2 |
| 8 | R = c·p/d | `cdp.rs:50` | 4.1 |
| 9 | d_new = d·(1+σ/N)^Δb | `cdp.rs:90-94` | 4.3 |
| 10 | obligation = d·(1+λ) | `liquidation.rs:178-181` | 5.1 |
| 11 | bad_debt = max(0, d − Δy_amm) | `liquidation.rs:192` | 5.1 |
| 12 | r_p ← r_p·(1+r_r)^Δb | `controller.rs:99` | 6.1 |
| 13 | e = (p_m − r_p)/r_p | `controller.rs:122` | 6.2 |
| 14 | r_r = clamp(−k_p·e + I, bounds) | `controller.rs:136-137` | 6.2 |
| 15 | e_log = ln(p_m/r_p) | `controller.rs:147` | 6.3 |
| 16 | zombie: R(TWAP)≥R_min ∧ R(spot)<R_min | `liquidation.rs:393-399` | 5.6 |
| 17 | graduated: d = Δy/(1+λ) | `liquidation.rs:525-527` | 5.5 |

---

*Cross-reference: [RESEARCH_SUMMARY.md](../RESEARCH_SUMMARY.md) for findings overview, [FINDINGS.md](../FINDINGS.md) for all 46 findings, source code in `src/`.*
