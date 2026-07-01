/**
 * Feeder reference implementation for the stellar-did-credit protocol.
 *
 * Each polling cycle the feeder:
 *   1. Reads get_active_vc_count(subject) from the identity-oracle.
 *   2. Queries the Horizon API for 30-day payment statistics for each subject.
 *   3. Submits set_vc_count(feeder, subject, count) to the credit-oracle.
 *   4. Submits update_tx_stats(feeder, subject, stats) to the credit-oracle.
 *
 * Usage (CLI):
 *   FEEDER_SECRET=YOUR_STELLAR_SECRET_KEY SUBJECTS=G1...,G2... \
 *   CREDIT_ORACLE_ID=C... IDENTITY_ORACLE_ID=C... \
 *   npm start
 *
 * Required environment variables:
 *   FEEDER_SECRET        — Stellar secret key of a registered feeder (starts with S)
 *   SUBJECTS             — Comma-separated list of subject G... addresses
 *   CREDIT_ORACLE_ID     — Contract address of the credit-oracle
 *   IDENTITY_ORACLE_ID   — Contract address of the identity-oracle
 *
 * Optional environment variables:
 *   NETWORK_PASSPHRASE   — Defaults to Stellar testnet passphrase
 *   RPC_URL              — Defaults to the public Soroban testnet RPC
 *   HORIZON_URL          — Defaults to the public Horizon testnet
 *   SIM_ACCOUNT          — Any funded account used as fee source for read-only sims
 *   POLL_INTERVAL_MS     — Feed cycle interval in ms (default: 3 600 000 = 1 hour)
 *   MAX_RETRIES          — Max retry attempts for transient RPC/Horizon failures (default: 3)
 *   RETRY_BASE_DELAY_MS  — Base backoff delay in ms (default: 1 000)
 */

import {
  Contract,
  SorobanRpc,
  TransactionBuilder,
  BASE_FEE,
  Account,
  scValToNative,
  nativeToScVal,
  Address,
  xdr,
  Keypair,
  Horizon,
} from "@stellar/stellar-sdk";
import { assembleTransaction } from "@stellar/stellar-sdk/rpc";

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

export interface FeederConfig {
  /** Soroban RPC URL */
  rpcUrl: string;
  /** Horizon REST API URL */
  horizonUrl: string;
  /** Stellar network passphrase */
  networkPassphrase: string;
  /** credit-oracle contract address */
  creditOracleId: string;
  /** identity-oracle contract address */
  identityOracleId: string;
  /** Any funded account used as fee source for read-only simulations */
  simAccount: string;
  /** Subject G... addresses to sync on every cycle */
  subjects: string[];
  /** How often to run a full feed cycle, in milliseconds */
  pollIntervalMs: number;
  /** Max retry attempts for transient RPC/Horizon failures */
  maxRetries?: number;
  /** Base delay for exponential backoff, in milliseconds */
  retryBaseDelayMs?: number;
}

/** Transaction statistics to be written to the credit-oracle via update_tx_stats. */
export interface TxStats {
  /** Total XLM payment volume over the last 30 days, in stroops (1 XLM = 10 000 000 stroops). */
  volume30d: bigint;
  /** Number of distinct payment transactions in the last 30 days. */
  txCount30d: number;
  /** Average number of distinct counterparties per transaction. */
  avgCounterparties: number;
}

// ---------------------------------------------------------------------------
// Horizon helpers
// ---------------------------------------------------------------------------

/**
 * Fetches 30-day payment statistics for a Stellar address via the Horizon API.
 *
 * Paginates backwards through the payment operation history, stopping at the
 * 30-day cutoff. Only native (XLM) payment amounts are included in the volume
 * total; non-native assets are counted toward tx_count and counterparties but
 * not volume, matching the credit-oracle's scoring semantics.
 */
