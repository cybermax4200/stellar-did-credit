/**
 * Public and wire types for the zk-wasm proof API.
 *
 * Everything the worker and the main thread exchange is defined here so both
 * sides (and consumers) share one source of truth. All fields are structured-
 * cloneable (numbers, bigints, booleans, Uint8Array).
 */

/** Raw scoring inputs plus the committed `ScoreRecord` metadata. */
export interface ScoreProofInputs {
  /** VC points (capped at 100 by the circuit). */
  vcPoints: number;
  /** 30-day transaction volume in stroops. Must be a safe integer (JS number). */
  txVolume30d: number;
  avgCounterparties: number;
  onTimeCount: number;
  totalCount: number;
  /** Total repaid in stroops. Must be a safe integer (JS number). */
  totalRepaid: number;
  /** Active weights; must sum to 100. */
  vcWeight: number;
  txWeight: number;
  repaymentWeight: number;
  /** Committed ScoreRecord fields. */
  vcCount: number;
  /** Ledger timestamp (u64). */
  lastUpdated: bigint;
  computedAtLedger: number;
  stale: boolean;
  /** Commitment blinding factor (u32). */
  blinding: number;
}

/** Public inputs shared by every proof statement. Byte fields are 32 bytes. */
export interface ProofBindings {
  /** SHA-256 of the subject address XDR (32 bytes). */
  subject: Uint8Array;
  /** SHA-256 of the credit-oracle address XDR (32 bytes). */
  creditOracleId: Uint8Array;
  snapshotLedger: number;
  /** Protocol/domain separator (32 bytes). */
  domainSeparator: Uint8Array;
}

/** Request for a `score > threshold` proof. */
export interface ScoreProofRequest extends ScoreProofInputs, ProofBindings {
  threshold: number;
}

/** Tier levels: 1 = Bronze (>= 600), 2 = Silver (>= 700), 3 = Gold (>= 800). */
export type TierLevel = 1 | 2 | 3;

/** Request for a tier-band proof. */
export interface TierProofRequest extends ScoreProofInputs, ProofBindings {
  tierLevel: TierLevel;
}

/** A Groth16 proof: `A || B || C`, arkworks uncompressed (384 bytes). */
export type ProofBytes = Uint8Array;

/** The kinds of job the worker can run. */
export type JobKind = "score" | "tier" | "commitment";

/** Main thread -> worker. */
export type WorkerRequest =
  | { id: number; kind: "score"; payload: ScoreProofRequest }
  | { id: number; kind: "tier"; payload: TierProofRequest }
  | { id: number; kind: "commitment"; payload: ScoreProofInputs };

/** Worker -> main thread. */
export type WorkerResponse =
  | { id: number; ok: true; bytes: Uint8Array }
  | { id: number; ok: false; error: string };

/** Options for the async API. */
export interface ProofOptions {
  /** Reject (and recycle the worker) if no result arrives within this time. */
  timeoutMs?: number;
  /** Reject early when aborted. The worker finishes the job in the background. */
  signal?: AbortSignal;
  /**
   * Create the worker yourself (custom bundler URL, shared worker pool, test
   * double). Only consulted when no worker exists yet.
   */
  workerFactory?: () => WorkerLike;
}

/** The subset of `Worker` this library uses; lets tests inject a fake. */
export interface WorkerLike {
  postMessage(message: WorkerRequest): void;
  terminate(): void;
  addEventListener(type: "message", listener: (ev: { data: WorkerResponse }) => void): void;
  addEventListener(type: "error", listener: (ev: { message?: string }) => void): void;
  addEventListener(type: "messageerror", listener: (ev: unknown) => void): void;
}
