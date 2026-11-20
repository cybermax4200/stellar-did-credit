/**
 * Main-thread API: generate zk proofs in a background Web Worker so the UI
 * thread is never blocked (Groth16 proving takes 1-2 s and would otherwise
 * freeze the DOM).
 *
 * ```ts
 * import { generateScoreProofAsync } from "@stellar-did-credit/zk-wasm/worker";
 * const proof = await generateScoreProofAsync(request); // Uint8Array, 384 bytes
 * ```
 */
import type {
  ProofBytes,
  ProofOptions,
  ScoreProofInputs,
  ScoreProofRequest,
  TierProofRequest,
  WorkerLike,
  WorkerRequest,
  WorkerResponse,
} from "./types.js";

export type * from "./types.js";

/** Size in bytes of a serialized proof. */
export const PROOF_BYTES = 384;
/** Size in bytes of a serialized commitment. */
export const COMMITMENT_BYTES = 32;

interface Pending {
  resolve: (bytes: Uint8Array) => void;
  reject: (err: Error) => void;
  cleanup: () => void;
}

class ProofWorkerClient {
  private worker: WorkerLike | undefined;
  private nextId = 1;
  private readonly pending = new Map<number, Pending>();

  /** The factory is only consulted when no worker is alive (first use, or after a crash/timeout/terminate). */
  private ensureWorker(factory: () => WorkerLike): WorkerLike {
    if (this.worker) return this.worker;
    const w = factory();
    w.addEventListener("message", (ev) => this.onMessage(ev.data));
    w.addEventListener("error", (ev) =>
      this.failAll(new Error(`proof worker error: ${ev.message ?? "unknown"}`)),
    );
    w.addEventListener("messageerror", () =>
      this.failAll(new Error("proof worker sent an unreadable message")),
    );
    this.worker = w;
    return w;
  }

  private onMessage(res: WorkerResponse): void {
    const p = this.pending.get(res.id);
    if (!p) return; // aborted or timed out: result is ignored
    this.pending.delete(res.id);
    p.cleanup();
    if (res.ok) p.resolve(res.bytes);
    else p.reject(new Error(res.error));
  }

  /** Reject every in-flight job and drop the worker so the next call starts fresh. */
  private failAll(err: Error): void {
    this.worker?.terminate();
    this.worker = undefined;
    const jobs = [...this.pending.values()];
    this.pending.clear();
    for (const p of jobs) {
      p.cleanup();
      p.reject(err);
    }
  }

  run(build: (id: number) => WorkerRequest, options: ProofOptions): Promise<Uint8Array> {
    const { timeoutMs, signal } = options;
    if (signal?.aborted) return Promise.reject(abortError(signal));

    return new Promise<Uint8Array>((resolve, reject) => {
      const id = this.nextId++;
      let timer: ReturnType<typeof setTimeout> | undefined;
      let onAbort: (() => void) | undefined;
      const cleanup = () => {
        if (timer !== undefined) clearTimeout(timer);
        if (onAbort) signal?.removeEventListener("abort", onAbort);
      };
      this.pending.set(id, { resolve, reject, cleanup });

      if (timeoutMs !== undefined) {
        timer = setTimeout(() => {
          if (!this.pending.has(id)) return;
          // A stuck WASM call cannot be interrupted: recycle the worker.
          this.failAll(new Error(`proof generation timed out after ${timeoutMs} ms`));
        }, timeoutMs);
      }
      if (signal) {
        onAbort = () => {
          const p = this.pending.get(id);
          if (!p) return;
          this.pending.delete(id);
          p.cleanup();
          p.reject(abortError(signal));
        };
        signal.addEventListener("abort", onAbort, { once: true });
      }

      try {
        this.ensureWorker(options.workerFactory ?? defaultWorkerFactory).postMessage(build(id));
      } catch (e) {
        this.pending.delete(id);
        cleanup();
        reject(e instanceof Error ? e : new Error(String(e)));
      }
    });
  }

  terminate(): void {
    this.failAll(new Error("proof worker terminated"));
  }
}

function abortError(signal: AbortSignal): Error {
  const reason = signal.reason;
  if (reason instanceof Error) return reason;
  const e = new Error("proof generation aborted");
  e.name = "AbortError";
  return e;
}