export async function fetchHorizonStats(
  horizonUrl: string,
  address: string,
): Promise<TxStats> {
  const horizon = new Horizon.Server(horizonUrl);
  const cutoff = new Date(Date.now() - 30 * 24 * 60 * 60 * 1000);

  let volumeStroops = BigInt(0);
  const txHashes = new Set<string>();
  // Map from transaction hash → set of distinct counterparty addresses
  const counterpartiesPerTx = new Map<string, Set<string>>();

  let page = await horizon
    .payments()
    .forAccount(address)
    .order("desc")
    .limit(200)
    .call();

  outer: while (page.records.length > 0) {
    for (const record of page.records) {
      const op = record as Horizon.ServerApi.PaymentOperationRecord &
        Horizon.ServerApi.CreateAccountOperationRecord & {
          transaction_hash: string;
          created_at: string;
          from?: string;
          to?: string;
          amount?: string;
          asset_type?: string;
        };

      if (new Date(op.created_at) < cutoff) {
        break outer;
      }

      const txHash = op.transaction_hash;
      txHashes.add(txHash);

      if (!counterpartiesPerTx.has(txHash)) {
        counterpartiesPerTx.set(txHash, new Set());
      }

      if (op.type === "payment") {
        // Accumulate XLM volume in stroops
        if (op.asset_type === "native" && op.amount) {
          const amountXLM = parseFloat(op.amount);
          volumeStroops += BigInt(Math.round(amountXLM * 10_000_000));
        }

        // Track the other party in this payment
        const counterparty = op.from === address ? op.to : op.from;
        if (counterparty) {
          counterpartiesPerTx.get(txHash)!.add(counterparty);
        }
      }
    }

    page = await page.next();
  }

  const txCount30d = txHashes.size;

  let totalCounterparties = 0;
  for (const cps of counterpartiesPerTx.values()) {
    totalCounterparties += cps.size;
  }
  const avgCounterparties =
    txCount30d > 0 ? Math.round(totalCounterparties / txCount30d) : 0;

  return { volume30d: volumeStroops, txCount30d, avgCounterparties };
}

/** Extract the sequence number string from a Soroban RPC account response. */
function getSequence(accountData: SorobanRpc.Api.GetAccountResponse): string {
  const data = accountData as unknown as { sequence: string };
  return data.sequence;
}

/**
 * Reads the active (non-revoked) VC count from the identity-oracle.
 * Uses a read-only simulation — no signing or fees required.
 */
export async function getActiveVcCount(
  server: SorobanRpc.Server,
  config: Pick<
    FeederConfig,
    "identityOracleId" | "networkPassphrase" | "simAccount"
  >,
  subjectAddress: string,
): Promise<number> {
  const contract = new Contract(config.identityOracleId);
  const sourceAccount = new Account(config.simAccount, "0");

  const tx = new TransactionBuilder(sourceAccount, {
    fee: BASE_FEE,
    networkPassphrase: config.networkPassphrase,
  })
    .addOperation(
      contract.call(
        "get_active_vc_count",
        new Address(subjectAddress).toScVal(),
      ),
    )
    .setTimeout(30)
    .build();

  const sim = await server.simulateTransaction(tx);

  if (SorobanRpc.Api.isSimulationError(sim)) {
    throw new Error(`get_active_vc_count simulation failed: ${sim.error}`);
  }
  if (!SorobanRpc.Api.isSimulationSuccess(sim)) {
    throw new Error("Unexpected simulation response for get_active_vc_count");
  }

  return Number(scValToNative(sim.result!.retval));
}

/**
 * Encodes a TxStats object as a Soroban ScVal struct (ScMap).
 * Keys are alphabetically sorted as required by the Soroban XDR encoding.
 */
