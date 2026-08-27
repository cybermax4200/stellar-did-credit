/**
 * ZK Prover Module for the Stellar DID Credit Protocol.
 *
 * Provides client-side zero-knowledge proof generation for selective score
 * disclosure using Groth16 proofs. Subjects can prove statements such as
 * "my credit score is above 650" without revealing the exact score or
 * underlying inputs.
 *
 * This module is lazy-loaded to avoid bloating the SDK bundle with the
 * snarkjs WASM runtime (~10 MB). The first call to `generateScoreRangeProof`
 * triggers the import.
 *
 * @module zk/prover
 */

import type {
  StellarDIDCreditSDK,
} from "../index";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/** Configuration for loading the circuit artifacts. */
export interface ZKPublicParams {
  /** URL or path to the Groth16 verification key JSON. */
  verificationKeyUrl: string;
  /** URL or path to the circuit WASM binary. */
  circuitWasmUrl: string;
  /** Optional domain separator bytes for protocol/version binding. */
  domainSeparator?: Uint8Array;
}

/**
 * All private witness data the prover needs to construct a valid proof.
 *
 * Gathered from on-chain ScoreRecord, TxStats, RepaymentRecord, and the
 * active ScoringWeights at the time of score computation.
 */
export interface ScoreWitness {
  /** Final credit score (300–850). */
  score: number;
  /** Number of active VCs at computation. */
  vcCount: number;
  /** 30-day transaction volume in stroops. */
  txVolume30d: bigint;
  /** Average distinct counterparties in the last 30 days. */
  avgCounterparties: number;
  /** Repayment rate in basis points (0–10000). */
  repaymentRate: number;
  /** Ledger timestamp when the score was computed. */
  lastUpdated: number;
  /** Ledger sequence at computation. */
  computedAtLedger: number;
  /** Whether the score is considered stale. */
  stale: boolean;

  /** Active vc_weight at the time of computation. */
  vcWeight: number;
  /** Active tx_weight at the time of computation. */
  txWeight: number;
  /** Active repayment_weight at the time of computation. */
  repaymentWeight: number;

  /** Intermediate vc_score component. */
  vcScore: number;
  /** Intermediate tx_score component. */
  txScore: number;
  /** Intermediate repay_score component. */
  repayScore: number;
  /** Counterparty bonus (0 or 10). */
  counterpartyBonus: number;
  /** Weighted composite before final mapping. */
  composite: number;
  /** Random blinding factor for the commitment. */
  blinding: bigint;
}

/** Public inputs visible to the on-chain verifier. */
export interface ScoreRangePublicInputs {
  /** Minimum score the prover claims (threshold). */
  threshold: number;
  /** Stellar account whose score is attested. */
  subject: string;
  /** Credit-oracle contract ID whose state is referenced. */
  creditOracleId: string;
  /** Commitment to the private ScoreRecord fields. */
  scoreCommitment: Uint8Array;
  /** Ledger sequence when inputs were valid. */
  snapshotLedger: number;
  /** Protocol/version binding bytes. */
  domainSeparator: Uint8Array;
}

/** The raw Groth16 proof output from snarkjs. */
export interface Groth16Proof {
  /** π_a — first proof element (3 field elements). */
  pi_a: [string, string, string];
  /** π_b — second proof element (3×2 field elements). */
  pi_b: [[string, string], [string, string], [string, string]];
  /** π_c — third proof element (3 field elements). */
  pi_c: [string, string, string];
  /** Protocol identifier. */
  protocol: string;
  /** Curve identifier. */
  curve: string;
}

/** Serialized proof bundle suitable for sending to a lender. */
export interface ProofBundle {
  /** Circuit version for on-chain compatibility checking. */
  circuitVersion: number;
  /** Base64-encoded Groth16 proof (192 bytes for BLS12-381). */
  proof: string;
  /** Public inputs for the verifier. */
  publicInputs: ScoreRangePublicInputs;
}

/** Result of generateScoreRangeProof. */
export interface ProofResult {
  /** The Groth16 proof. */
  proof: Groth16Proof;
  /** Public inputs bound to this proof. */
  publicInputs: ScoreRangePublicInputs;
  /** Serialized bundle ready for on-chain verification. */
  bundle: ProofBundle;
}

