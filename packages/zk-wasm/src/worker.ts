/// <reference lib="webworker" />
/**
 * Web Worker entry point. Runs the (CPU-heavy, 1-2 s) Groth16 proof generation
 * off the main thread. Load it through `generateScoreProofAsync`, or directly:
 *
 *   new Worker(new URL("./worker.js", import.meta.url), { type: "module" })
 *
 * Jobs run one at a time in arrival order.
 */
import init, {
  compute_score_commitment,
  generate_score_proof,
  generate_tier_score_proof,
} from "../pkg-web/zk_wasm.js";
import type { ScoreProofInputs, WorkerRequest, WorkerResponse } from "./types.js";

const ctx = self as unknown as DedicatedWorkerGlobalScope;

// Load the WASM module once, on first use.
let ready: Promise<unknown> | undefined;
const ensureReady = (): Promise<unknown> => (ready ??= init());

function inputArgs(i: ScoreProofInputs) {
  return [
    i.vcPoints,
    i.txVolume30d,
    i.avgCounterparties,
    i.onTimeCount,
    i.totalCount,
    i.totalRepaid,
    i.vcWeight,
    i.txWeight,
    i.repaymentWeight,
    i.vcCount,
    i.lastUpdated,
    i.computedAtLedger,
    i.stale,
    i.blinding,
  ] as const;
}

async function run(req: WorkerRequest): Promise<Uint8Array> {
  await ensureReady();
  switch (req.kind) {
    case "score": {
      const p = req.payload;
      return generate_score_proof(
        ...inputArgs(p),
        p.threshold,
        p.subject,
        p.creditOracleId,
        p.snapshotLedger,
        p.domainSeparator,
      );
    }
    case "tier": {
      const p = req.payload;
      return generate_tier_score_proof(
        ...inputArgs(p),
        p.tierLevel,
        p.subject,
        p.creditOracleId,
        p.snapshotLedger,
        p.domainSeparator,
      );
    }
    case "commitment":
      return compute_score_commitment(...inputArgs(req.payload));
  }
}

function describe(e: unknown): string {
  return e instanceof Error ? e.message : typeof e === "string" ? e : String(e);
}

ctx.addEventListener("message", (ev: MessageEvent<WorkerRequest>) => {
  const req = ev.data;
  run(req).then(
    (bytes) => {
      const res: WorkerResponse = { id: req.id, ok: true, bytes };
      // Transfer the buffer instead of copying it.
      ctx.postMessage(res, [bytes.buffer]);
    },
    (e) => {
      const res: WorkerResponse = { id: req.id, ok: false, error: describe(e) };
      ctx.postMessage(res);
    },
  );
});
