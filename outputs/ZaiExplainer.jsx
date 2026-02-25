// ======================================================================
// ZaiExplainer.jsx — Animated SVG explainer for the ZAI flatcoin simulator
// Single-file React 18 + motion component. No external dependencies.
// ======================================================================

import React, { useState, useEffect, useCallback, useRef } from "react";
import { createRoot } from "react-dom/client";
import {
  motion,
  useMotionValue,
  useSpring,
  useTransform,
} from "motion/react";

// ======= SECTION: COLOR PALETTE =======

const C = {
  BG: "#0F172A",
  ZEC: "#6B46C1",
  ZAI: "#10B981",
  Healthy: "#22C55E",
  Warning: "#F59E0B",
  Danger: "#EF4444",
  Liquidated: "#374151",
  Whale: "#991B1B",
  Arber: "#06B6D4",
  Text: "#F8FAFC",
  Panel: "#1E293B",
  Muted: "#94A3B8",
};

// ======= SECTION: SPRING CONFIGS =======

const SPRING = {
  fluid: { stiffness: 100, damping: 14, mass: 1.5 },
  prices: { stiffness: 200, damping: 20, mass: 1 },
  twap: { stiffness: 50, damping: 20, mass: 2 },
  arber: { stiffness: 300, damping: 25, mass: 0.8 },
};

// ======= SECTION: HELPERS =======

function lerp(a, b, t) {
  return a + (b - a) * t;
}

function fmt(n, decimals = 2) {
  if (n == null) return "0";
  return Number(n).toLocaleString(undefined, {
    minimumFractionDigits: decimals,
    maximumFractionDigits: decimals,
  });
}

function fmtInt(n) {
  if (n == null) return "0";
  return Math.round(n).toLocaleString();
}

function fmtSci(n) {
  if (n == null) return "0";
  const exp = Math.floor(Math.log10(Math.abs(n)));
  const mantissa = n / Math.pow(10, exp);
  return mantissa.toFixed(2) + "e" + exp;
}

// ======= SECTION: DATA ARRAYS =======

// --- Scene 1: Normal Day (20 points, ~7s) ---
const SCENE_1_DATA = (() => {
  const points = [];
  const K = 5e11;
  for (let i = 0; i < 20; i++) {
    const t = i / 19;
    const block = Math.round(lerp(1, 1000, t));
    const extPrice = 50;
    const spotPrice = lerp(50.0, 49.5, t);
    const twapPrice = lerp(50.0, 49.55, Math.max(0, t - 0.05));
    const reserveZec = Math.round(lerp(100000, 101000, t));
    const reserveZai = Math.round(K / reserveZec);
    let message = "A normal day on the ZAI protocol.";
    let emphasis = false;
    if (t > 0.33 && t <= 0.66) {
      message = "ZEC price holds steady at $50.";
    } else if (t > 0.66) {
      message = "All vaults healthy. The system just works.";
    }
    points.push({
      block,
      extPrice,
      spotPrice: +spotPrice.toFixed(2),
      twapPrice: +twapPrice.toFixed(2),
      reserveZec,
      reserveZai,
      arberCapPct: 100,
      arberDirection: 0,
      vaultCounts: { green: 25, yellow: 0, red: 0, dead: 0 },
      liquidatingNow: [],
      badDebt: 0,
      whalePnL: 0,
      whaleAction: "idle",
      message,
      emphasis,
    });
  }
  return points;
})();

// --- Scene 2: The Crash (50 points, ~17s) ---
const SCENE_2_DATA = (() => {
  const points = [];
  const totalBlocks = 1000;

  // Build block sample array: 10 calm, 25 crash, 15 recovery
  const blocks = [];
  for (let i = 0; i < 10; i++) blocks.push(Math.round(lerp(1, 240, i / 9)));
  for (let i = 0; i < 25; i++) blocks.push(Math.round(lerp(245, 400, i / 24)));
  for (let i = 0; i < 15; i++) blocks.push(Math.round(lerp(420, 1000, i / 14)));

  // External price path from scenarios.rs
  function extAt(b) {
    if (b <= 249) return 50;
    if (b <= 349) return lerp(50, 20, (b - 250) / 100);
    if (b <= 599) return lerp(20, 35, (b - 350) / 250);
    return 35;
  }

  // AMM spot: lags external by ~37%
  function spotAt(b) {
    if (b <= 249) return 50;
    if (b <= 280) return lerp(50, 44, (b - 250) / 30);
    if (b <= 349) return lerp(44, 36, (b - 280) / 69);
    if (b <= 500) return lerp(36, 42, (b - 350) / 150);
    if (b <= 700) return lerp(42, 46, (b - 500) / 200);
    return lerp(46, 47.89, Math.min(1, (b - 700) / 300));
  }

  // TWAP: even more lagged (48-block window)
  function twapAt(b) {
    if (b <= 260) return 50;
    if (b <= 300) return lerp(50, 47, (b - 260) / 40);
    if (b <= 400) return lerp(47, 43, (b - 300) / 100);
    if (b <= 600) return lerp(43, 45, (b - 400) / 200);
    return lerp(45, 47.5, Math.min(1, (b - 600) / 400));
  }

  // Arber capital depletion
  function arberAt(b) {
    if (b <= 249) return 100;
    if (b <= 350) return lerp(100, 40, (b - 250) / 100);
    if (b <= 600) return lerp(40, 55, (b - 350) / 250);
    return lerp(55, 60, Math.min(1, (b - 600) / 400));
  }

  // Reserves
  function zecAt(b) {
    if (b <= 249) return 100000;
    if (b <= 349) return lerp(100000, 118000, (b - 250) / 100);
    if (b <= 600) return lerp(118000, 110000, (b - 350) / 250);
    return lerp(110000, 105000, Math.min(1, (b - 600) / 400));
  }
  function zaiAt(b) {
    const K = 5e11;
    return Math.round(K / zecAt(b));
  }

  // Liquidation bursts at blocks ~259, 267, 273, 284, 298
  const liqBursts = [259, 267, 273, 284, 298];
  let vaultsLiquidated = 0;

  for (let idx = 0; idx < blocks.length; idx++) {
    const b = blocks[idx];
    const ext = extAt(b);
    const spot = spotAt(b);
    const twap = twapAt(b);
    const arb = arberAt(b);
    const rZec = Math.round(zecAt(b));
    const rZai = zaiAt(b);

    // Count liquidations that have happened by this block
    const liqsBefore = liqBursts.filter((lb) => lb <= b).length;
    const deadNow = liqsBefore * 5;
    const liquidatingNow = [];
    // Check if we are near a burst block
    for (const lb of liqBursts) {
      if (Math.abs(b - lb) <= 3 && deadNow > vaultsLiquidated) {
        for (let v = vaultsLiquidated; v < deadNow && v < 25; v++) {
          liquidatingNow.push(v);
        }
      }
    }
    vaultsLiquidated = deadNow;

    const alive = 25 - Math.min(deadNow, 25);
    const yellowCount = b > 250 && b < 600 ? Math.min(alive, Math.round(lerp(0, 5, Math.min(1, (b - 250) / 100)))) : 0;
    const greenCount = alive - yellowCount;

    let message = "Everything looks fine...";
    let emphasis = false;
    if (b >= 250 && b < 260) {
      message = "EXTERNAL PRICE CRASHES";
      emphasis = true;
    } else if (b >= 260 && b < 300) {
      message = "The AMM doesn't follow. The elastic stretches.";
    } else if (b >= 300 && b < 350) {
      message = "Arber capital depleting... but AMM holds.";
    } else if (b >= 350 && b < 500) {
      message = "The AMM's ignorance IS the stability mechanism.";
    } else if (b >= 500) {
      message = "Zero bad debt. The system bent but didn't break.";
      emphasis = true;
    }

    let arberDir = 0;
    if (b > 250 && b < 400) arberDir = -1;
    else if (b >= 400 && b < 600) arberDir = 1;

    points.push({
      block: b,
      extPrice: +ext.toFixed(2),
      spotPrice: +spot.toFixed(2),
      twapPrice: +twap.toFixed(2),
      reserveZec: rZec,
      reserveZai: rZai,
      arberCapPct: +arb.toFixed(1),
      arberDirection: arberDir,
      vaultCounts: {
        green: greenCount,
        yellow: yellowCount,
        red: 0,
        dead: Math.min(deadNow, 25),
      },
      liquidatingNow,
      badDebt: 0,
      whalePnL: 0,
      whaleAction: "idle",
      message,
      emphasis,
    });
  }
  return points;
})();