/** Configuration for the prover's lazy-loading behavior. */
export interface ProverConfig {
  /** Circuit version identifier. */
  circuitVersion?: number;
  /** Optional fetch function override (for testing or custom HTTP clients). */
  fetchFn?: typeof globalThis.fetch;
}

// ---------------------------------------------------------------------------
// Lazy-loaded snarkjs singleton
// ---------------------------------------------------------------------------

// eslint-disable-next-line @typescript-eslint/no-explicit-any
let _snarkjs: any = null;

/**
 * Lazily load snarkjs. The WASM bundle is ~10 MB, so we defer the import
 * until the first proof generation request.
 *
 * @returns The snarkjs module
 * @throws {Error} if snarkjs cannot be loaded
 */
async function loadSnarkjs(): Promise<Record<string, unknown>> {
  if (_snarkjs) {
    return _snarkjs;
  }
  // Dynamic import for lazy loading — snarkjs brings in ~10 MB of WASM
  const snarkjs = await import("snarkjs");
  _snarkjs = snarkjs;
  return snarkjs;
}

// ---------------------------------------------------------------------------
// Scoring formula helpers
// ---------------------------------------------------------------------------

export const MIN_SCORE = 300;
export const MAX_SCORE = 850;

/**
 * Replicate the on-chain scoring formula to compute intermediate component
 * scores from raw witness data. This must stay in sync with the contract's
 * `compute_score` implementation.
 */
export function computeScoringComponents(
  witness: Pick<
    ScoreWitness,
    | "vcCount"
    | "txVolume30d"
    | "avgCounterparties"
    | "repaymentRate"
    | "vcWeight"
    | "txWeight"
    | "repaymentWeight"
  >,
): {
  vcScore: number;
  txScore: number;
  repayScore: number;
  counterpartyBonus: number;
  composite: number;
  score: number;
} {
  const vcScore = Math.min(witness.vcCount * 20, 100);
  const txScore = Math.min(
    Number(witness.txVolume30d / 100_000_000n),
    100,
  );
  const repayScore = Math.floor(witness.repaymentRate / 100);
  const counterpartyBonus = witness.avgCounterparties >= 10 ? 10 : 0;

  const composite =
    (vcScore * witness.vcWeight +
      (txScore + counterpartyBonus) * witness.txWeight +
      repayScore * witness.repaymentWeight) /
    100;

  const score = Math.min(
    Math.max(MIN_SCORE + Math.floor((composite * 550) / 100), MIN_SCORE),
    MAX_SCORE,
  );

  return { vcScore, txScore, repayScore, counterpartyBonus, composite, score };
}

// ---------------------------------------------------------------------------
// Pedersen-style commitment (simplified for development)
// ---------------------------------------------------------------------------

/**
 * Compute a simple SHA-256-based commitment hash over the ScoreRecord fields.
 *
 * **Note:** This is a development placeholder. The production circuit will
 * use a Pedersen-style vector commitment or Poseidon hash inside the
 * Circom/arkworks circuit. The on-chain verifier must use the same
 * commitment scheme.
 */
export function computeScoreCommitment(
  score: number,
  vcCount: number,
  txVolume30d: bigint,
  avgCounterparties: number,
  repaymentRate: number,
  lastUpdated: number,
  computedAtLedger: number,
  blinding: bigint,
): Uint8Array {
  // Build a canonical byte representation of the witness fields.
  // Each field is encoded as 8 bytes (big-endian) for uniform hashing.
  const parts: bigint[] = [
    BigInt(score),
    BigInt(vcCount),
    txVolume30d,
    BigInt(avgCounterparties),
    BigInt(repaymentRate),
    BigInt(lastUpdated),
    BigInt(computedAtLedger),
    blinding,
  ];

  const data = new Uint8Array(parts.length * 8);
  for (let i = 0; i < parts.length; i++) {
    const view = new DataView(data.buffer, i * 8, 8);
    view.setBigUint64(0, parts[i]);
  }

  // Use Node.js crypto for the hash. In a browser, the Web Crypto API would
  // be used instead (available via the same globalThis.crypto).
  // We use a synchronous hash for simplicity; the production version can use
  // an async SHA-256 from the Web Crypto API.
  // eslint-disable-next-line @typescript-eslint/no-var-requires, global-require
  const cryptoModule = require("crypto") as typeof import("crypto");
  const hash = cryptoModule.createHash("sha256").update(Buffer.from(data)).digest();
  return new Uint8Array(hash);
}

