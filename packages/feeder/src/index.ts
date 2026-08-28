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
 *   FEEDER_ALLOW_PARTIAL_STATS — When "false", tx stats gathered from an
 *                            incomplete Horizon pagination pass are NOT written
 *                            on-chain (default: true, i.e. partial data is
 *                            submitted rather than discarded). See #518.
 *   REVOCATION_REGISTRY_ID — Revocation-registry contract address (optional; needed
 *                            for event-driven revocation sync)
 *   GOVERNANCE_ID        — Governance contract address (optional; reserved for future
 *                          governance-aware features; the feeder does not call it yet)
 *   HEALTH_PORT          — When set, starts an HTTP server on this port exposing
 *                          GET /health and GET /ready for production monitoring
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
import {
  HealthTracker,
  createHealthServer,
  parseHealthPort,
} from "./health";

// ---------------------------------------------------------------------------
// Network configurations
// ---------------------------------------------------------------------------

type NetworkType = 'testnet' | 'mainnet' | 'futurenet';

const NETWORK_CONFIGS: Record<NetworkType, {
  networkPassphrase: string;
  rpcUrl: string;
  horizonUrl: string;
  simAccount: string;
}> = {
  testnet: {
    networkPassphrase: "Test SDF Network ; September 2015",
    rpcUrl: "https://soroban-testnet.stellar.org", 
    horizonUrl: "https://horizon-testnet.stellar.org",
    simAccount: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
  },
  mainnet: {
    networkPassphrase: "Public Global Stellar Network ; September 2015",
    rpcUrl: "https://soroban-rpc.mainnet.stellarchain.io",
    horizonUrl: "https://horizon.stellar.org",
    simAccount: "", // Must be set via env var for mainnet
  },
  futurenet: {
    networkPassphrase: "Test SDF Future Network ; October 2022", 
    rpcUrl: "https://rpc-futurenet.stellar.org",
    horizonUrl: "https://horizon-futurenet.stellar.org",
    simAccount: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
  },
};

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
  /** revocation-registry contract address; needed for event-driven revocation sync (optional) */
  revocationRegistryId?: string;
  /** governance contract address; reserved for future governance-aware features (optional) */
  governanceId?: string;
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
  /**
   * Whether `update_tx_stats` may be submitted from a partial Horizon fetch
   * (issue #518). Defaults to `true`: committing understated-but-real data is
   * better than discarding a whole cycle's work. Set to `false` (via
   * `FEEDER_ALLOW_PARTIAL_STATS=false`) to suppress the on-chain write and
   * wait for a complete pass on the next cycle.
   */
  allowPartialStats?: boolean;
  /** Whether to skip legacy set_vc_count calls when identity oracle is configured */
  skipLegacyVcCount?: boolean;
  /** Max consecutive failures before a subject enters dead-letter state (default: 5) */
  maxConsecutiveFailures?: number;
  /** Network type for configuration */
  network?: string;
  /** Whether to run event-driven synchronization mode */
  eventDriven?: boolean;
  /** How often to poll for events, in milliseconds */
  eventPollIntervalMs?: number;
  /** Optional in-memory health tracker for /health and /ready endpoints */
  healthTracker?: HealthTracker;
}

/** Transaction statistics to be written to the credit-oracle via update_tx_stats. */
export interface TxStats {
  /** Total XLM payment volume over the last 30 days, in stroops (1 XLM = 10 000 000 stroops). */
  volume30d: bigint;
  /** Number of distinct payment transactions in the last 30 days. */
  txCount30d: number;
  /** Average number of distinct counterparties per transaction. */
  avgCounterparties: number;
  /**
   * True when Horizon pagination ended early (issue #518), meaning these
   * counters cover only the pages fetched before the failure and therefore
   * understate the real 30-day activity. Always present on values returned by
   * `fetchHorizonStats`; optional so existing callers that build a `TxStats`
   * literal keep compiling.
   */
  partial?: boolean;
}

/**
 * Minimal Horizon operation record shape used by fetchHorizonStats.
 * Only the fields needed for 30-day stats aggregation are surfaced.
 *
 * Extended to cover:
 *   - payment               : plain XLM/asset transfer
 *   - path_payment_strict_send / path_payment_strict_receive :
 *       source_amount / source_asset_type (send side)
 *       amount / asset_type (destination side)
 *   - create_account        : counted toward tx_count only
 *   - claim_claimable_balance : counted toward tx_count only
 */
interface HorizonOperationRecord {
  type: string;
  transaction_hash: string;
  created_at: string;
  /** Sender address (payment, path_payment_strict_send/receive) */
  from?: string;
  /** Recipient address (payment, path_payment_strict_send/receive) */
  to?: string;
  /** Destination amount string */
  amount?: string;
  /** Destination asset type ("native" = XLM) */
  asset_type?: string;
  /** Source amount string (path payments) */
  source_amount?: string;
  /** Source asset type (path payments, "native" = XLM) */
  source_asset_type?: string;
  /** New account address (create_account) */
  account?: string;
  /** Funder address (create_account) */
  funder?: string;
  /** Starting balance in XLM (create_account) */
  starting_balance?: string;
  /** Claimant address (claim_claimable_balance) */
  claimant?: string;
}

/** Minimal shape of an HTTP error surfaced by Horizon/RPC clients. */
interface ErrorWithMeta {
  message?: string;
  code?: string;
  response?: {
    status?: number;
    data?: { extras?: { result_codes?: unknown } };
    headers?: { get(name: string): string | null } | Record<string, string>;
  };
}

/**
 * Retry tuning for `fetchHorizonStats` pagination (issue #518).
 *
 * Exposed so callers — and tests — can shrink the backoff instead of waiting
 * out the production defaults.
 */