function txStatsToScVal(stats: TxStats): xdr.ScVal {
  return xdr.ScVal.scvMap([
    new xdr.ScMapEntry({
      key: xdr.ScVal.scvSymbol("avg_counterparties"),
      val: nativeToScVal(stats.avgCounterparties, { type: "u32" }),
    }),
    new xdr.ScMapEntry({
      key: xdr.ScVal.scvSymbol("tx_count_30d"),
      val: nativeToScVal(stats.txCount30d, { type: "u32" }),
    }),
    new xdr.ScMapEntry({
      key: xdr.ScVal.scvSymbol("volume_30d"),
      val: nativeToScVal(stats.volume30d, { type: "i128" }),
    }),
  ]);
}

/**
 * Simulates, assembles, signs, and submits a single contract operation.
 * Returns the transaction hash once the network accepts the submission.
 */
async function submitOperation(
  server: SorobanRpc.Server,
  networkPassphrase: string,
  feederKeypair: Keypair,
  operation: xdr.Operation,
): Promise<string> {
  const accountData = await server.getAccount(feederKeypair.publicKey());
  const sourceAccount = new Account(
    feederKeypair.publicKey(),
    getSequence(accountData),
  );

  const tx = new TransactionBuilder(sourceAccount, {
    fee: BASE_FEE,
    networkPassphrase,
  })
    .addOperation(operation)
    .setTimeout(30)
    .build();

  const sim = await server.simulateTransaction(tx);

  if (SorobanRpc.Api.isSimulationError(sim)) {
    throw new Error(`Simulation failed: ${sim.error}`);
  }
  if (!SorobanRpc.Api.isSimulationSuccess(sim)) {
    throw new Error("Unexpected simulation response");
  }

  const preparedTx = assembleTransaction(tx, sim).build();
  preparedTx.sign(feederKeypair);

  const response = await server.sendTransaction(preparedTx);
  if (response.status !== "PENDING") {
    throw new Error(
      `Transaction rejected: ${JSON.stringify(response.errorResult)}`,
    );
  }

  return response.hash;
}

/**
 * Polls the RPC until a transaction reaches a terminal state.
 * Throws if the transaction fails or is not confirmed within timeoutMs.
 */
export async function waitForConfirmation(
  server: SorobanRpc.Server,
  txHash: string,
  timeoutMs = 60_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;

  while (Date.now() < deadline) {
    await sleep(3_000);
    const status = await server.getTransaction(txHash);

    if (status.status === "SUCCESS") return;
    if (status.status === "FAILED") {
      throw new Error(`Transaction ${txHash} failed on-chain`);
    }
    // "NOT_FOUND" means still in-flight — keep polling
  }

  throw new Error(`Transaction ${txHash} not confirmed within ${timeoutMs}ms`);
}

// ---------------------------------------------------------------------------
// Feeder class
// ---------------------------------------------------------------------------

/**
 * Reference feeder that syncs VC counts and Horizon transaction statistics
 * into the credit-oracle on a configurable polling interval.
 *
 * Prerequisites:
 *   - The feeder keypair must be registered on-chain via register_feeder(admin, feeder).
 *   - The feeder account must be funded with enough XLM to pay transaction fees.
 *
 * @example
 * ```typescript
 * import { Feeder, FeederConfig } from "@stellar-did-credit/feeder";
 * import { Keypair } from "@stellar/stellar-sdk";
 *
 * const config: FeederConfig = {
 *   rpcUrl: "https://soroban-testnet.stellar.org",
 *   horizonUrl: "https://horizon-testnet.stellar.org",
 *   networkPassphrase: "Test SDF Network ; September 2015",
 *   creditOracleId: "C...",
 *   identityOracleId: "C...",
 *   simAccount: "G...",
 *   subjects: ["GSUBJECT1...", "GSUBJECT2..."],
 *   pollIntervalMs: 3_600_000,
 * };
 *
 * const feeder = new Feeder(config, Keypair.fromSecret("YOUR_STELLAR_SECRET_KEY"));
 * const stop = feeder.start();   // begins polling; call stop() to halt
 * ```
 */
export class Feeder {
  private server: SorobanRpc.Server;