// --- Scene 3: The Attack (50 points, ~17s) ---
const SCENE_3_DATA = (() => {
  const points = [];
  const blocks = [];
  for (let i = 0; i < 10; i++) blocks.push(Math.round(lerp(1, 50, i / 9)));
  for (let i = 0; i < 20; i++) blocks.push(Math.round(lerp(55, 150, i / 19)));
  for (let i = 0; i < 10; i++) blocks.push(Math.round(lerp(155, 250, i / 9)));
  for (let i = 0; i < 5; i++) blocks.push(Math.round(lerp(260, 350, i / 4)));
  for (let i = 0; i < 5; i++) blocks.push(Math.round(lerp(360, 500, i / 4)));

  function spotAt(b) {
    if (b <= 50) return 50;
    if (b <= 150) return lerp(50, 15, (b - 50) / 100);
    if (b <= 250) return lerp(15, 35, (b - 150) / 100);
    if (b <= 350) return lerp(35, 52, (b - 250) / 100);
    return lerp(52, 50, Math.min(1, (b - 350) / 150));
  }

  function twapAt(b) {
    if (b <= 70) return 50;
    if (b <= 180) return lerp(50, 28, (b - 70) / 110);
    if (b <= 280) return lerp(28, 40, (b - 180) / 100);
    if (b <= 380) return lerp(40, 49, (b - 280) / 100);
    return lerp(49, 50, Math.min(1, (b - 380) / 120));
  }

  function arberAt(b) {
    if (b <= 50) return 100;
    if (b <= 150) return lerp(100, 30, (b - 50) / 100);
    if (b <= 250) return lerp(30, 50, (b - 150) / 100);
    if (b <= 350) return lerp(50, 70, (b - 250) / 100);
    return lerp(70, 80, Math.min(1, (b - 350) / 150));
  }

  function zecAt(b) {
    if (b <= 50) return 100000;
    if (b <= 150) return lerp(100000, 150000, (b - 50) / 100);
    if (b <= 250) return lerp(150000, 115000, (b - 150) / 100);
    if (b <= 350) return lerp(115000, 98000, (b - 250) / 100);
    return lerp(98000, 100000, Math.min(1, (b - 350) / 150));
  }

  function pnlAt(b) {
    if (b <= 50) return 0;
    if (b <= 150) return lerp(0, -5000, (b - 50) / 100);
    if (b <= 250) return lerp(-5000, -10000, (b - 150) / 100);
    if (b <= 350) return lerp(-10000, -15000, (b - 250) / 100);
    return lerp(-15000, -17000, Math.min(1, (b - 350) / 150));
  }

  for (let idx = 0; idx < blocks.length; idx++) {
    const b = blocks[idx];
    const K = 5e11;
    const rZec = Math.round(zecAt(b));
    const rZai = Math.round(K / rZec);
    const spot = spotAt(b);
    const twap = twapAt(b);
    const arb = arberAt(b);
    const pnl = pnlAt(b);

    let whaleAction = "idle";
    let message = "A whale appears with 50,000 ZEC.";
    let emphasis = false;
    let arberDir = 0;

    if (b > 50 && b <= 150) {
      whaleAction = "dump";
      message = "WHALE DUMPS ZEC INTO THE AMM";
      emphasis = true;
      arberDir = 1;
    } else if (b > 150 && b <= 250) {
      whaleAction = "idle";
      message = "Arber fights back, buying cheap ZEC.";
      arberDir = 1;
    } else if (b > 250 && b <= 350) {
      whaleAction = "buy";
      message = "Whale buys back at a loss. The pool absorbs the blow.";
      arberDir = 0;
    } else if (b > 350) {
      whaleAction = "idle";
      message = "Attacking ZAI costs more than it's worth.";
      emphasis = true;
    }

    // Vaults stay mostly healthy since ext price is still $50
    const yellowCount = b > 80 && b < 200 ? Math.min(3, Math.round(lerp(0, 3, (b - 80) / 70))) : 0;

    points.push({
      block: b,
      extPrice: 50,
      spotPrice: +spot.toFixed(2),
      twapPrice: +twap.toFixed(2),
      reserveZec: rZec,
      reserveZai: rZai,
      arberCapPct: +arb.toFixed(1),
      arberDirection: arberDir,
      vaultCounts: {
        green: 25 - yellowCount,
        yellow: yellowCount,
        red: 0,
        dead: 0,
      },
      liquidatingNow: [],
      badDebt: 0,
      whalePnL: Math.round(pnl),
      whaleAction,
      message,
      emphasis,
    });
  }
  return points;
})();