export interface FetchHorizonStatsOptions {
  /** Max retry attempts for a transient `page.next()` failure (default: 3). */
  maxRetries?: number;
  /** Base delay between page retries, in milliseconds (default: 1 000). */
  retryBaseDelayMs?: number;
}

/** Minimal shape of a Horizon payments page consumed by fetchHorizonStats. */
interface HorizonPaymentPage {
  records: HorizonOperationRecord[];
  next: () => Promise<HorizonPaymentPage>;
}

/**
 * Reads the `Retry-After` HTTP header (in seconds) from a response headers
 * object, falling back to a raw string lookup. Returns undefined when absent.
 */
function getRetryAfterSeconds(
  headers: { get(name: string): string | null } | Record<string, string>,
): number | undefined {
  const raw =
    typeof headers.get === "function"
      ? headers.get("retry-after")
      : (headers as Record<string, string>)["retry-after"];
  if (!raw) return undefined;
  const seconds = Number(raw);
  return Number.isNaN(seconds) ? undefined : Math.max(0.5, seconds);
}

/**
 * Per-subject snapshot of the last synced state.
 * Used to detect changes between cycles so only modified subjects are re-synced.
 */
interface SubjectSyncState {
  vcCount: number;
  volume30d: bigint;
  txCount30d: number;
  avgCounterparties: number;
}

// ---------------------------------------------------------------------------
// Validation and error handling helpers
// ---------------------------------------------------------------------------

/**
 * Checks if an address is a valid Stellar public key (G-address).
 * Must start with 'G', be 56 characters total, and pass Stellar SDK validation.
 */
function isValidStellarAddress(address: string): boolean {
  if (!address || typeof address !== "string") return false;
  if (!address.startsWith("G")) return false;
  if (address.length !== 56) return false;

  // Validate against Stellar SDK
  try {
    Keypair.fromPublicKey(address);
    return true;
  } catch {
    return false;
  }
}

/**
 * Checks if a string is a valid Soroban contract address (C-address).
 * Must start with 'C', be 56 characters total, and pass Stellar SDK validation.
 *
 * Used for the optional `revocationRegistryId` / `governanceId` configuration.
 */
export function isValidSorobanContractId(address: string): boolean {
  if (!address || typeof address !== "string") return false;
  if (!address.startsWith("C")) return false;
  if (address.length !== 56) return false;

  // Validate against Stellar SDK (verifies the contract ID checksum)
  try {
    Address.fromString(address);
    return true;
  } catch {
    return false;
  }
}

/**
 * Checks if an error is a Horizon "account not found" (404) response.
 * These errors are permanent and should not be retried.
 */
function isAccountNotFoundError(error: unknown): boolean {
  const err = error as ErrorWithMeta;

  // Check for Horizon 404 response
  if (err?.response?.status === 404) return true;

  // Check for Horizon error code in extras
  if (err?.response?.data?.extras?.result_codes) return true;

  // Check for SDK-specific not-found error messages
  if (
    err?.message &&
    typeof err.message === "string" &&
    err.message.includes("account") &&
    err.message.includes("not found")
  ) {
    return true;
  }

  return false;
}

/**
 * Checks if an error is a transient failure that should be retried.
 * Includes network timeouts, rate limits (429), and server errors (500/503).
 */
function isTransientError(error: unknown): boolean {
  const err = error as ErrorWithMeta;

  // Network timeout
  if (err?.code === "ECONNREFUSED" || err?.code === "ETIMEDOUT") return true;

  // Rate limit
  if (err?.response?.status === 429) return true;

  // Server errors
  if (err?.response?.status === 500 || err?.response?.status === 503)
    return true;

  // General network errors
  if (err?.message && typeof err.message === "string") {
    const msg = err.message.toLowerCase();
    if (
      msg.includes("timeout") ||
      msg.includes("econnrefused") ||
      msg.includes("network")
    ) {
      return true;
    }
  }

  return false;
}

/**
 * Checks if an error is permanent and should not be retried.
 * Includes 404s, invalid addresses, and simulation failures.
 */
function isPermanentError(error: unknown): boolean {
  return isAccountNotFoundError(error);
}

// ---------------------------------------------------------------------------
// Horizon helpers
// ---------------------------------------------------------------------------

/**
 * Zeroed stats for the "nothing to report" paths (invalid address, unknown
 * account, empty history). `partial: false` is explicit so consumers can treat
 * the flag as a plain boolean rather than tri-state (issue #518).
 */
function EMPTY_TX_STATS(): TxStats {
  return {
    volume30d: BigInt(0),
    txCount30d: 0,
    avgCounterparties: 0,
    partial: false,
  };
}

/**
 * Fetches 30-day payment statistics for a Stellar address via the Horizon API.
 *
 * Paginates backwards through the operation history (payments endpoint),
 * stopping at the 30-day cutoff. The following operation types are recognised:
 *
 * | Op type                        | volume_30d       | tx_count_30d |
 * | ------------------------------ | ---------------- | ------------ |
 * | payment                        | XLM leg only     | ✓            |
 * | path_payment_strict_send       | XLM source leg   | ✓            |
 * | path_payment_strict_receive    | XLM dest leg     | ✓            |
 * | create_account                 | —                | ✓            |
 * | claim_claimable_balance        | —                | ✓            |
 *
 * Only native (XLM) asset amounts are included in volume; non-native assets
 * are counted toward tx_count and counterparties but not volume, matching
 * the credit-oracle's scoring semantics.
 *
 * This change is backward-compatible: existing scores cannot decrease because
 * we only ever add to volume and tx_count.
 *
 * Returns empty stats if the address is invalid or the account is not found.
 *
 * ### Partial results (issue #518)
 *
 * Failing to fetch page 4 of 10 used to throw away the records already read
 * from pages 1-3, so the feeder restarted from scratch every cycle and a
 * flaky Horizon could starve a subject indefinitely. A transient `page.next()`
 * failure is now retried up to `options.maxRetries` times with exponential
 * backoff; if it still fails, pagination stops and whatever was aggregated so
 * far is returned with `partial: true` instead of being discarded.
 *
 * `partial` is advisory: the caller decides whether understated stats are
 * worth writing on-chain (see `FeederConfig.allowPartialStats`). The *initial*
 * page request is deliberately excluded — a failure there yields no data at
 * all, so it still throws and lets the caller's own retry loop handle it.
 */