function defaultWorkerFactory(): WorkerLike {
  // The URL form is understood by Vite, webpack 5, esbuild and Rollup.
  return new Worker(new URL("./worker.js", import.meta.url), {
    type: "module",
  }) as unknown as WorkerLike;
}

let client: ProofWorkerClient | undefined;

function getClient(): ProofWorkerClient {
  return (client ??= new ProofWorkerClient());
}

// ---------------------------------------------------------------------------
// Validation (fail fast on the main thread; the WASM side validates too)
// ---------------------------------------------------------------------------

function assertU32(name: string, v: number): void {
  if (!Number.isInteger(v) || v < 0 || v > 0xffff_ffff) {
    throw new RangeError(`${name} must be an integer in [0, 2^32), got ${v}`);
  }
}

function assertSafeInt(name: string, v: number): void {
  if (!Number.isSafeInteger(v) || v < 0) {
    throw new RangeError(`${name} must be a non-negative safe integer, got ${v}`);
  }
}

function assertBytes32(name: string, v: Uint8Array): void {
  if (!(v instanceof Uint8Array) || v.length !== 32) {
    throw new TypeError(`${name} must be a Uint8Array of exactly 32 bytes`);
  }
}

function validateInputs(i: ScoreProofInputs): void {
  for (const k of [
    "vcPoints", "avgCounterparties", "onTimeCount", "totalCount", "vcWeight",
    "txWeight", "repaymentWeight", "vcCount", "computedAtLedger", "blinding",
  ] as const) {
    assertU32(k, i[k]);
  }
  assertSafeInt("txVolume30d", i.txVolume30d);
  assertSafeInt("totalRepaid", i.totalRepaid);
  if (typeof i.lastUpdated !== "bigint" || i.lastUpdated < 0n || i.lastUpdated >= 1n << 64n) {
    throw new RangeError("lastUpdated must be a bigint in [0, 2^64)");
  }
  if (i.vcWeight + i.txWeight + i.repaymentWeight !== 100) {
    throw new RangeError("vcWeight + txWeight + repaymentWeight must equal 100");
  }
}

function validateBindings(r: ScoreProofRequest | TierProofRequest): void {
  assertBytes32("subject", r.subject);
  assertBytes32("creditOracleId", r.creditOracleId);
  assertBytes32("domainSeparator", r.domainSeparator);
  assertU32("snapshotLedger", r.snapshotLedger);
}

/** Run a validation synchronously but surface failures as a rejected promise. */
function guarded<T>(validate: () => void, run: () => Promise<T>): Promise<T> {
  try {
    validate();
  } catch (e) {
    return Promise.reject(e);
  }
  return run();
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/**
 * Generate a Groth16 proof that the subject's score is `> threshold`, in a
 * background Web Worker. Resolves with the 384-byte proof (`A || B || C`).
 */
export function generateScoreProofAsync(
  request: ScoreProofRequest,
  options: ProofOptions = {},
): Promise<ProofBytes> {
  return guarded(
    () => {
      validateInputs(request);
      validateBindings(request);
      assertU32("threshold", request.threshold);
    },
    () => getClient().run((id) => ({ id, kind: "score", payload: request }), options),
  );
}

/**
 * Generate a Groth16 proof that the subject's score lies in tier
 * `request.tierLevel` (1 Bronze >= 600, 2 Silver >= 700, 3 Gold >= 800).
 */
export function generateTierProofAsync(
  request: TierProofRequest,
  options: ProofOptions = {},
): Promise<ProofBytes> {
  return guarded(
    () => {
      validateInputs(request);
      validateBindings(request);
      if (![1, 2, 3].includes(request.tierLevel)) {
        throw new RangeError(`tierLevel must be 1, 2 or 3, got ${request.tierLevel}`);
      }
    },
    () => getClient().run((id) => ({ id, kind: "tier", payload: request }), options),
  );
}

/**
 * The Poseidon `score_commitment` (32 bytes, big-endian) for the given inputs;
 * the exact value the circuit checks.
 */
export function computeScoreCommitmentAsync(
  inputs: ScoreProofInputs,
  options: ProofOptions = {},
): Promise<Uint8Array> {
  return guarded(
    () => validateInputs(inputs),
    () => getClient().run((id) => ({ id, kind: "commitment", payload: inputs }), options),
  );
}

/** Terminate the shared worker (rejects in-flight jobs). A new one is created on demand. */
export function terminateProofWorker(): void {
  client?.terminate();
  client = undefined;
}