// --- Scene 4: The Ultimate Test (30 points, ~10s) ---
const SCENE_4_DATA = (() => {
  const points = [];
  const totalBlocks = 10000;
  const blocks = [];
  for (let i = 0; i < 30; i++) blocks.push(Math.round(lerp(1, 10000, i / 29)));

  function extAt(b) {
    return lerp(50, 5, b / totalBlocks);
  }

  // Arber exhausts around block 2000; spot freezes at ~40-42
  function spotAt(b) {
    if (b <= 2000) return lerp(50, 40, b / 2000);
    return lerp(40, 42, Math.sin((b - 2000) / 3000) * 0.5 + 0.5);
  }

  function twapAt(b) {
    if (b <= 2200) return lerp(50, 41, b / 2200);
    return lerp(41, 42, Math.sin((b - 2200) / 3500) * 0.5 + 0.5);
  }

  function arberAt(b) {
    if (b <= 500) return lerp(100, 60, b / 500);
    if (b <= 1500) return lerp(60, 10, (b - 500) / 1000);
    if (b <= 2000) return lerp(10, 0, (b - 1500) / 500);
    return 0;
  }

  function zecAt(b) {
    if (b <= 2000) return lerp(200000, 240000, b / 2000);
    return 240000;
  }

  for (let idx = 0; idx < blocks.length; idx++) {
    const b = blocks[idx];
    const K = 2e12; // $20M pool: 200K ZEC * $10M ZAI
    const rZec = Math.round(zecAt(b));
    const rZai = Math.round(K / rZec);
    const ext = extAt(b);
    const spot = spotAt(b);
    const twap = twapAt(b);
    const arb = arberAt(b);

    let message = "43 days. ZEC drops 90%.";
    let emphasis = false;
    const dayEst = Math.round((b / 48) * 75 / 3600); // rough day estimate

    if (b <= 1000) {
      message = "43 days. ZEC drops 90%.";
    } else if (b <= 2000) {
      message = "Arber tries to keep up...";
    } else if (b <= 2500) {
      message = "Capital exhausted at day 4.";
      emphasis = true;
    } else if (b <= 5000) {
      message = "AMM price freezes. It can't see the crash.";
    } else if (b <= 8000) {
      message = "ZERO BAD DEBT.";
      emphasis = true;
    } else {
      message = "Price inertia IS the stability mechanism.";
      emphasis = true;
    }

    // Vaults slowly go yellow but never liquidate
    const t = b / totalBlocks;
    const yellowCount = Math.min(15, Math.round(t * 15));
    const greenCount = 25 - yellowCount;

    points.push({
      block: b,
      extPrice: +ext.toFixed(2),
      spotPrice: +spot.toFixed(2),
      twapPrice: +twap.toFixed(2),
      reserveZec: rZec,
      reserveZai: rZai,
      arberCapPct: +arb.toFixed(1),
      arberDirection: arb > 0 ? -1 : 0,
      vaultCounts: { green: greenCount, yellow: yellowCount, red: 0, dead: 0 },
      liquidatingNow: [],
      badDebt: 0,
      whalePnL: 0,
      whaleAction: "idle",
      message,
      emphasis,
    });
  }
  return points;
})();

const SCENES = [
  { name: "Normal Day", data: SCENE_1_DATA },
  { name: "The Crash", data: SCENE_2_DATA },
  { name: "The Attack", data: SCENE_3_DATA },
  { name: "The Ultimate Test", data: SCENE_4_DATA },
];

// ======= SECTION: SVG SUB-COMPONENTS =======

// --- MessageBanner ---
function MessageBanner({ message, emphasis }) {
  return (
    <g>
      {emphasis && (
        <motion.text
          x={600}
          y={42}
          textAnchor="middle"
          fill={C.Warning}
          fontSize={30}
          fontWeight="bold"
          fontFamily="monospace"
          opacity={0.15}
          filter="url(#glow)"
        >
          {message}
        </motion.text>
      )}
      <motion.text
        x={600}
        y={42}
        textAnchor="middle"
        fill={emphasis ? C.Warning : C.Text}
        fontSize={emphasis ? 28 : 22}
        fontWeight={emphasis ? "bold" : "normal"}
        fontFamily="monospace"
      >
        {message}
      </motion.text>
    </g>
  );
}

// --- VaultGrid ---
function VaultGrid({ vaultCounts, liquidatingNow }) {
  const cols = 5;
  const size = 36;
  const gap = 6;
  const startX = 600 - ((cols * size + (cols - 1) * gap) / 2);
  const startY = 72;

  const colors = [];
  for (let i = 0; i < vaultCounts.green; i++) colors.push(C.Healthy);
  for (let i = 0; i < vaultCounts.yellow; i++) colors.push(C.Warning);
  for (let i = 0; i < vaultCounts.red; i++) colors.push(C.Danger);
  for (let i = 0; i < vaultCounts.dead; i++) colors.push(C.Liquidated);
  while (colors.length < 25) colors.push(C.Liquidated);

  return (
    <g>
      <text x={startX} y={startY - 6} fill={C.Muted} fontSize={11} fontFamily="monospace">
        VAULTS ({25 - vaultCounts.dead}/25 active)
      </text>
      {colors.map((color, i) => {
        const row = Math.floor(i / cols);
        const col = i % cols;
        const cx = startX + col * (size + gap);
        const cy = startY + 4 + row * (size + gap);
        const isLiquidating = liquidatingNow.includes(i);
        return (
          <motion.rect
            key={i}
            x={cx}
            y={cy}
            width={size}
            height={size}
            rx={4}
            fill={color}
            opacity={isLiquidating ? 1 : 0.85}
            stroke={isLiquidating ? C.Danger : "none"}
            strokeWidth={isLiquidating ? 3 : 0}
            animate={
              isLiquidating
                ? { opacity: [1, 0.3, 1], stroke: [C.Danger, "#FF0000", C.Danger] }
                : {}
            }
            transition={isLiquidating ? { duration: 0.4, repeat: 2 } : {}}
          />
        );
      })}
    </g>
  );
}