  constructor(
    private config: FeederConfig,
    private feederKeypair: Keypair,
  ) {
    this.server = new SorobanRpc.Server(config.rpcUrl);
  }

  /**
   * Syncs a single subject: fetches stats, then submits set_vc_count followed
   * by update_tx_stats, waiting for each transaction to be confirmed.
   */
  async feedSubject(subjectAddress: string): Promise<void> {
    console.log(`[feeder] syncing ${subjectAddress}`);

    const maxRetries = this.config.maxRetries ?? 3;
    const retryBaseDelayMs = this.config.retryBaseDelayMs ?? 1_000;

    // Step 1: read active VC count from identity-oracle
    const vcCount = await withExponentialBackoff(
      `get_active_vc_count(${subjectAddress})`,
      maxRetries,
      retryBaseDelayMs,
      () => getActiveVcCount(this.server, this.config, subjectAddress),
    );
    console.log(`  vc_count          = ${vcCount}`);

    // Step 2: fetch 30-day payment stats from Horizon
    const stats = await withExponentialBackoff(
      `fetch_horizon_stats(${subjectAddress})`,
      maxRetries,
      retryBaseDelayMs,
      () => fetchHorizonStats(this.config.horizonUrl, subjectAddress),
    );
    console.log(
      `  volume_30d        = ${stats.volume30d} stroops` +
        ` (${Number(stats.volume30d) / 10_000_000} XLM)`,
    );
    console.log(`  tx_count_30d      = ${stats.txCount30d}`);
    console.log(`  avg_counterparties = ${stats.avgCounterparties}`);

    const creditContract = new Contract(this.config.creditOracleId);
    const feederAddress = this.feederKeypair.publicKey();

    // Step 3: submit set_vc_count
    const vcCountTxHash = await withExponentialBackoff(
      `set_vc_count(${subjectAddress})`,
      maxRetries,
      retryBaseDelayMs,
      () =>
        submitOperation(
          this.server,
          this.config.networkPassphrase,
          this.feederKeypair,
          creditContract.call(
            "set_vc_count",
            new Address(feederAddress).toScVal(),
            new Address(subjectAddress).toScVal(),
            nativeToScVal(vcCount, { type: "u32" }),
          ),
        ),
    );
    console.log(`  set_vc_count tx   = ${vcCountTxHash}`);

    await withExponentialBackoff(
      `wait_set_vc_count_confirmation(${subjectAddress})`,
      maxRetries,
      retryBaseDelayMs,
      () => waitForConfirmation(this.server, vcCountTxHash),
    );

    // Step 4: submit update_tx_stats
    const statsTxHash = await withExponentialBackoff(
      `update_tx_stats(${subjectAddress})`,
      maxRetries,
      retryBaseDelayMs,
      () =>
        submitOperation(
          this.server,
          this.config.networkPassphrase,
          this.feederKeypair,
          creditContract.call(
            "update_tx_stats",
            new Address(feederAddress).toScVal(),
            new Address(subjectAddress).toScVal(),
            txStatsToScVal(stats),
          ),
        ),
    );
    console.log(`  update_tx_stats tx = ${statsTxHash}`);

    await withExponentialBackoff(
      `wait_update_tx_stats_confirmation(${subjectAddress})`,
      maxRetries,
      retryBaseDelayMs,
      () => waitForConfirmation(this.server, statsTxHash),
    );

    console.log(`  done`);
  }

  /** Runs one complete feed cycle across all configured subjects. */
  async runCycle(): Promise<void> {
    for (const subject of this.config.subjects) {
      try {
        await this.feedSubject(subject);
      } catch (err) {
        console.error(
          `[feeder] error syncing ${subject}:`,
          err instanceof Error ? err.message : err,
        );
      }
    }
  }