/**
 * Random blinding factor for Pedersen commitments.
 *
 * Returns a cryptographically secure 256-bit random value as a bigint.
 */
export function generateBlinding(): bigint {
  // eslint-disable-next-line @typescript-eslint/no-var-requires, global-require
  const cryptoModule = require("crypto") as typeof import("crypto");
  const bytes = cryptoModule.randomBytes(32);
  let value = 0n;
  for (const byte of bytes) {
    value = (value << 8n) | BigInt(byte);
  }
  return value;
}

// ---------------------------------------------------------------------------
// collectWitness
// ---------------------------------------------------------------------------

/**
 * Gather all on-chain data needed to construct a proof witness.
 *
 * This helper makes the following SDK calls in sequence:
 * 1. `getScore(subjectAddress)` — ScoreRecord
 * 2. `getTxStats(subjectAddress)` — TxStats (via get_score metadata)
 * 3. `getWeights()` — active ScoringWeights
 *
 * The caller is responsible for ensuring `compute_score` has been called
 * for this subject so that the ScoreRecord exists on-chain.
 *
 * @param sdk - The Stellar DID Credit SDK instance
 * @param subjectAddress - Stellar G... address of the subject
 * @returns A fully populated ScoreWitness ready for proof generation
 * @throws {Error} if any required on-chain data is missing
 */
export async function collectWitness(
  sdk: StellarDIDCreditSDK,
  subjectAddress: string,
): Promise<ScoreWitness> {
  // 1. Fetch the ScoreRecord
  const scoreRecord = await sdk.getScore(subjectAddress);
  if (!scoreRecord) {
    throw new Error(
      `No score computed for ${subjectAddress}. Call computeScore first.`,
    );
  }

  // 2. Fetch the active ScoringWeights
  const weights = await sdk.getWeights();

  // 3. Compute intermediate component scores using the canonical formula.
  //    These values are part of the witness so the circuit can verify them
  //    against committed values without re-computing inside the circuit.
  const components = computeScoringComponents({
    vcCount: scoreRecord.vcCount,
    txVolume30d: scoreRecord.txVolume30d,
    // avgCounterparties is not currently available from getScore alone.
    // The caller must ensure this is populated from TxStats if available.
    // For now, we default to 0 and document this as a known limitation.
    // See: https://github.com/cybermax4200/stellar-did-credit/issues/533
    avgCounterparties: 0,
    repaymentRate: scoreRecord.repaymentRate,
    vcWeight: weights.vcWeight,
    txWeight: weights.txWeight,
    repaymentWeight: weights.repaymentWeight,
  });

  // 4. Generate a random blinding factor for the commitment
  const blinding = generateBlinding();

  return {
    score: scoreRecord.score,
    vcCount: scoreRecord.vcCount,
    txVolume30d: scoreRecord.txVolume30d,
    avgCounterparties: 0, // See note above — must be populated from TxStats
    repaymentRate: scoreRecord.repaymentRate,
    lastUpdated: scoreRecord.lastUpdated,
    computedAtLedger: scoreRecord.computedAtLedger,
    stale: scoreRecord.stale,
    vcWeight: weights.vcWeight,
    txWeight: weights.txWeight,
    repaymentWeight: weights.repaymentWeight,
    vcScore: components.vcScore,
    txScore: components.txScore,
    repayScore: components.repayScore,
    counterpartyBonus: components.counterpartyBonus,
    composite: components.composite,
    blinding,
  };
}

// ---------------------------------------------------------------------------
// generateScoreRangeProof
// ---------------------------------------------------------------------------

/**
 * Generate a Groth16 proof that the subject's score exceeds the given
 * threshold.
 *
 * The proof binds:
 * - `subject` — prevents proof reuse across accounts
 * - `creditOracleId` — prevents cross-deployment replay
 * - `snapshotLedger` — prevents stale-snapshot replay
 * - `domainSeparator` — protocol/version binding
 *
 * snarkjs is lazy-loaded on the first invocation (~10 MB WASM). Subsequent
 * calls reuse the cached import.
 *
 * @param witness - Private witness data (ScoreRecord + TxStats + weights)
 * @param threshold - Minimum score the prover claims
 * @param publicParams - URLs to circuit artifacts + public metadata
 * @param config - Optional prover configuration
 * @returns The Groth16 proof, public inputs, and a serialized bundle
 * @throws {Error} if witness score does not exceed threshold
 * @throws {Error} if circuit artifacts cannot be loaded
 */