export async function fetchHorizonStats(
  horizonUrl: string,
  address: string,
  options: FetchHorizonStatsOptions = {},
): Promise<TxStats> {
  const maxPageRetries = Number.isFinite(options.maxRetries as number)
    ? Math.max(0, options.maxRetries as number)
    : 3;
  const pageRetryBaseDelayMs = Number.isFinite(options.retryBaseDelayMs as number)
    ? Math.max(1, options.retryBaseDelayMs as number)
    : 1_000;
  // Validate address before making API calls
  if (!isValidStellarAddress(address)) {
    console.log(`[feeder] Skipping invalid Stellar address: ${address}`);
    return EMPTY_TX_STATS();
  }

  const horizon = new Horizon.Server(horizonUrl);
  const cutoff = new Date(Date.now() - 30 * 24 * 60 * 60 * 1000);

  let volumeStroops = BigInt(0);
  const txHashes = new Set<string>();
  // Map from transaction hash → set of distinct counterparty addresses
  const counterpartiesPerTx = new Map<string, Set<string>>();

  // Rate-limit aware call helper: if Horizon responds with 429 and a
  // `Retry-After` header, wait that duration and retry.
  async function callWithHorizonRateLimit<T>(fn: () => Promise<T>): Promise<T> {
    const maxRateLimitRetries = 5;
    for (let attempt = 0; ; attempt++) {
      try {
        return await fn();
      } catch (err) {
        const errMeta = err as ErrorWithMeta;
        const status = errMeta?.response?.status;
        const headers = errMeta?.response?.headers;
        if (status === 429 && headers) {
          // Try to read `Retry-After` header (seconds). Fall back to a small delay.
          let retryAfterMs = 1000;
          try {
            const sec = getRetryAfterSeconds(headers);
            if (sec !== undefined) retryAfterMs = Math.max(500, sec * 1000);
          } catch (e) {
            // ignore header parsing errors
          }
          console.warn(
            `[feeder] Horizon rate-limited (429); retrying in ${retryAfterMs}ms`,
          );
          if (attempt >= maxRateLimitRetries) throw err;
          await sleep(retryAfterMs);
          continue;
        }
        throw err;
      }
    }
  }

  /**
   * Advances to the next page, retrying transient failures (issue #518).
   *
   * Returns `null` once the retry budget is exhausted, which the pagination
   * loop treats as "stop here and keep what we have" rather than as a hard
   * failure. Non-transient errors are rethrown unchanged: a malformed
   * response or a 404 will not fix itself, and silently truncating on those
   * would hide real bugs behind an understated score.
   */
  async function fetchNextPage(
    current: HorizonPaymentPage,
  ): Promise<HorizonPaymentPage | null> {
    for (let attempt = 0; ; attempt++) {
      try {
        // callWithHorizonRateLimit already absorbs 429s with Retry-After; this
        // outer loop covers the transient errors it rethrows (timeouts,
        // 500/503, connection resets, and 429s past its own budget).
        return await callWithHorizonRateLimit(() => current.next());
      } catch (err) {
        if (!isTransientError(err)) throw err;
        if (attempt >= maxPageRetries) {
          console.warn(
            `[feeder] Horizon pagination failed for ${address} after` +
              ` ${maxPageRetries} retries; committing partial stats:`,
            err instanceof Error ? err.message : err,
          );
          return null;
        }
        const delayMs = pageRetryBaseDelayMs * 2 ** attempt;
        console.warn(
          `[feeder] retry ${attempt + 1}/${maxPageRetries} for` +
            ` horizon_next_page(${address}) in ${delayMs}ms:`,
          err instanceof Error ? err.message : err,
        );
        await sleep(delayMs);
      }
    }
  }

  // Set when pagination stops early, so the caller can tell an understated
  // 30-day window apart from a genuinely quiet account.
  let partial = false;

  let page: HorizonPaymentPage;
  try {
    page = await callWithHorizonRateLimit(() =>
      horizon.payments().forAccount(address).order("desc").limit(200).call(),
    );
  } catch (err) {
    if (isAccountNotFoundError(err)) {
      console.log(`[feeder] Account not found for ${address}, skipping`);
      return EMPTY_TX_STATS();
    }
    if (isTransientError(err)) {
      throw err;
    }
    console.error(
      `[feeder] Error fetching Horizon stats for ${address}:`,
      err instanceof Error ? err.message : err,
    );
    return EMPTY_TX_STATS();
  }

  // Handle case where Horizon returns zero records
  if (!page.records || page.records.length === 0) {
    return EMPTY_TX_STATS();
  }

  outer: while (page.records.length > 0) {
    for (const record of page.records) {
      const op = record;

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
      } else if (
        op.type === "path_payment_strict_send" ||
        op.type === "path_payment_strict_receive"
      ) {
        // For path payments we count whichever leg(s) are native (XLM).
        // path_payment_strict_send:    source_amount / source_asset_type is the
        //   amount the sender spent; amount / asset_type is what the recipient got.
        // path_payment_strict_receive: same field layout from Horizon.
        //
        // Strategy: if the subject is the sender, count the XLM source leg;
        //           if the subject is the recipient, count the XLM destination leg;
        //           both legs can be XLM simultaneously (same-asset path).
        if (op.from === address && op.source_asset_type === "native" && op.source_amount) {
          const amountXLM = parseFloat(op.source_amount);
          volumeStroops += BigInt(Math.round(amountXLM * 10_000_000));
        }
        if (op.to === address && op.asset_type === "native" && op.amount) {
          const amountXLM = parseFloat(op.amount);
          volumeStroops += BigInt(Math.round(amountXLM * 10_000_000));
        }

        // Track counterparty
        const counterparty = op.from === address ? op.to : op.from;
        if (counterparty) {
          counterpartiesPerTx.get(txHash)!.add(counterparty);
        }
      } else if (op.type === "create_account") {
        // create_account counts as a transaction; no XLM volume is added
        // (the starting_balance is locked in the new account, not a payment flow).
        // Track the funder/new-account as a counterparty.
        const counterparty = op.funder === address ? op.account : op.funder;
        if (counterparty) {
          counterpartiesPerTx.get(txHash)!.add(counterparty);
        }
      } else if (op.type === "claim_claimable_balance") {
        // Claimable balance claims represent real XLM flows but the amount
        // requires a separate API call which is too expensive per-operation.
        // Count them toward tx_count only; volume is not updated.
        const counterparty = op.claimant;
        if (counterparty && counterparty !== address) {
          counterpartiesPerTx.get(txHash)!.add(counterparty);
        }
      }
    }

    const nextPage = await fetchNextPage(page);
    if (nextPage === null) {
      // Retry budget spent. Keep the pages already aggregated instead of
      // throwing them away, and flag the result as incomplete (issue #518).
      partial = true;
      break;
    }
    page = nextPage;
  }

  const txCount30d = txHashes.size;

  let totalCounterparties = 0;
  for (const cps of counterpartiesPerTx.values()) {
    totalCounterparties += cps.size;
  }
  const avgCounterparties =
    txCount30d > 0 ? Math.round(totalCounterparties / txCount30d) : 0;

  return { volume30d: volumeStroops, txCount30d, avgCounterparties, partial };
}

