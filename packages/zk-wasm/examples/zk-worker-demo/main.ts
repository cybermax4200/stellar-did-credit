import {
  computeScoreCommitmentAsync,
  generateScoreProofAsync,
  generateTierProofAsync,
  type ScoreProofInputs,
} from "../../dist/index.js";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const log = (msg: string) => ($("log").textContent = msg);
const hex = (b: Uint8Array, n = 16) =>
  [...b.slice(0, n)].map((x) => x.toString(16).padStart(2, "0")).join("") + (b.length > n ? "…" : "");

// --- main-thread liveness probe -------------------------------------------
let ticks = 0;
let last = performance.now();
let maxGap = 0;
setInterval(() => {
  const now = performance.now();
  maxGap = Math.max(maxGap, now - last);
  last = now;
  $("ticks").textContent = String(++ticks);
  $("gap").textContent = maxGap.toFixed(0);
}, 50);

// --- demo data ---------------------------------------------------------------
// In a real app these come from the credit-oracle (see the SDK's generateScoreProof).
const inputs: ScoreProofInputs = {
  vcPoints: 50,
  txVolume30d: 50_000_000_000,
  avgCounterparties: 10,
  onTimeCount: 50,
  totalCount: 100,
  totalRepaid: 20_000_000_000,
  vcWeight: 40,
  txWeight: 30,
  repaymentWeight: 30,
  vcCount: 10,
  lastUpdated: 1_700_000_000n,
  computedAtLedger: 1_234_567,
  stale: false,
  blinding: 42,
};
const bindings = {
  subject: new Uint8Array(32).fill(1),
  creditOracleId: new Uint8Array(32).fill(2),
  snapshotLedger: 12345,
  domainSeparator: new Uint8Array(32).fill(3),
};

async function timed(label: string, fn: () => Promise<Uint8Array>) {
  maxGap = 0;
  log(`${label}: generating…`);
  const t0 = performance.now();
  try {
    const out = await fn();
    const ms = (performance.now() - t0).toFixed(0);
    log(`${label}: ${out.length} bytes in ${ms} ms\n${hex(out, 24)}\nlongest main-thread gap: ${maxGap.toFixed(0)} ms`);
  } catch (e) {
    log(`${label} failed: ${e instanceof Error ? e.message : String(e)}`);
  }
}

$("threshold").addEventListener("click", () =>
  timed("score > 600 proof", () => generateScoreProofAsync({ ...inputs, ...bindings, threshold: 600 }, { timeoutMs: 120_000 })),
);
// Score 668 falls in tier 1 (Bronze, 600-699).
$("tier").addEventListener("click", () =>
  timed("tier 1 proof", () => generateTierProofAsync({ ...inputs, ...bindings, tierLevel: 1 }, { timeoutMs: 120_000 })),
);

computeScoreCommitmentAsync(inputs).then((c) => log(`Ready. Poseidon commitment: ${hex(c, 32)}`));