// --- PriceThermometer ---
function PriceThermometer({ extPriceSpring }) {
  const x = 60;
  const y = 260;
  const w = 50;
  const h = 300;
  const maxPrice = 60;

  const fillHeight = useTransform(extPriceSpring, (v) => (v / maxPrice) * h);
  const fillY = useTransform(fillHeight, (fh) => y + h - fh);

  const labelY = useTransform(fillY, (fy) => Math.max(y + 14, Math.min(y + h - 8, fy - 6)));
  const priceText = useTransform(extPriceSpring, (v) => "$" + v.toFixed(1));
  const fillColor = useTransform(extPriceSpring, (v) => {
    if (v > 40) return C.Healthy;
    if (v > 25) return C.Warning;
    return C.Danger;
  });

  return (
    <g>
      <text x={x + w / 2} y={y - 8} textAnchor="middle" fill={C.Muted} fontSize={11} fontFamily="monospace">
        External
      </text>
      {/* Outer frame */}
      <rect x={x} y={y} width={w} height={h} rx={6} fill={C.Panel} stroke={C.Muted} strokeWidth={1} />
      {/* Scale marks */}
      {[0, 10, 20, 30, 40, 50, 60].map((p) => {
        const my = y + h - (p / maxPrice) * h;
        return (
          <g key={p}>
            <line x1={x} y1={my} x2={x + 8} y2={my} stroke={C.Muted} strokeWidth={0.5} />
            <text x={x - 4} y={my + 3} textAnchor="end" fill={C.Muted} fontSize={8} fontFamily="monospace">
              {p}
            </text>
          </g>
        );
      })}
      {/* Clip for mercury fill */}
      <defs>
        <clipPath id="thermo-clip">
          <rect x={x + 4} y={y} width={w - 8} height={h} rx={4} />
        </clipPath>
      </defs>
      <motion.rect
        x={x + 4}
        width={w - 8}
        rx={4}
        style={{ y: fillY, height: fillHeight }}
        fill={fillColor}
        clipPath="url(#thermo-clip)"
        opacity={0.9}
      />
      {/* Price label */}
      <motion.text
        x={x + w / 2}
        textAnchor="middle"
        fill={C.Text}
        fontSize={13}
        fontWeight="bold"
        fontFamily="monospace"
        style={{ y: labelY }}
      >
        {priceText}
      </motion.text>
    </g>
  );
}

// --- ElasticBand ---
function ElasticBand({ extPriceSpring, spotPriceSpring }) {
  const thermoX = 110;
  const utubeX = 300;
  const baseY = 260;
  const h = 300;
  const maxPrice = 60;

  const extY = useTransform(extPriceSpring, (v) => baseY + h - (v / maxPrice) * h);
  const spotY = useTransform(spotPriceSpring, (v) => baseY + h - (v / maxPrice) * h);
  const divergence = useTransform(
    [extPriceSpring, spotPriceSpring],
    ([ext, spot]) => Math.abs(ext - spot) / Math.max(ext, 1) * 100
  );

  const pathD = useTransform([extY, spotY], ([ey, sy]) => {
    const midX = (thermoX + utubeX) / 2;
    // Zigzag spring path
    const steps = 8;
    let d = `M ${thermoX} ${ey}`;
    const dx = (utubeX - thermoX) / steps;
    const amp = Math.abs(ey - sy) * 0.15 + 3;
    for (let i = 1; i <= steps; i++) {
      const px = thermoX + dx * i;
      const py = lerp(ey, sy, i / steps) + (i % 2 === 0 ? -amp : amp);
      d += ` L ${px} ${py}`;
    }
    d += ` L ${utubeX} ${sy}`;
    return d;
  });

  const bandColor = useTransform(divergence, (d) => {
    if (d < 5) return C.Healthy;
    if (d < 15) return C.Warning;
    return C.Danger;
  });

  const bandOpacity = useTransform(divergence, (d) => Math.min(1, 0.3 + d / 30));
  const divText = useTransform(divergence, (d) => (d > 5 ? `TWAP lag: ${d.toFixed(0)}%` : ""));
  const labelX = (thermoX + utubeX) / 2;
  const labelY = useTransform([extY, spotY], ([ey, sy]) => Math.min(ey, sy) - 10);

  return (
    <g>
      <motion.path
        d={pathD}
        fill="none"
        stroke={bandColor}
        strokeWidth={2}
        strokeDasharray="6 3"
        opacity={bandOpacity}
      />
      <motion.text
        x={labelX}
        textAnchor="middle"
        fill={C.Warning}
        fontSize={11}
        fontFamily="monospace"
        fontWeight="bold"
        style={{ y: labelY }}
      >
        {divText}
      </motion.text>
    </g>
  );
}

// --- UTube (centerpiece) ---
function UTube({ reserveZecSpring, reserveZaiSpring, spotPriceSpring, currentScene }) {
  const ox = 300;
  const oy = 260;
  const tubeW = 500;
  const tubeH = 300;
  const colW = 80;
  const innerW = 60;

  // Column positions
  const leftX = ox;
  const rightX = ox + tubeW - colW;
  const bottomY = oy + tubeH - 40;

  // Normalize reserves: scene-dependent ranges
  const zecRange = currentScene === 3
    ? [180000, 260000]
    : currentScene === 2
    ? [95000, 125000]
    : [95000, 155000];
  const zaiRange = currentScene === 3
    ? [3000000, 12000000]
    : currentScene === 2
    ? [4000000, 5500000]
    : [3000000, 6000000];

  const maxFillH = tubeH - 80;

  const zecFillH = useTransform(reserveZecSpring, (v) => {
    const t = Math.max(0, Math.min(1, (v - zecRange[0]) / (zecRange[1] - zecRange[0])));
    return 40 + t * maxFillH;
  });
  const zaiFillH = useTransform(reserveZaiSpring, (v) => {
    const t = Math.max(0, Math.min(1, (v - zaiRange[0]) / (zaiRange[1] - zaiRange[0])));
    return 40 + t * maxFillH;
  });

  const zecFillY = useTransform(zecFillH, (h) => bottomY - h);
  const zaiFillY = useTransform(zaiFillH, (h) => bottomY - h);

  const spotText = useTransform(spotPriceSpring, (v) => "$" + v.toFixed(2));
  const kValue = useTransform(
    [reserveZecSpring, reserveZaiSpring],
    ([zec, zai]) => fmtSci(zec * (zai / zec) * zec > 0 ? zec * zai : 0)
  );
  const kText = useTransform([reserveZecSpring, reserveZaiSpring], ([zec, zai]) => "k = " + fmtSci(zec * zai));

  // U-tube bottom path
  const bottomPath = `M ${leftX + colW} ${bottomY} Q ${leftX + colW + 30} ${bottomY + 40} ${ox + tubeW / 2} ${bottomY + 40} Q ${rightX - 30} ${bottomY + 40} ${rightX} ${bottomY}`;

  return (
    <g>
      {/* U-tube structure */}
      <defs>
        <clipPath id="left-col-clip">
          <rect x={leftX + 10} y={oy} width={innerW} height={tubeH} />
        </clipPath>
        <clipPath id="right-col-clip">
          <rect x={rightX + 10} y={oy} width={innerW} height={tubeH} />
        </clipPath>
      </defs>

      {/* Left column frame (ZEC) */}
      <rect x={leftX} y={oy} width={colW} height={tubeH - 40} rx={6} fill={C.Panel} stroke={C.ZEC} strokeWidth={1.5} opacity={0.7} />
      {/* Right column frame (ZAI) */}
      <rect x={rightX} y={oy} width={colW} height={tubeH - 40} rx={6} fill={C.Panel} stroke={C.ZAI} strokeWidth={1.5} opacity={0.7} />
      {/* Bottom connection */}
      <path d={bottomPath} fill="none" stroke={C.Muted} strokeWidth={1.5} opacity={0.5} />

      {/* ZEC fluid */}
      <motion.rect
        x={leftX + 10}
        width={innerW}
        rx={4}
        fill={C.ZEC}
        opacity={0.7}
        style={{ y: zecFillY, height: zecFillH }}
        clipPath="url(#left-col-clip)"
      />
      {/* ZAI fluid */}
      <motion.rect
        x={rightX + 10}
        width={innerW}
        rx={4}
        fill={C.ZAI}
        opacity={0.7}
        style={{ y: zaiFillY, height: zaiFillH }}
        clipPath="url(#right-col-clip)"
      />

      {/* Column labels */}
      <text x={leftX + colW / 2} y={oy - 8} textAnchor="middle" fill={C.ZEC} fontSize={14} fontWeight="bold" fontFamily="monospace">
        ZEC
      </text>
      <text x={rightX + colW / 2} y={oy - 8} textAnchor="middle" fill={C.ZAI} fontSize={14} fontWeight="bold" fontFamily="monospace">
        ZAI
      </text>

      {/* Center price dial */}
      <circle cx={ox + tubeW / 2} cy={oy + tubeH / 2 - 20} r={42} fill={C.Panel} stroke={C.Text} strokeWidth={2} />
      <text x={ox + tubeW / 2} y={oy + tubeH / 2 - 32} textAnchor="middle" fill={C.Muted} fontSize={9} fontFamily="monospace">
        AMM SPOT
      </text>
      <motion.text
        x={ox + tubeW / 2}
        y={oy + tubeH / 2 - 14}
        textAnchor="middle"
        fill={C.Text}
        fontSize={18}
        fontWeight="bold"
        fontFamily="monospace"
      >
        {spotText}
      </motion.text>

      {/* k label */}
      <motion.text
        x={ox + tubeW / 2}
        y={oy + tubeH / 2 + 6}
        textAnchor="middle"
        fill={C.Muted}
        fontSize={10}
        fontFamily="monospace"
      >
        {kText}
      </motion.text>
    </g>
  );
}