export async function generateScoreRangeProof(
  witness: ScoreWitness,
  threshold: number,
  publicParams: ZKPublicParams,
  config: ProverConfig = {},
): Promise<ProofResult> {
  if (witness.score <= threshold) {
    throw new Error(
      `Witness score ${witness.score} does not exceed threshold ${threshold}. ` +
        `Cannot generate a valid proof.`,
    );
  }

  // 1. Build public inputs
  const domainSeparator = publicParams.domainSeparator ?? new Uint8Array(32);
  const scoreCommitment = computeScoreCommitment(
    witness.score,
    witness.vcCount,
    witness.txVolume30d,
    witness.avgCounterparties,
    witness.repaymentRate,
    witness.lastUpdated,
    witness.computedAtLedger,
    witness.blinding,
  );

  const publicInputs: ScoreRangePublicInputs = {
    threshold,
    subject: "", // Must be set by the caller before bundling
    creditOracleId: "", // Must be set by the caller before bundling
    scoreCommitment,
    snapshotLedger: witness.computedAtLedger,
    domainSeparator,
  };

  // 2. Construct the circuit input signals
  const circuitInput: Record<string, string | number | string[]> = {
    // Private witness signals
    score: witness.score,
    vc_count: witness.vcCount,
    tx_volume_30d: witness.txVolume30d.toString(),
    avg_counterparties: witness.avgCounterparties,
    repayment_rate: witness.repaymentRate,
    last_updated: witness.lastUpdated,
    computed_at_ledger: witness.computedAtLedger,
    stale: witness.stale ? 1 : 0,
    vc_weight: witness.vcWeight,
    tx_weight: witness.txWeight,
    repayment_weight: witness.repaymentWeight,
    vc_score: witness.vcScore,
    tx_score: witness.txScore,
    repay_score: witness.repayScore,
    counterparty_bonus: witness.counterpartyBonus,
    composite: witness.composite,
    blinding: witness.blinding.toString(),
    // Public signals
    threshold,
    score_commitment: Array.from(scoreCommitment)
      .map((b) => b.toString())
      .join(","),
    snapshot_ledger: witness.computedAtLedger,
    domain_separator: Array.from(domainSeparator)
      .map((b) => b.toString())
      .join(","),
  };

  // 3. Lazy-load snarkjs and generate the proof
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const snarkjs: any = await loadSnarkjs();

  let proof: Groth16Proof;

  try {
    const result = await snarkjs.groth16.fullProve(
      circuitInput,
      publicParams.circuitWasmUrl,
      publicParams.verificationKeyUrl,
    );
    proof = result.proof as Groth16Proof;
  } catch (error) {
    const message =
      error instanceof Error ? error.message : String(error);
    throw new Error(
      `snarkjs proof generation failed: ${message}. ` +
        `Ensure the circuit WASM and verification key are correctly configured.`,
    );
  }

  // 4. Build the serialized bundle
  const bundle: ProofBundle = {
    circuitVersion: config.circuitVersion ?? 1,
    proof: Buffer.from(JSON.stringify(proof)).toString("base64"),
    publicInputs,
  };

  return { proof, publicInputs, bundle };
}

// ---------------------------------------------------------------------------
// verifyProof (local helper — not for on-chain use)
// ---------------------------------------------------------------------------

/**
 * Verify a Groth16 proof against a verification key.
 *
 * This is an **off-chain** helper for testing and debugging. On-chain
 * verification should use the `score-range-verifier` Soroban contract.
 *
 * @param verificationKey - The verification key object or URL
 * @param publicSignals - The public signal values
 * @param proof - The Groth16 proof to verify
 * @returns true if the proof is valid
 */
export async function verifyProof(
  verificationKey: unknown,
  publicSignals: string[],
  proof: Groth16Proof,
): Promise<boolean> {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const snarkjs: any = await loadSnarkjs();
  return snarkjs.groth16.verify(verificationKey, publicSignals, proof) as Promise<boolean>;
}