/** Extract the sequence number string from a Soroban RPC account response. */
function getSequence(account: Account): string {
  return account.sequenceNumber();
}

/**
 * Reads the active (non-revoked) VC count from the identity-oracle.
 * Uses a read-only simulation — no signing or fees required.
 *
 * Returns 0 for unknown subjects without throwing.
 */
export async function getActiveVcCount(
  server: SorobanRpc.Server,
  config: Pick<
    FeederConfig,
    "identityOracleId" | "networkPassphrase" | "simAccount"
  >,
  subjectAddress: string,
): Promise<number> {
  // Validate address before making API calls
  if (!isValidStellarAddress(subjectAddress)) {
    console.log(
      `[feeder] Skipping invalid Stellar address for VC count: ${subjectAddress}`,
    );
    return 0;
  }

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

  let sim: SorobanRpc.Api.SimulateTransactionResponse;
  try {
    sim = await server.simulateTransaction(tx);
  } catch (err) {
    console.error(
      `[feeder] Error simulating get_active_vc_count for ${subjectAddress}:`,
      err instanceof Error ? err.message : err,
    );
    return 0;
  }

  if (SorobanRpc.Api.isSimulationError(sim)) {
    console.error(
      `[feeder] get_active_vc_count simulation failed for ${subjectAddress}: ${sim.error}`,
    );
    return 0;
  }
  if (!SorobanRpc.Api.isSimulationSuccess(sim)) {
    console.error(
      `[feeder] Unexpected simulation response for get_active_vc_count on ${subjectAddress}`,
    );
    return 0;
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

  const preparedTx = SorobanRpc.assembleTransaction(tx, sim).build();
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
  /** Tracks the last-synced state per subject to avoid redundant syncs. */
  private syncState = new Map<string, SubjectSyncState>();
  /** Tracks consecutive failure count per subject. */
  private failureCounts = new Map<string, number>();
  /** Subjects that have exceeded the maxConsecutiveFailures threshold. */
  private deadLetterSubjects = new Set<string>();
  /** Tracks vc_hash -> subject from VCAnch events. Used for resolving Revoked events. */
  private hashToSubject = new Map<string, string>();

  constructor(
    private config: FeederConfig,
    private feederKeypair: Keypair,
  ) {
    this.server = new SorobanRpc.Server(config.rpcUrl);
  }

  /**
   * Returns the current set of dead-letter subjects — those that have failed
   * at least `maxConsecutiveFailures` consecutive cycles. Subjects are removed
   * from this set once they feed successfully.
   */
  getDeadLetterSubjects(): string[] {
    return Array.from(this.deadLetterSubjects);
  }

  /**
   * Checks whether the credit-oracle has an identity-oracle configured
   * for cross-contract VC count lookups.
   * Uses a read-only simulation — no signing or fees required.
   */
  async getHasIdentityOracle(): Promise<boolean> {
    const contract = new Contract(this.config.creditOracleId);
    const sourceAccount = new Account(this.config.simAccount, "0");

    const tx = new TransactionBuilder(sourceAccount, {
      fee: BASE_FEE,
      networkPassphrase: this.config.networkPassphrase,
    })
      .addOperation(contract.call("get_identity_oracle"))
      .setTimeout(30)
      .build();

    const sim = await this.server.simulateTransaction(tx);

    if (SorobanRpc.Api.isSimulationError(sim)) {
      throw new Error(`get_identity_oracle simulation failed: ${sim.error}`);
    }
    if (!SorobanRpc.Api.isSimulationSuccess(sim)) {
      throw new Error("Unexpected simulation response for get_identity_oracle");
    }

    const result = scValToNative(sim.result!.retval);
    // Option<Address> — null/undefined means not configured
    return result !== null && result !== undefined;
  }

  /**
   * Syncs a single subject: fetches stats, then submits set_vc_count followed
   * by update_tx_stats, waiting for each transaction to be confirmed.
   *
   * If `signal` is aborted between steps, the in-flight step is allowed to
   * complete (transaction submission cannot be cancelled mid-flight) but no
   * further steps are started — the subject may end up partially synced.
   */
  async feedSubject(subjectAddress: string, signal?: AbortSignal): Promise<void> {
    const maxRetries = this.config.maxRetries ?? 3;
    const retryBaseDelayMs = this.config.retryBaseDelayMs ?? 1_000;
    // Issue #518: partial stats are submitted unless explicitly opted out.
    const allowPartialStats = this.config.allowPartialStats !== false;

    // Step 1: read active VC count from identity-oracle
    const vcCount = await withExponentialBackoff(
      `get_active_vc_count(${subjectAddress})`,
      maxRetries,
      retryBaseDelayMs,
      () => getActiveVcCount(this.server, this.config, subjectAddress),
    );
    if (signal?.aborted) {
      console.log(`[feeder] ${subjectAddress} — aborted after vc_count read`);
      return;
    }

    // Step 2: fetch 30-day payment stats from Horizon
    const stats = await withExponentialBackoff(
      `fetch_horizon_stats(${subjectAddress})`,
      maxRetries,
      retryBaseDelayMs,
      () =>
        fetchHorizonStats(this.config.horizonUrl, subjectAddress, {
          maxRetries,
          retryBaseDelayMs,
        }),
    );
    if (signal?.aborted) {
      console.log(`[feeder] ${subjectAddress} — aborted after horizon fetch`);
      return;
    }

    // Check whether on-chain data has changed since the last sync.
    const lastState = this.syncState.get(subjectAddress);
    if (lastState) {
      if (
        lastState.vcCount === vcCount &&
        lastState.volume30d === stats.volume30d &&
        lastState.txCount30d === stats.txCount30d &&
        lastState.avgCounterparties === stats.avgCounterparties
      ) {
        console.log(`[feeder] ${subjectAddress} — unchanged, skipping`);
        return;
      }
    }

    console.log(`[feeder] syncing ${subjectAddress}`);
    console.log(`  vc_count          = ${vcCount}`);
    console.log(
      `  volume_30d        = ${stats.volume30d} stroops` +
        ` (${Number(stats.volume30d) / 10_000_000} XLM)`,
    );
    console.log(`  tx_count_30d      = ${stats.txCount30d}`);
    console.log(`  avg_counterparties = ${stats.avgCounterparties}`);
    // Issue #518: always logged as a boolean so log scrapers can alert on it.
    console.log(`  partial           = ${stats.partial === true}`);
    if (stats.partial) {
      console.warn(
        `[feeder] ${subjectAddress} — Horizon pagination was incomplete;` +
          ` volume_30d and tx_count_30d understate real activity` +
          ` (partial=true, allowPartialStats=${allowPartialStats})`,
      );
    }

    const creditContract = new Contract(this.config.creditOracleId);
    const feederAddress = this.feederKeypair.publicKey();

    // Check if cross-contract VC count lookup is configured
    const identityOracleConfigured = await withExponentialBackoff(
      `get_identity_oracle`,
      maxRetries,
      retryBaseDelayMs,
      () => this.getHasIdentityOracle(),
    );

    // Step 3: submit set_vc_count (skip if cross-contract is configured or explicitly disabled)
    const shouldSkipVcCount = identityOracleConfigured || this.config.skipLegacyVcCount === true;
    
    if (shouldSkipVcCount) {
      const reason = identityOracleConfigured 
        ? "cross-contract lookup configured" 
        : "skipLegacyVcCount enabled";
      console.log(`  skipping set_vc_count (${reason})`);
    } else {
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
    }
    if (signal?.aborted) {
      console.log(
        `[feeder] ${subjectAddress} — aborted after set_vc_count step`,
      );
      return;
    }

    // Step 4: submit update_tx_stats
    //
    // Issue #518: partial stats understate the 30-day window, which would push
    // an artificially low score on-chain. Operators who prefer a stale-but-
    // complete score over a fresh-but-truncated one set
    // FEEDER_ALLOW_PARTIAL_STATS=false to suppress this write. vc_count above
    // is unaffected — it comes from the identity-oracle, not from Horizon.
    //
    // syncState is deliberately left untouched on this path so the next cycle
    // re-fetches the subject instead of treating the truncated numbers as the
    // last known good state.
    if (stats.partial && !allowPartialStats) {
      console.warn(
        `[feeder] ${subjectAddress} — skipping update_tx_stats:` +
          ` partial=true and FEEDER_ALLOW_PARTIAL_STATS=false.` +
          ` Stats will be re-fetched next cycle.`,
      );
      return;
    }

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

    // Update the sync state so the next cycle can detect changes.
    this.syncState.set(subjectAddress, {
      vcCount,
      volume30d: stats.volume30d,
      txCount30d: stats.txCount30d,
      avgCounterparties: stats.avgCounterparties,
    });
  }

  /**
   * Runs one complete feed cycle across all configured subjects.
   *
   * Validates all subject addresses at the start. If it's already aborted,
   * the loop stops and no further subjects are started — subjects already
   * in progress when the signal was raised are left to `feedSubject` to wind
   * down gracefully.
   *
   * Logs a summary at the end with succeeded/skipped/failed counts.
   */
  async runCycle(signal?: AbortSignal): Promise<void> {
    // Validate all subjects at the start of the cycle
    let validSubjects = this.config.subjects;
    let invalidCount = 0;
    const invalidAddresses: string[] = [];

    for (const subject of this.config.subjects) {
      if (!isValidStellarAddress(subject)) {
        invalidCount++;
        invalidAddresses.push(subject);
      }
    }

    if (invalidCount > 0) {
      console.warn(
        `[feeder] Found ${invalidCount} invalid addresses, processing ${this.config.subjects.length - invalidCount} valid subjects`,
      );
      validSubjects = this.config.subjects.filter(isValidStellarAddress);
    }

    const maxFailures = this.config.maxConsecutiveFailures ?? 5;

    let succeeded = 0;
    let skipped = 0;
    let failed = 0;

    for (const subject of validSubjects) {
      if (signal?.aborted) {
        console.log(
          `[feeder] cycle aborted — not starting remaining subjects`,
        );
        break;
      }
      try {
        await this.feedSubject(subject, signal);
        succeeded++;
        // Clear failure tracking and dead-letter state on success
        if (this.deadLetterSubjects.has(subject)) {
          console.log(
            `[feeder] ${subject} recovered — removed from dead-letter queue`,
          );
          this.deadLetterSubjects.delete(subject);
        }
        this.failureCounts.delete(subject);
      } catch (err) {
        const errorMsg = err instanceof Error ? err.message : String(err);
        const errorType = isPermanentError(err) ? "permanent" : "transient";
        const action = isPermanentError(err) ? "skipped" : "will retry";
        console.error(
          `[feeder] error syncing ${subject} (${errorType}): ${errorMsg} — ${action}`,
        );
        if (isPermanentError(err)) {
          skipped++;
        } else {
          failed++;
        }

        // Track consecutive failures for dead-letter detection
        const prev = this.failureCounts.get(subject) ?? 0;
        const next = prev + 1;
        this.failureCounts.set(subject, next);

        if (next >= maxFailures) {
          this.deadLetterSubjects.add(subject);
          console.error(
            `[feeder] [dead-letter] ${subject} has failed ${next} consecutive cycles (threshold: ${maxFailures})`,
          );
        } else {
          console.warn(
            `[feeder] [dead-letter] ${subject} failure ${next}/${maxFailures} — will retry next cycle`,
          );
        }
      }
    }

    console.log(
      `[feeder] Cycle complete: ${succeeded} succeeded, ${skipped} skipped, ${failed} failed`,
    );

    this.config.healthTracker?.recordCycleResult(succeeded, failed);
  }

  /**
  /**
   * Starts the event polling loop. Used when eventDriven = true.
   * Polls identity-oracle for VCAnch events and revocation-registry for Revoked events.
   */
  startEventDriven(signal?: AbortSignal): () => void {
    const controller = new AbortController();
    if (signal) {
      if (signal.aborted) {
        controller.abort();
      } else {
        signal.addEventListener("abort", () => controller.abort(), {
          once: true,
        });
      }
    }

    let identityLastLedger: number | undefined;
    let revocationLastLedger: number | undefined;
    let isPollingEvents = false;
    const pollIntervalMs = this.config.eventPollIntervalMs ?? 30_000;

    const pollEvents = async (): Promise<void> => {
      if (controller.signal.aborted) return;
      if (isPollingEvents) return;
      isPollingEvents = true;

      try {
        if (
          identityLastLedger === undefined ||
          (this.config.revocationRegistryId && revocationLastLedger === undefined)
        ) {
          const latestLedger = await this.server.getLatestLedger();
          if (identityLastLedger === undefined) {
            identityLastLedger = latestLedger.sequence;
          }
          if (this.config.revocationRegistryId && revocationLastLedger === undefined) {
            revocationLastLedger = latestLedger.sequence;
          }
        }

        if (controller.signal.aborted) return;

        // Poll Identity Oracle for VCAnch
        try {
          const response = await this.server.getEvents({
            startLedger: identityLastLedger,
            filters: [
              {
                type: "contract",
                contractIds: [this.config.identityOracleId],
                topics: [[xdr.ScVal.scvSymbol("VCAnch").toXDR("base64")]],
              },
            ],
            limit: 100,
          });

          if (response.events.length > 0) {
            const latestEventLedger = response.events.reduce(
              (highestLedger, event) => Math.max(highestLedger, event.ledger),
              identityLastLedger!,
            );
            identityLastLedger = Math.max(response.latestLedger, latestEventLedger) + 1;

            for (const event of response.events) {
              const val = event.value;
              if (val.switch().name === "scvVec") {
                const vec = val.vec();
                if (vec && vec.length >= 3) {
                  const subjectScVal = vec[1];
                  const hashScVal = vec[2];

                  let subject: string | undefined;
                  try {
                    if (subjectScVal.switch().name === "scvAddress") {
                      subject = scValToNative(subjectScVal) as string;
                    }
                  } catch (e) { /* ignore */ }

                  let hash: string | undefined;
                  try {
                    if (hashScVal.switch().name === "scvBytes") {
                      hash = Buffer.from(hashScVal.bytes()).toString("hex");
                    }
                  } catch (e) { /* ignore */ }

                  if (subject && hash) {
                    this.hashToSubject.set(hash, subject);
                    if (this.config.subjects.includes(subject)) {
                      this.feedSubject(subject, controller.signal).catch((err) => {
                        console.error(`[feeder] Error event-syncing ${subject}:`, err);
                      });
                    }
                  }
                }
              }
            }
          } else {
            identityLastLedger = response.latestLedger + 1;
          }
        } catch (err) {
          // ignore transient RPC errors in polling
        }

        // Poll Revocation Registry for Revoked
        if (this.config.revocationRegistryId) {
          try {
            const response = await this.server.getEvents({
              startLedger: revocationLastLedger,
              filters: [
                {
                  type: "contract",
                  contractIds: [this.config.revocationRegistryId],
                  topics: [[xdr.ScVal.scvSymbol("Revoked").toXDR("base64")]],
                },
              ],
              limit: 100,
            });

            if (response.events.length > 0) {
              const latestEventLedger = response.events.reduce(
                (highestLedger, event) => Math.max(highestLedger, event.ledger),
                revocationLastLedger!,
              );
              revocationLastLedger = Math.max(response.latestLedger, latestEventLedger) + 1;

              for (const event of response.events) {
                const val = event.value;
                if (val.switch().name === "scvVec") {
                  const vec = val.vec();
                  if (vec && vec.length >= 2) {
                    const hashScVal = vec[1];

                    let hash: string | undefined;
                    try {
                      if (hashScVal.switch().name === "scvBytes") {
                        hash = Buffer.from(hashScVal.bytes()).toString("hex");
                      }
                    } catch (e) { /* ignore */ }

                    if (hash) {
                      const subject = this.hashToSubject.get(hash);
                      if (subject && this.config.subjects.includes(subject)) {
                        this.feedSubject(subject, controller.signal).catch((err) => {
                          console.error(`[feeder] Error event-syncing ${subject}:`, err);
                        });
                      }
                    }
                  }
                }
              }
            } else {
              revocationLastLedger = response.latestLedger + 1;
            }
          } catch (err) {
            // ignore transient RPC errors in polling
          }
        }
      } finally {
        isPollingEvents = false;
        if (!controller.signal.aborted) {
          setTimeout(() => void pollEvents(), pollIntervalMs);
        }
      }
    };

    void pollEvents();
    return () => {
      controller.abort();
    };
  }

  /**
   * Starts the polling loop. The first cycle runs immediately; subsequent
   * cycles start after pollIntervalMs elapses. Returns a stop function that
   * triggers graceful shutdown: the in-progress cycle (and whichever subject
   * is currently mid-sync) is allowed to wind down, but no new subject or
   * cycle is started afterward.
   *
   * An external `AbortSignal` may be supplied to tie the feeder's shutdown
   * to another source (e.g. a process-level controller); calling the
   * returned `stop()` function aborts the feeder regardless.
   */
  start(signal?: AbortSignal): () => void {
    const controller = new AbortController();
    if (signal) {
      if (signal.aborted) {
        controller.abort();
      } else {
        signal.addEventListener("abort", () => controller.abort(), {
          once: true,
        });
      }
    }

    let stopEventDriven: (() => void) | undefined;
    if (this.config.eventDriven) {
      stopEventDriven = this.startEventDriven(controller.signal);
    }

    const tick = async (): Promise<void> => {
      if (controller.signal.aborted) return;
      await this.runCycle(controller.signal);
      if (!controller.signal.aborted) {
        setTimeout(() => void tick(), this.config.pollIntervalMs);
      }
    };

    void tick();
    return () => {
      controller.abort();
      if (stopEventDriven) stopEventDriven();
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

      // If the error carries a `Retry-After` header, prefer that delay.
      let retryAfterMs: number | undefined = undefined;
      try {
        const errMeta = err as ErrorWithMeta;
        const status = errMeta?.response?.status;
        const headers = errMeta?.response?.headers;
        if (status === 429 && headers) {
          const sec = getRetryAfterSeconds(headers);
          if (sec !== undefined) retryAfterMs = Math.max(500, sec * 1000);
        }
      } catch (e) {
        /* ignore */
      }

      const delayMs = retryAfterMs ?? delayBase * 2 ** attempt;
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

/** Minimum acceptable polling interval in milliseconds (1 minute). */
export const MIN_POLL_INTERVAL_MS = 60_000;

/**
 * Parses and validates a raw POLL_INTERVAL_MS string value.
 *
 * Rules:
 *   - Must be a plain integer string — floats, mixed strings, and empty
 *     values are all rejected with a descriptive error.
 *   - Must be a positive integer (zero or negative cause an error exit).
 *   - Must be >= MIN_POLL_INTERVAL_MS. Values below this threshold emit a
 *     warning and then exit with an error to prevent accidentally hammering
 *     the RPC endpoint.
 *
 * @param raw - The raw string from the environment variable (or a default).
 * @returns The validated poll interval in milliseconds.
 */
export function parsePollIntervalMs(raw: string): number {
  const trimmed = raw.trim();

  // Reject anything that is not a plain integer string (no floats, no mixed).
  if (!/^-?\d+$/.test(trimmed)) {
    console.error(
      `Error: POLL_INTERVAL_MS must be a positive integer (got "${raw}"). ` +
        `Non-numeric values are not accepted.`,
    );
    process.exit(1);
  }

  const value = parseInt(trimmed, 10);

  if (value <= 0) {
    console.error(
      `Error: POLL_INTERVAL_MS must be a positive integer greater than zero (got ${value}).`,
    );
    process.exit(1);
  }

  if (value < MIN_POLL_INTERVAL_MS) {
    console.warn(
      `Warning: POLL_INTERVAL_MS is set to ${value}ms, which is below the recommended ` +
        `minimum of ${MIN_POLL_INTERVAL_MS}ms (1 minute). ` +
        `Values this low may hammer the RPC endpoint. ` +
        `Set POLL_INTERVAL_MS to at least ${MIN_POLL_INTERVAL_MS}.`,
    );
    console.error(
      `Error: POLL_INTERVAL_MS must be at least ${MIN_POLL_INTERVAL_MS}ms (got ${value}).`,
    );
    process.exit(1);
  }

  return value;
}

if (require.main === module) {
  const feederSecret = requireEnv("FEEDER_SECRET");
  const subjectsRaw = requireEnv("SUBJECTS");
  const creditOracleId = requireEnv("CREDIT_ORACLE_ID");
  const identityOracleId = requireEnv("IDENTITY_ORACLE_ID");

  // Get network from NETWORK env var, default to testnet
  const network = (process.env["NETWORK"]?.toLowerCase() as NetworkType) || 'testnet';
  if (network !== 'testnet' && network !== 'mainnet' && network !== 'futurenet') {
    console.error(`Error: NETWORK must be one of: testnet, mainnet, futurenet. Got: ${process.env["NETWORK"]}`);
    process.exit(1);
  }

  // Get network defaults, allow env var overrides
  const networkDefaults = NETWORK_CONFIGS[network];
  const networkPassphrase = process.env["NETWORK_PASSPHRASE"] ?? networkDefaults.networkPassphrase;
  const rpcUrl = process.env["RPC_URL"] ?? networkDefaults.rpcUrl;
  const horizonUrl = process.env["HORIZON_URL"] ?? networkDefaults.horizonUrl;
  const simAccount = process.env["SIM_ACCOUNT"] ?? networkDefaults.simAccount;

  // Validate mainnet SIM_ACCOUNT requirement
  if (network === 'mainnet' && !simAccount) {
    console.error(
      "Error: SIM_ACCOUNT is required for mainnet feeder operations. Set SIM_ACCOUNT=G... to a funded mainnet account."
    );
    process.exit(1);
  }

  const pollIntervalMs = parsePollIntervalMs(
    process.env["POLL_INTERVAL_MS"] ?? "3600000",
  );
  const maxRetries = parseInt(process.env["MAX_RETRIES"] ?? "3", 10);
  const retryBaseDelayMs = parseInt(
    process.env["RETRY_BASE_DELAY_MS"] ?? "1000",
    10,
  );
  // Issue #518: opt-out flag. Anything other than the exact string "false"
  // keeps the default of committing partial stats, so a typo cannot silently
  // stop a feeder from writing.
  const allowPartialStats = process.env["FEEDER_ALLOW_PARTIAL_STATS"] !== "false";

  // Optional contract integrations. The feeder must not fail when these are
  // absent — they only enable additional behaviour (event-driven revocation
  // sync, future governance-aware features).
  const revocationRegistryId =
    process.env["REVOCATION_REGISTRY_ID"]?.trim() || undefined;
  const governanceId = process.env["GOVERNANCE_ID"]?.trim() || undefined;

  if (revocationRegistryId && !isValidSorobanContractId(revocationRegistryId)) {
    console.error(
      "Error: REVOCATION_REGISTRY_ID is not a valid Soroban contract address (must start with 'C' and be 56 characters).",
    );
    process.exit(1);
  }
  if (governanceId && !isValidSorobanContractId(governanceId)) {
    console.error(
      "Error: GOVERNANCE_ID is not a valid Soroban contract address (must start with 'C' and be 56 characters).",
    );
    process.exit(1);
  }

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

  // Validate all subjects at startup
  const validSubjects = subjects.filter(isValidStellarAddress);
  const invalidSubjects = subjects.filter((s) => !isValidStellarAddress(s));

  if (invalidSubjects.length > 0) {
    console.warn(
      `[feeder] Found ${invalidSubjects.length} invalid address(es) in SUBJECTS:`,
    );
    for (const invalid of invalidSubjects) {
      console.warn(`  - ${invalid}`);
    }
    console.warn(
      `[feeder] Processing ${validSubjects.length} valid subject(s)`,
    );
  }

  if (validSubjects.length === 0) {
    console.error(
      "Error: No valid Stellar addresses found in SUBJECTS. All addresses must start with 'G' and be 56 characters.",
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
  console.log(`  network    : ${network}`);
  console.log(`  subjects   : ${validSubjects.join(", ")}`);
  console.log(`  interval   : ${pollIntervalMs}ms`);
  console.log(`  rpc        : ${rpcUrl}`);
  console.log(`  horizon    : ${horizonUrl}`);
  console.log(`  maxRetries : ${maxRetries}`);
  console.log(`  retryBase  : ${retryBaseDelayMs}ms`);
  console.log(`  partialStats: ${allowPartialStats ? "submitted" : "suppressed"}`);
  const eventDriven = process.env["EVENT_DRIVEN"] === "true";
  const eventPollIntervalMsStr = process.env["EVENT_POLL_INTERVAL_MS"];
  const eventPollIntervalMs = eventPollIntervalMsStr
    ? parseInt(eventPollIntervalMsStr, 10)
    : 30_000;

  console.log("  optional integrations:");
  console.log(
    `    revocationRegistry : ${revocationRegistryId ?? "not configured"}`,
  );
  console.log(`    governance        : ${governanceId ?? "not configured"}`);
  console.log(`    eventDriven       : ${eventDriven}`);
  if (eventDriven) {
    console.log(`    eventPollInterval : ${eventPollIntervalMs}ms`);
  }

  const healthPort = parseHealthPort(process.env["HEALTH_PORT"]);
  const healthTracker = healthPort !== undefined ? new HealthTracker() : undefined;

  if (healthPort !== undefined && healthTracker) {
    createHealthServer(healthPort, healthTracker);
    console.log(`  healthPort  : ${healthPort} (/health, /ready)`);
  }

  const config: FeederConfig = {
    rpcUrl,
    horizonUrl,
    networkPassphrase,
    creditOracleId,
    identityOracleId,
    revocationRegistryId,
    governanceId,
    simAccount,
    subjects: validSubjects,
    pollIntervalMs,
    maxRetries,
    retryBaseDelayMs,
    allowPartialStats,
    network,
    eventDriven,
    eventPollIntervalMs,
    healthTracker,
  };

  const feeder = new Feeder(config, feederKeypair);
  feeder.start();
}