// --- ArberRobot ---
function ArberRobot({ arberCapSpring, arberDirSpring }) {
  const baseX = 185;
  const baseY = 380;

  const xShift = useTransform(arberDirSpring, (d) => d * 20);
  const robotX = useTransform(xShift, (s) => baseX + s);

  const capWidth = useTransform(arberCapSpring, (c) => Math.max(0, (c / 100) * 30));
  const capColor = useTransform(arberCapSpring, (c) => {
    if (c > 50) return C.Healthy;
    if (c > 20) return C.Warning;
    return C.Danger;
  });

  const eyeOpacity = useTransform(arberCapSpring, (c) => (c > 0 ? 1 : 0.3));
  const bodyRotate = useTransform(arberCapSpring, (c) => (c <= 0 ? 8 : 0));
  const capText = useTransform(arberCapSpring, (c) => Math.round(c) + "%");

  return (
    <motion.g style={{ x: xShift }}>
      <g transform={`translate(${baseX}, ${baseY})`}>
        {/* Body */}
        <motion.g style={{ rotate: bodyRotate }}>
          <rect x={-18} y={0} width={36} height={44} rx={6} fill={C.Arber} opacity={0.9} />
          {/* Head */}
          <rect x={-14} y={-24} width={28} height={22} rx={5} fill={C.Arber} />
          {/* Eyes */}
          <motion.circle cx={-5} cy={-14} r={3.5} fill="white" opacity={eyeOpacity} />
          <motion.circle cx={8} cy={-14} r={3.5} fill="white" opacity={eyeOpacity} />
          <motion.circle cx={-5} cy={-14} r={1.5} fill={C.BG} opacity={eyeOpacity} />
          <motion.circle cx={8} cy={-14} r={1.5} fill={C.BG} opacity={eyeOpacity} />
          {/* Antenna */}
          <line x1={0} y1={-24} x2={0} y2={-32} stroke={C.Arber} strokeWidth={2} />
          <circle cx={0} cy={-34} r={3} fill={C.Warning} />
          {/* Arms */}
          <rect x={-26} y={6} width={8} height={24} rx={3} fill={C.Arber} opacity={0.7} />
          <rect x={18} y={6} width={8} height={24} rx={3} fill={C.Arber} opacity={0.7} />
        </motion.g>

        {/* Battery indicator */}
        <rect x={-17} y={50} width={34} height={14} rx={3} fill="none" stroke={C.Muted} strokeWidth={1} />
        <motion.rect x={-15} y={52} height={10} rx={2} fill={capColor} style={{ width: capWidth }} />
        <motion.text x={0} y={77} textAnchor="middle" fill={C.Muted} fontSize={9} fontFamily="monospace">
          {capText}
        </motion.text>

        {/* Label */}
        <text x={0} y={92} textAnchor="middle" fill={C.Arber} fontSize={11} fontWeight="bold" fontFamily="monospace">
          ARBER
        </text>
      </g>
    </motion.g>
  );
}

// --- Whale (Scene 3 only) ---
function WhaleCharacter({ whalePnLSpring, whaleAction, visible }) {
  const baseX = 900;
  const baseY = 340;

  const pnlText = useTransform(whalePnLSpring, (v) => {
    if (Math.abs(v) < 1) return "$0";
    const sign = v >= 0 ? "+" : "-";
    return sign + "$" + fmtInt(Math.abs(v));
  });
  const pnlColor = useTransform(whalePnLSpring, (v) => (v >= 0 ? C.Healthy : C.Danger));

  const bobY = useTransform(whalePnLSpring, () => 0);

  if (!visible) return null;

  return (
    <motion.g
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
      transition={{ duration: 0.5 }}
    >
      <g transform={`translate(${baseX}, ${baseY})`}>
        {/* Body */}
        <motion.g
          animate={whaleAction === "idle" ? { y: [0, -5, 0] } : {}}
          transition={whaleAction === "idle" ? { duration: 2, repeat: Infinity, ease: "easeInOut" } : {}}
        >
          <ellipse cx={0} cy={0} rx={50} ry={30} fill={C.Whale} />
          {/* Tail */}
          <path d="M 45 0 L 70 -18 L 70 18 Z" fill={C.Whale} opacity={0.8} />
          {/* Eye */}
          <circle cx={-20} cy={-8} r={6} fill="white" />
          <circle cx={-20} cy={-8} r={3} fill={C.BG} />
          {/* Mouth */}
          <path d="M -35 5 Q -25 12 -15 5" fill="none" stroke={C.BG} strokeWidth={1.5} />
          {/* Blow hole */}
          {whaleAction === "dump" && (
            <motion.g
              animate={{ opacity: [1, 0.3, 1] }}
              transition={{ duration: 0.5, repeat: Infinity }}
            >
              <rect x={-40} y={-50} width={6} height={6} rx={1} fill={C.ZEC} opacity={0.8} />
              <rect x={-30} y={-60} width={6} height={6} rx={1} fill={C.ZEC} opacity={0.6} />
              <rect x={-50} y={-55} width={6} height={6} rx={1} fill={C.ZEC} opacity={0.7} />
              <rect x={-35} y={-70} width={5} height={5} rx={1} fill={C.ZEC} opacity={0.5} />
            </motion.g>
          )}
        </motion.g>

        {/* P&L counter */}
        <motion.text
          x={0}
          y={-45}
          textAnchor="middle"
          fill={pnlColor}
          fontSize={16}
          fontWeight="bold"
          fontFamily="monospace"
        >
          {pnlText}
        </motion.text>

        {/* Label */}
        <text x={0} y={50} textAnchor="middle" fill={C.Whale} fontSize={11} fontWeight="bold" fontFamily="monospace">
          WHALE
        </text>
      </g>
    </motion.g>
  );
}