  /**
   * Starts the polling loop. The first cycle runs immediately; subsequent
   * cycles start after pollIntervalMs elapses. Returns a stop function.
   */
  start(): () => void {
    let stopped = false;

    const tick = async (): Promise<void> => {
      if (stopped) return;
      await this.runCycle();
      if (!stopped) {
        setTimeout(() => void tick(), this.config.pollIntervalMs);
      }
    };

    void tick();
    return () => {
      stopped = true;
    };
  }
}

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function withExponentialBackoff<T>(
  operationName: string,
  maxRetries: number,
  baseDelayMs: number,
  fn: () => Promise<T>,
): Promise<T> {
  const retries = Number.isFinite(maxRetries) ? Math.max(0, maxRetries) : 3;
  const delayBase = Number.isFinite(baseDelayMs)
    ? Math.max(1, baseDelayMs)
    : 1_000;

  for (let attempt = 0; ; attempt++) {
    try {
      return await fn();
    } catch (err) {
      const isLastAttempt = attempt >= retries;
      if (isLastAttempt) {
        throw err;
      }

      const delayMs = delayBase * 2 ** attempt;
      console.warn(
        `[feeder] retry ${attempt + 1}/${retries} for ${operationName} in ${delayMs}ms:`,
        err instanceof Error ? err.message : err,
      );
      await sleep(delayMs);
    }
  }
}

// ---------------------------------------------------------------------------
// CLI entry point
// ---------------------------------------------------------------------------

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value) {
    console.error(`Error: environment variable ${name} is not set`);
    process.exit(1);
  }
  return value;
}

if (require.main === module) {
  const feederSecret = requireEnv("FEEDER_SECRET");
  const subjectsRaw = requireEnv("SUBJECTS");
  const creditOracleId = requireEnv("CREDIT_ORACLE_ID");
  const identityOracleId = requireEnv("IDENTITY_ORACLE_ID");

  const networkPassphrase =
    process.env["NETWORK_PASSPHRASE"] ?? "Test SDF Network ; September 2015";
  const rpcUrl =
    process.env["RPC_URL"] ?? "https://soroban-testnet.stellar.org";
  const horizonUrl =
    process.env["HORIZON_URL"] ?? "https://horizon-testnet.stellar.org";
  const simAccount =
    process.env["SIM_ACCOUNT"] ??
    "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
  const pollIntervalMs = parseInt(
    process.env["POLL_INTERVAL_MS"] ?? "3600000",
    10,
  );
  const maxRetries = parseInt(process.env["MAX_RETRIES"] ?? "3", 10);
  const retryBaseDelayMs = parseInt(
    process.env["RETRY_BASE_DELAY_MS"] ?? "1000",
    10,
  );

  const subjects = subjectsRaw
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);

  if (subjects.length === 0) {
    console.error(
      "Error: SUBJECTS must be a non-empty comma-separated list of G... addresses",
    );
    process.exit(1);
  }

  let feederKeypair: Keypair;
  try {
    feederKeypair = Keypair.fromSecret(feederSecret);
  } catch {
    console.error("Error: FEEDER_SECRET is not a valid Stellar secret key");
    process.exit(1);
  }

  console.log("[feeder] starting");
  console.log(`  feeder     : ${feederKeypair.publicKey()}`);
  console.log(`  subjects   : ${subjects.join(", ")}`);
  console.log(`  interval   : ${pollIntervalMs}ms`);
  console.log(`  rpc        : ${rpcUrl}`);
  console.log(`  horizon    : ${horizonUrl}`);
  console.log(`  maxRetries : ${maxRetries}`);
  console.log(`  retryBase  : ${retryBaseDelayMs}ms`);

  const config: FeederConfig = {
    rpcUrl,
    horizonUrl,
    networkPassphrase,
    creditOracleId,
    identityOracleId,
    simAccount,
    subjects,
    pollIntervalMs,
    maxRetries,
    retryBaseDelayMs,
  };

  const feeder = new Feeder(config, feederKeypair);
  feeder.start();
}