// --- StatsPanel ---
function StatsPanel({ data }) {
  if (!data) return null;
  const y = 640;
  const panelH = 120;
  const pegDeviation = Math.abs(data.extPrice - data.spotPrice) / Math.max(data.extPrice, 1) * 100;
  const activeVaults = data.vaultCounts.green + data.vaultCounts.yellow + data.vaultCounts.red;

  return (
    <g>
      <rect x={30} y={y} width={1140} height={panelH} rx={8} fill={C.Panel} opacity={0.9} />

      {/* Column 1: Prices */}
      <text x={60} y={y + 20} fill={C.Muted} fontSize={10} fontFamily="monospace">PRICES</text>
      <text x={60} y={y + 36} fill={C.Muted} fontSize={11} fontFamily="monospace">
        Block: <tspan fill={C.Text}>#{fmtInt(data.block)}</tspan>
      </text>
      <text x={60} y={y + 50} fill={C.Muted} fontSize={11} fontFamily="monospace">
        External: <tspan fill={C.Text}>${fmt(data.extPrice)}</tspan>
      </text>
      <text x={60} y={y + 64} fill={C.Muted} fontSize={11} fontFamily="monospace">
        AMM Spot: <tspan fill={C.Text}>${fmt(data.spotPrice)}</tspan>
      </text>
      <text x={60} y={y + 78} fill={C.Muted} fontSize={11} fontFamily="monospace">
        TWAP: <tspan fill={C.Text}>${fmt(data.twapPrice)}</tspan>
      </text>

      {/* Column 2: Pool */}
      <text x={420} y={y + 20} fill={C.Muted} fontSize={10} fontFamily="monospace">POOL</text>
      <text x={420} y={y + 36} fill={C.Muted} fontSize={11} fontFamily="monospace">
        Reserve ZEC: <tspan fill={C.ZEC}>{fmtInt(data.reserveZec)}</tspan>
      </text>
      <text x={420} y={y + 50} fill={C.Muted} fontSize={11} fontFamily="monospace">
        Reserve ZAI: <tspan fill={C.ZAI}>{fmtInt(data.reserveZai)}</tspan>
      </text>
      <text x={420} y={y + 64} fill={C.Muted} fontSize={11} fontFamily="monospace">
        k: <tspan fill={C.Text}>{fmtSci(data.reserveZec * data.reserveZai)}</tspan>
      </text>

      {/* Column 3: System */}
      <text x={780} y={y + 20} fill={C.Muted} fontSize={10} fontFamily="monospace">SYSTEM</text>
      <text x={780} y={y + 36} fill={C.Muted} fontSize={11} fontFamily="monospace">
        Peg Deviation: <tspan fill={pegDeviation > 10 ? C.Danger : pegDeviation > 5 ? C.Warning : C.Healthy}>{fmt(pegDeviation, 1)}%</tspan>
      </text>
      <text x={780} y={y + 50} fill={C.Muted} fontSize={11} fontFamily="monospace">
        Bad Debt: <tspan fill={data.badDebt > 0 ? C.Danger : C.Healthy}>${fmtInt(data.badDebt)}</tspan>
      </text>
      <text x={780} y={y + 64} fill={C.Muted} fontSize={11} fontFamily="monospace">
        Active Vaults: <tspan fill={C.Text}>{activeVaults}/25</tspan>
      </text>
      <text x={780} y={y + 78} fill={C.Muted} fontSize={11} fontFamily="monospace">
        Arber Capital: <tspan fill={data.arberCapPct > 50 ? C.Healthy : data.arberCapPct > 20 ? C.Warning : C.Danger}>{fmt(data.arberCapPct, 0)}%</tspan>
      </text>
    </g>
  );
}

// ======= SECTION: TWAP INDICATOR =======

function TwapIndicator({ twapPriceSpring, spotPriceSpring }) {
  const ox = 300;
  const tubeW = 500;
  const centerX = ox + tubeW / 2;
  const baseY = 570;

  const twapText = useTransform(twapPriceSpring, (v) => "$" + v.toFixed(2));
  const lagPct = useTransform(
    [twapPriceSpring, spotPriceSpring],
    ([twap, spot]) => {
      const diff = Math.abs(twap - spot) / Math.max(spot, 1) * 100;
      return diff > 2 ? `(${diff.toFixed(0)}% lag)` : "";
    }
  );

  return (
    <g>
      <text x={centerX} y={baseY} textAnchor="middle" fill={C.Muted} fontSize={10} fontFamily="monospace">
        TWAP (48-block window)
      </text>
      <motion.text
        x={centerX}
        y={baseY + 16}
        textAnchor="middle"
        fill={C.Warning}
        fontSize={15}
        fontWeight="bold"
        fontFamily="monospace"
      >
        {twapText}
      </motion.text>
      <motion.text
        x={centerX}
        y={baseY + 30}
        textAnchor="middle"
        fill={C.Danger}
        fontSize={10}
        fontFamily="monospace"
      >
        {lagPct}
      </motion.text>
    </g>
  );
}

// ======= SECTION: SCENE TITLE =======

function SceneTitle({ sceneName, sceneIndex }) {
  return (
    <g>
      <text x={1140} y={72} textAnchor="end" fill={C.Muted} fontSize={12} fontFamily="monospace">
        Scene {sceneIndex + 1}/4
      </text>
      <text x={1140} y={90} textAnchor="end" fill={C.Text} fontSize={16} fontWeight="bold" fontFamily="monospace">
        {sceneName}
      </text>
    </g>
  );
}

// ======= SECTION: MAIN APP COMPONENT =======

function App() {
  const [currentScene, setCurrentScene] = useState(0);
  const [dataIndex, setDataIndex] = useState(0);
  const [isPlaying, setIsPlaying] = useState(true);
  const [speed, setSpeed] = useState(1);
  const [playAllMode, setPlayAllMode] = useState(true);
  const [transitioning, setTransitioning] = useState(false);
  const [finished, setFinished] = useState(false);
  const intervalRef = useRef(null);
  const transitionTimerRef = useRef(null);

  const sceneData = SCENES[currentScene].data;
  const currentPoint = sceneData[dataIndex] || sceneData[0];

  // --- Motion values and springs ---
  const extPriceMV = useMotionValue(currentPoint.extPrice);
  const spotPriceMV = useMotionValue(currentPoint.spotPrice);
  const twapPriceMV = useMotionValue(currentPoint.twapPrice);
  const reserveZecMV = useMotionValue(currentPoint.reserveZec);
  const reserveZaiMV = useMotionValue(currentPoint.reserveZai);
  const arberCapMV = useMotionValue(currentPoint.arberCapPct);
  const arberDirMV = useMotionValue(currentPoint.arberDirection);
  const whalePnLMV = useMotionValue(currentPoint.whalePnL);

  const extPriceSpring = useSpring(extPriceMV, SPRING.prices);
  const spotPriceSpring = useSpring(spotPriceMV, SPRING.prices);
  const twapPriceSpring = useSpring(twapPriceMV, SPRING.twap);
  const reserveZecSpring = useSpring(reserveZecMV, SPRING.fluid);
  const reserveZaiSpring = useSpring(reserveZaiMV, SPRING.fluid);
  const arberCapSpring = useSpring(arberCapMV, SPRING.arber);
  const arberDirSpring = useSpring(arberDirMV, SPRING.arber);
  const whalePnLSpring = useSpring(whalePnLMV, SPRING.prices);

  // Update motion values when data changes
  useEffect(() => {
    const d = sceneData[dataIndex];
    if (!d) return;
    extPriceMV.set(d.extPrice);
    spotPriceMV.set(d.spotPrice);
    twapPriceMV.set(d.twapPrice);
    reserveZecMV.set(d.reserveZec);
    reserveZaiMV.set(d.reserveZai);
    arberCapMV.set(d.arberCapPct);
    arberDirMV.set(d.arberDirection);
    whalePnLMV.set(d.whalePnL);
  }, [dataIndex, currentScene]);

  // Cleanup transition timer on unmount
  useEffect(() => {
    return () => {
      if (transitionTimerRef.current) clearTimeout(transitionTimerRef.current);
    };
  }, []);

  // Playback timer
  useEffect(() => {
    if (intervalRef.current) clearInterval(intervalRef.current);
    if (!isPlaying) return;

    intervalRef.current = setInterval(() => {
      setDataIndex((prev) => {
        if (prev >= sceneData.length - 1) {
          if (playAllMode && currentScene < 3) {
            // Auto-advance to next scene with transition
            setTransitioning(true);
            setIsPlaying(false);
            if (transitionTimerRef.current) clearTimeout(transitionTimerRef.current);
            transitionTimerRef.current = setTimeout(() => {
              const next = currentScene + 1;
              setCurrentScene(next);
              setDataIndex(0);
              setTransitioning(false);
              setIsPlaying(true);
              // Jump MVs to first frame of next scene
              const d = SCENES[next].data[0];
              extPriceMV.set(d.extPrice);
              spotPriceMV.set(d.spotPrice);
              twapPriceMV.set(d.twapPrice);
              reserveZecMV.set(d.reserveZec);
              reserveZaiMV.set(d.reserveZai);
              arberCapMV.set(d.arberCapPct);
              arberDirMV.set(d.arberDirection);
              whalePnLMV.set(d.whalePnL);
            }, 1500 / speed);
          } else if (playAllMode && currentScene === 3) {
            setFinished(true);
            setIsPlaying(false);
          } else {
            setIsPlaying(false);
          }
          return prev;
        }
        return prev + 1;
      });
    }, 333 / speed);

    return () => {
      if (intervalRef.current) clearInterval(intervalRef.current);
    };
  }, [isPlaying, speed, sceneData.length, currentScene, playAllMode]);

  // Scene change handler
  const changeScene = useCallback(
    (idx) => {
      if (transitionTimerRef.current) clearTimeout(transitionTimerRef.current);
      setTransitioning(false);
      setFinished(false);
      setCurrentScene(idx);
      setDataIndex(0);
      setIsPlaying(true);
      // Jump motion values to first frame of new scene
      const d = SCENES[idx].data[0];
      extPriceMV.set(d.extPrice);
      spotPriceMV.set(d.spotPrice);
      twapPriceMV.set(d.twapPrice);
      reserveZecMV.set(d.reserveZec);
      reserveZaiMV.set(d.reserveZai);
      arberCapMV.set(d.arberCapPct);
      arberDirMV.set(d.arberDirection);
      whalePnLMV.set(d.whalePnL);
    },
    []
  );

  // Scrub handler
  const handleScrub = useCallback(
    (e) => {
      const val = parseInt(e.target.value, 10);
      setDataIndex(val);
    },
    []
  );

  // ======= RENDER =======
  return (
    <div style={{ background: C.BG, minHeight: "100vh", display: "flex", flexDirection: "column", alignItems: "center", padding: "20px 0" }}>
      {/* Title */}
      <h1 style={{ color: C.Text, fontFamily: "monospace", fontSize: 24, margin: "0 0 6px 0", letterSpacing: 2 }}>
        ZAI Flatcoin Simulator
      </h1>
      <p style={{ color: C.Muted, fontFamily: "monospace", fontSize: 13, margin: "0 0 16px 0" }}>
        Oracle-free CDP stability visualized
      </p>

      {/* SVG Canvas */}
      <svg
        viewBox="0 0 1200 800"
        width="1200"
        height="800"
        style={{ background: C.BG, borderRadius: 12, border: `1px solid ${C.Panel}`, maxWidth: "100%" }}
      >
        {/* Defs */}
        <defs>
          <filter id="glow">
            <feGaussianBlur stdDeviation="6" result="blur" />
            <feMerge>
              <feMergeNode in="blur" />
              <feMergeNode in="SourceGraphic" />
            </feMerge>
          </filter>
        </defs>

        {/* Message Banner */}
        <MessageBanner message={currentPoint.message} emphasis={currentPoint.emphasis} />

        {/* Scene Title */}
        <SceneTitle sceneName={SCENES[currentScene].name} sceneIndex={currentScene} />

        {/* Vault Grid */}
        <VaultGrid vaultCounts={currentPoint.vaultCounts} liquidatingNow={currentPoint.liquidatingNow} />

        {/* Price Thermometer */}
        <PriceThermometer extPriceSpring={extPriceSpring} />

        {/* Elastic Band */}
        <ElasticBand extPriceSpring={extPriceSpring} spotPriceSpring={spotPriceSpring} />

        {/* U-Tube */}
        <UTube
          reserveZecSpring={reserveZecSpring}
          reserveZaiSpring={reserveZaiSpring}
          spotPriceSpring={spotPriceSpring}
          currentScene={currentScene}
        />

        {/* TWAP Indicator */}
        <TwapIndicator twapPriceSpring={twapPriceSpring} spotPriceSpring={spotPriceSpring} />

        {/* Arber Robot */}
        <ArberRobot arberCapSpring={arberCapSpring} arberDirSpring={arberDirSpring} />

        {/* Whale (Scene 3 only) */}
        <WhaleCharacter
          whalePnLSpring={whalePnLSpring}
          whaleAction={currentPoint.whaleAction}
          visible={currentScene === 2}
        />

        {/* Stats Panel */}
        <StatsPanel data={currentPoint} />

        {/* Transition fade overlay */}
        {transitioning && (
          <motion.g
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            transition={{ duration: 0.5 }}
          >
            <rect x={0} y={0} width={1200} height={800} fill={C.BG} opacity={0.85} />
            <text x={600} y={380} textAnchor="middle" fill={C.Text} fontSize={28}
              fontWeight="bold" fontFamily="monospace">
              {currentScene < 3 ? SCENES[currentScene + 1].name : ""}
            </text>
            <text x={600} y={420} textAnchor="middle" fill={C.Muted} fontSize={14}
              fontFamily="monospace">
              {currentScene < 3 ? `Scene ${currentScene + 2} of 4` : ""}
            </text>
          </motion.g>
        )}

        {/* Finished summary screen */}
        {finished && (
          <g>
            <rect x={0} y={0} width={1200} height={800} fill={C.BG} opacity={0.9} />
            <text x={600} y={340} textAnchor="middle" fill={C.Text} fontSize={32}
              fontWeight="bold" fontFamily="monospace">
              46 findings. 238 tests.
            </text>
            <text x={600} y={390} textAnchor="middle" fill={C.Healthy} fontSize={36}
              fontWeight="bold" fontFamily="monospace">
              Zero bad debt.
            </text>
            <text x={600} y={450} textAnchor="middle" fill={C.Muted} fontSize={14}
              fontFamily="monospace">
              ZAI: Oracle-free stability on Zcash
            </text>
          </g>
        )}
      </svg>

      {/* ======= PLAYBACK CONTROLS ======= */}
      <div
        style={{
          display: "flex",
          flexWrap: "wrap",
          alignItems: "center",
          justifyContent: "center",
          gap: 12,
          marginTop: 16,
          padding: "12px 20px",
          background: C.Panel,
          borderRadius: 10,
          maxWidth: 1200,
          width: "100%",
          boxSizing: "border-box",
        }}
      >
        {/* Play All / Single toggle */}
        <button
          onClick={() => { setPlayAllMode((m) => !m); setFinished(false); }}
          style={{
            background: playAllMode ? C.ZAI : C.BG,
            color: C.Text,
            border: `1px solid ${playAllMode ? C.ZAI : C.Muted}`,
            borderRadius: 6,
            padding: "6px 14px",
            fontFamily: "monospace",
            fontSize: 12,
            cursor: "pointer",
            fontWeight: "bold",
            transition: "all 0.2s",
          }}
        >
          {playAllMode ? "Play All" : "Single"}
        </button>

        {/* Scene selector */}
        <div style={{ display: "flex", gap: 4 }}>
          {SCENES.map((s, i) => (
            <button
              key={i}
              onClick={() => {
                setPlayAllMode(false);
                setFinished(false);
                changeScene(i);
              }}
              style={{
                background: currentScene === i ? C.ZEC : C.BG,
                color: C.Text,
                border: `1px solid ${currentScene === i ? C.ZEC : C.Muted}`,
                borderRadius: 6,
                padding: "6px 14px",
                fontFamily: "monospace",
                fontSize: 12,
                cursor: "pointer",
                fontWeight: currentScene === i ? "bold" : "normal",
                transition: "all 0.2s",
              }}
            >
              {i + 1}. {s.name}
            </button>
          ))}
        </div>

        {/* Play/Pause */}
        <button
          onClick={() => {
            if (finished) {
              // Restart from beginning in Play All mode
              setFinished(false);
              setCurrentScene(0);
              setDataIndex(0);
              const d = SCENES[0].data[0];
              extPriceMV.set(d.extPrice);
              spotPriceMV.set(d.spotPrice);
              twapPriceMV.set(d.twapPrice);
              reserveZecMV.set(d.reserveZec);
              reserveZaiMV.set(d.reserveZai);
              arberCapMV.set(d.arberCapPct);
              arberDirMV.set(d.arberDirection);
              whalePnLMV.set(d.whalePnL);
              setIsPlaying(true);
            } else if (dataIndex >= sceneData.length - 1 && !playAllMode) {
              // Single mode: restart current scene
              setDataIndex(0);
              setIsPlaying(true);
            } else {
              setIsPlaying((p) => !p);
            }
          }}
          style={{
            background: C.BG,
            color: C.Text,
            border: `1px solid ${C.Muted}`,
            borderRadius: 6,
            padding: "6px 16px",
            fontFamily: "monospace",
            fontSize: 16,
            cursor: "pointer",
            minWidth: 44,
          }}
        >
          {isPlaying ? "\u23F8" : "\u25B6"}
        </button>

        {/* Speed selector */}
        <div style={{ display: "flex", gap: 3 }}>
          {[0.5, 1, 2, 5].map((s) => (
            <button
              key={s}
              onClick={() => setSpeed(s)}
              style={{
                background: speed === s ? C.ZEC : C.BG,
                color: C.Text,
                border: `1px solid ${speed === s ? C.ZEC : C.Muted}`,
                borderRadius: 4,
                padding: "4px 10px",
                fontFamily: "monospace",
                fontSize: 11,
                cursor: "pointer",
                fontWeight: speed === s ? "bold" : "normal",
              }}
            >
              {s}x
            </button>
          ))}
        </div>

        {/* Scrub bar */}
        <input
          type="range"
          min={0}
          max={sceneData.length - 1}
          value={dataIndex}
          onChange={handleScrub}
          style={{
            flex: "1 1 200px",
            maxWidth: 350,
            accentColor: C.ZEC,
            cursor: "pointer",
          }}
        />

        {/* Progress text */}
        <span style={{ color: C.Muted, fontFamily: "monospace", fontSize: 12, minWidth: 90, textAlign: "right" }}>
          {playAllMode
            ? `Scene ${currentScene + 1}/4 — Frame ${dataIndex + 1}/${sceneData.length}`
            : `Frame ${dataIndex + 1} / ${sceneData.length}`}
        </span>
      </div>
    </div>
  );
}

// ======= SECTION: MOUNT =======

const root = createRoot(document.getElementById("root"));
root.render(<App />);
