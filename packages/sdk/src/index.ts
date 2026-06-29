import {
  Contract,
  SorobanRpc,
  TransactionBuilder,
  Networks,
  BASE_FEE,
  Account,
  scValToNative,
  nativeToScVal,
  Address,
  xdr,
  Keypair,
} from "@stellar/stellar-sdk";

export const MIN_SCORE = 300;
export const MAX_SCORE = 850;

/**
 * Credit score record returned by the credit-oracle `get_score` entrypoint.
 *
 * Maps to the `ScoreRecord` struct in `contracts/credit-oracle`.
 */
export interface ScoreRecord {
  /** Credit score value, bounded to {@link MIN_SCORE}–{@link MAX_SCORE}. Soroban `u32`. */
  score: number;
  /** Ledger timestamp (Unix seconds) of the last score computation. Soroban `u64`. */
  lastUpdated: number;
  /** Number of verified credentials counted toward the score. Soroban `u32`. */
  vcCount: number;
  /** Repayment rate in basis points (0–10000). Soroban `u32`. */
  repaymentRate: number;
  /** 30-day transaction volume in stroops. Soroban `i128`. */
  txVolume30d: bigint;
}

/**
 * Transaction statistics for a subject, as stored by the credit-oracle contract
 * and supplied by a trusted feeder via `update_tx_stats`.
 *
 * All fields contribute to the scoring formula. See docs/scoring-spec.md for details.
 */
export interface TxStats {
  /** Total transaction volume over the last 30 days, in stroops. Soroban `i128`. */
  volume30d: bigint;
  /** Number of transactions in the last 30 days. Currently unused. Soroban `u32`. */
  txCount30d: number;
  /** Average number of distinct counterparties. Awards up to 10 bonus points when >= 10.
   *  Soroban `u32`.
   */
  avgCounterparties: number;
}

/**
 * Weights used by the credit-oracle when computing a composite credit score.
 * By contract invariant the three weights always sum to 100.
 *
 * Maps to the `ScoringWeights` struct in `contracts/credit-oracle`.
 */
export interface ScoringWeights {
  /** Weight applied to the verified-credentials component (0–100). Soroban `u32`. */
  vcWeight: number;
  /** Weight applied to the transaction-history component (0–100). Soroban `u32`. */
  txWeight: number;
  /** Weight applied to the repayment-history component (0–100). Soroban `u32`. */
  repaymentWeight: number;
}

/**
 * Repayment counters tracked per subject by the credit-oracle contract and
 * updated by trusted lenders via `record_repayment`.
 *
 * Maps to the `RepaymentRecord` struct in `contracts/credit-oracle`.
 */
export interface RepaymentRecord {
  /** Number of repayments made on time. Soroban `u32`. */
  onTimeCount: number;
  /** Total number of recorded repayments. Soroban `u32`. */
  totalCount: number;
}

/**
 * On-chain anchor record for a verifiable credential, as stored by the
 * identity-oracle contract and created via `anchor_vc`.
 *
 * Maps to the `VCRecord` struct in `contracts/identity-oracle`.
 */
export interface VCRecord {
  /** SHA-256 hash of the off-chain verifiable credential JSON. Soroban `BytesN<32>`. */
  vcHash: Buffer;
  /** Stellar address (G...) of the issuer that anchored this credential. Soroban `Address`. */
  issuer: string;
  /** Ledger timestamp (Unix seconds) when the credential was anchored. Soroban `u64`. */
  anchoredAt: number;
  /** Whether the credential has been revoked by its issuer. Soroban `bool`. */
  revoked: boolean;
}

export interface ProtocolConfig {
  identityOracleId: string;
  creditOracleId: string;
  revocationRegistryId: string;
  networkPassphrase: string;
  rpcUrl: string;
  simAccount: string;
}

export class StellarDIDCreditSDK {
  constructor(private config: ProtocolConfig) {}

  /**
   * Anchor a DID document on-chain by storing its IPFS CID.
   *
   * Submits a signed transaction to the identity-oracle contract. Requires the subject
   * keypair to authorize the operation.
   *
   * @param subjectKeypair - Stellar keypair of the subject (private + public key)
   * @param didDocCid - IPFS CID of the DID document (e.g. "Qm...")
   * @returns Transaction hash on successful submission
   */
  async anchorDID(subjectKeypair: Keypair, didDocCid: string): Promise<string> {
    const server = new SorobanRpc.Server(this.config.rpcUrl);
    const contract = new Contract(this.config.identityOracleId);

    const publicKey = subjectKeypair.publicKey();

    // Get the current account sequence number
    const accountData = await server.getAccount(publicKey);
    const sourceAccount = new Account(publicKey, (accountData as any).sequence);

    const tx = new TransactionBuilder(sourceAccount, {
      fee: BASE_FEE,
      networkPassphrase: this.config.networkPassphrase,
    })
      .addOperation(
        contract.call(
          "anchor_did",
          new Address(publicKey).toScVal(),
          nativeToScVal(didDocCid),
        ),
      )
      .setTimeout(30)
      .build();

    // Simulate to ensure the call succeeds
    const sim = await server.simulateTransaction(tx);

    if (SorobanRpc.Api.isSimulationError(sim)) {
      throw new Error(`Simulation failed: ${sim.error}`);
    }

    if (!SorobanRpc.Api.isSimulationSuccess(sim)) {
      throw new Error("Simulation returned unexpected response");
    }

    // Apply simulation result and prepare the transaction
    const preparedTx = (SorobanRpc.Api as any)
      .assembleTransaction(tx, sim)
      .build();
    preparedTx.sign(subjectKeypair);

    // Submit to the network
    const response = await server.sendTransaction(preparedTx);

    if (response.status !== "PENDING") {
      throw new Error(`Transaction submission failed: ${response.errorResult}`);
    }

    return response.hash;
  }

  /**
   * Issue a verifiable credential by anchoring its hash on-chain.
   *
   * Submits a signed transaction to the identity-oracle contract. Requires the issuer
   * keypair to authorize the operation. The issuer must be registered with the contract.
   *
   * @param issuerKeypair - Stellar keypair of the credential issuer
   * @param subjectAddress - Stellar G... address of the credential subject
   * @param vcHash - SHA-256 hash of the verifiable credential
   * @returns Transaction hash on successful submission
   */
  async issueVC(
    issuerKeypair: Keypair,
    subjectAddress: string,
    vcHash: Buffer,
  ): Promise<string> {
    const server = new SorobanRpc.Server(this.config.rpcUrl);
    const contract = new Contract(this.config.identityOracleId);

    const publicKey = issuerKeypair.publicKey();

    // Get the current account sequence number
    const accountData = await server.getAccount(publicKey);
    const sourceAccount = new Account(publicKey, (accountData as any).sequence);

    // Convert vcHash Buffer to ScVal
    const hashScVal = nativeToScVal(new Uint8Array(vcHash), { type: "bytes" });

    const tx = new TransactionBuilder(sourceAccount, {
      fee: BASE_FEE,
      networkPassphrase: this.config.networkPassphrase,
    })
      .addOperation(
        contract.call(
          "anchor_vc",
          new Address(publicKey).toScVal(),
          new Address(subjectAddress).toScVal(),
          hashScVal,
        ),
      )
      .setTimeout(30)
      .build();

    // Simulate to ensure the call succeeds
    const sim = await server.simulateTransaction(tx);

    if (SorobanRpc.Api.isSimulationError(sim)) {
      throw new Error(`Simulation failed: ${sim.error}`);
    }

    if (!SorobanRpc.Api.isSimulationSuccess(sim)) {
      throw new Error("Simulation returned unexpected response");
    }

    // Apply simulation result and prepare the transaction
    const preparedTx = (SorobanRpc.Api as any)
      .assembleTransaction(tx, sim)
      .build();
    preparedTx.sign(issuerKeypair);

    // Submit to the network
    const response = await server.sendTransaction(preparedTx);

    if (response.status !== "PENDING") {
      throw new Error(`Transaction submission failed: ${response.errorResult}`);
    }

    return response.hash;
  }

  /**
   * Revoke a verifiable credential on-chain.
   *
   * Submits a single signed transaction that calls `revoke` on the revocation-registry
   * contract and `mark_vc_revoked` on the identity-oracle contract. Requires the issuer
   * keypair to authorize both operations.
   *
   * @param issuerKeypair - Stellar keypair of the credential issuer
   * @param subjectAddress - Stellar G... address of the credential subject
   * @param vcHash - SHA-256 hash of the verifiable credential to revoke
   * @returns Transaction hash on successful submission
   */
  async revokeVC(
    issuerKeypair: Keypair,
    subjectAddress: string,
    vcHash: Buffer,
  ): Promise<string> {
    if (vcHash.length !== 32) {
      throw new Error("vcHash must be exactly 32 bytes");
    }

    const server = new SorobanRpc.Server(this.config.rpcUrl);
    const revocationContract = new Contract(this.config.revocationRegistryId);
    const identityContract = new Contract(this.config.identityOracleId);

    const publicKey = issuerKeypair.publicKey();

    const accountData = await server.getAccount(publicKey);
    const sourceAccount = new Account(publicKey, (accountData as any).sequence);

    const hashScVal = nativeToScVal(new Uint8Array(vcHash), { type: "bytes" });
    const issuerScVal = new Address(publicKey).toScVal();

    const tx = new TransactionBuilder(sourceAccount, {
      fee: BASE_FEE,
      networkPassphrase: this.config.networkPassphrase,
    })
      .addOperation(
        revocationContract.call("revoke", issuerScVal, hashScVal),
      )
      .addOperation(
        identityContract.call(
          "mark_vc_revoked",
          issuerScVal,
          new Address(subjectAddress).toScVal(),
          hashScVal,
        ),
      )
      .setTimeout(30)
      .build();

    const sim = await server.simulateTransaction(tx);

    if (SorobanRpc.Api.isSimulationError(sim)) {
      throw new Error(`Simulation failed: ${sim.error}`);
    }

    if (!SorobanRpc.Api.isSimulationSuccess(sim)) {
      throw new Error("Simulation returned unexpected response");
    }

    const preparedTx = (SorobanRpc.Api as any)
      .assembleTransaction(tx, sim)
      .build();
    preparedTx.sign(issuerKeypair);

    const response = await server.sendTransaction(preparedTx);

    if (response.status !== "PENDING") {
      throw new Error(`Transaction submission failed: ${response.errorResult}`);
    }

    return response.hash;
  }

  /**
   * Compute and persist a subject's credit score, then return the stored ScoreRecord.
   *
   * Submits a signed transaction to the credit-oracle contract, waits for ledger
   * confirmation, then fetches the persisted score via `getScore`.
   *
   * @param payerKeypair - Stellar keypair paying the transaction fee
   * @param subjectAddress - Stellar G... address of the subject
   * @returns Persisted ScoreRecord after the compute_score transaction is confirmed
   */
  async computeScore(
    payerKeypair: Keypair,
    subjectAddress: string,
  ): Promise<ScoreRecord> {
    const server = new SorobanRpc.Server(this.config.rpcUrl);
    const contract = new Contract(this.config.creditOracleId);

    const publicKey = payerKeypair.publicKey();

    const accountData = await server.getAccount(publicKey);
    const sourceAccount = new Account(publicKey, (accountData as any).sequence);

    const tx = new TransactionBuilder(sourceAccount, {
      fee: BASE_FEE,
      networkPassphrase: this.config.networkPassphrase,
    })
      .addOperation(
        contract.call("compute_score", new Address(subjectAddress).toScVal()),
      )
      .setTimeout(30)
      .build();

    const sim = await server.simulateTransaction(tx);

    if (SorobanRpc.Api.isSimulationError(sim)) {
      throw new Error(`Simulation failed: ${sim.error}`);
    }

    if (!SorobanRpc.Api.isSimulationSuccess(sim)) {
      throw new Error("Simulation returned unexpected response");
    }

    const preparedTx = (SorobanRpc.Api as any)
      .assembleTransaction(tx, sim)
      .build();
    preparedTx.sign(payerKeypair);

    const response = await server.sendTransaction(preparedTx);

    if (response.status !== "PENDING") {
      throw new Error(`Transaction submission failed: ${response.errorResult}`);
    }

    await waitForTransactionConfirmation(server, response.hash);

    try {
      return await this.getScore(subjectAddress);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      throw new Error(
        `computeScore transaction succeeded, but fetching the stored score for ${subjectAddress} failed: ${message}`,
      );
    }
  }

  /**
   * Fetch the on-chain ScoreRecord for a subject address from the credit-oracle.
   *
   * Uses a read-only simulation (no signing required) against the configured RPC endpoint.
   *
   * @param subjectAddress - Stellar G... address of the subject
   * @returns Parsed ScoreRecord
   * @throws {ScoreNotComputedError} If the score has not been computed for this address
   */
  async getScore(subjectAddress: string): Promise<ScoreRecord> {
    // 1. Create RPC server
    const server = new SorobanRpc.Server(this.config.rpcUrl);

    // 2. Instantiate the credit-oracle contract
    const contract = new Contract(this.config.creditOracleId);

    // 3. Build a read-only transaction — use a well-known funded account as the fee source
    //    for simulation; no actual submission occurs.
    const sourceAccount = new Account(this.config.simAccount, "0");
    const tx = new TransactionBuilder(sourceAccount, {
      fee: BASE_FEE,
      networkPassphrase: this.config.networkPassphrase,
    })
      .addOperation(
        contract.call("get_score", new Address(subjectAddress).toScVal()),
      )
      .setTimeout(30)
      .build();

    // 4. Simulate to get the return value without submitting
    const sim = await server.simulateTransaction(tx);

    if (SorobanRpc.Api.isSimulationError(sim)) {
      if (sim.error && sim.error.includes("score not computed")) {
        throw new ScoreNotComputedError(subjectAddress);
      }
      throw new Error(`Simulation failed: ${sim.error}`);
    }

    if (!SorobanRpc.Api.isSimulationSuccess(sim)) {
      throw new Error("Simulation returned unexpected response");
    }

    const resultScVal = sim.result?.retval;
    if (!resultScVal) {
      throw new Error("No return value in simulation result");
    }

    // 5. Parse the ScoreRecord struct.
    //    Soroban structs are returned as ScMap with symbol keys.
    return parseScoreRecord(resultScVal, subjectAddress);
  }

  /**
   * Fetch the current scoring weights from the credit-oracle.
   *
   * Uses a read-only simulation (no signing required) against the configured RPC endpoint.
   *
   * @returns Parsed ScoringWeights
   */
  async getWeights(): Promise<ScoringWeights> {
    const server = new SorobanRpc.Server(this.config.rpcUrl);
    const contract = new Contract(this.config.creditOracleId);

    const sourceAccount = new Account(this.config.simAccount, "0");
    const tx = new TransactionBuilder(sourceAccount, {
      fee: BASE_FEE,
      networkPassphrase: this.config.networkPassphrase,
    })
      .addOperation(contract.call("get_scoring_weights"))
      .setTimeout(30)
      .build();

    const sim = await server.simulateTransaction(tx);

    if (SorobanRpc.Api.isSimulationError(sim)) {
      throw new Error(`Simulation failed: ${sim.error}`);
    }

    if (!SorobanRpc.Api.isSimulationSuccess(sim)) {
      throw new Error("Simulation returned unexpected response");
    }

    const resultScVal = sim.result?.retval;
    if (!resultScVal) {
      throw new Error("No return value in simulation result");
    }

    return parseScoringWeights(resultScVal);
  }

  /**
   * Retrieve the anchored DID document CID for a subject.
   *
   * Returns the IPFS CID of the subject's anchored DID document, or `null` if
   * no DID document has been anchored for this subject. Uses a read-only
   * simulation — no signing or fees required.
   *
   * @param subjectAddress - Stellar address (G...) of the DID subject
   * @returns IPFS CID string (e.g. "Qm...") if anchored, `null` otherwise
   */
  async getDIDDocument(subjectAddress: string): Promise<string | null> {
    const server = new SorobanRpc.Server(this.config.rpcUrl);
    const contract = new Contract(this.config.identityOracleId);

    const sourceAccount = new Account(this.config.simAccount, "0");

    const tx = new TransactionBuilder(sourceAccount, {
      fee: BASE_FEE,
      networkPassphrase: this.config.networkPassphrase,
    })
      .addOperation(
        contract.call(
          "get_did_document",
          new Address(subjectAddress).toScVal(),
        ),
      )
      .setTimeout(30)
      .build();

    const sim = await server.simulateTransaction(tx);

    if (SorobanRpc.Api.isSimulationError(sim)) {
      throw new Error(`Simulation failed: ${sim.error}`);
    }

    if (!SorobanRpc.Api.isSimulationSuccess(sim)) {
      throw new Error("Simulation returned unexpected response");
    }

    if (!sim.result?.retval) {
      throw new Error("No return value in simulation result");
    }

    const native = scValToNative(sim.result.retval);
    // Option::None is represented as null/undefined by scValToNative
    if (native === null || native === undefined) {
      return null;
    }

    return native as string;
  }

  /**
   * Verify that a specific VC hash is valid and not revoked for the given subject.
   *
   * Uses a read-only simulation against the identity-oracle contract.
   *
   * @param subjectAddress - Stellar G... address of the subject
   * @param vcHash - SHA-256 hash of the credential
   * @returns true if the credential is valid and not revoked
   */
  async verifyVC(subjectAddress: string, vcHash: Buffer): Promise<boolean> {
    if (vcHash.length !== 32) {
      throw new Error("vcHash must be exactly 32 bytes");
    }

    const server = new SorobanRpc.Server(this.config.rpcUrl);
    const contract = new Contract(this.config.identityOracleId);

    const sourceAccount = new Account(this.config.simAccount, "0");
    const hashScVal = nativeToScVal(new Uint8Array(vcHash), { type: "bytes" });

    const tx = new TransactionBuilder(sourceAccount, {
      fee: BASE_FEE,
      networkPassphrase: this.config.networkPassphrase,
    })
      .addOperation(
        contract.call(
          "verify_vc",
          new Address(subjectAddress).toScVal(),
          hashScVal,
        ),
      )
      .setTimeout(30)
      .build();

    const sim = await server.simulateTransaction(tx);

    if (SorobanRpc.Api.isSimulationError(sim)) {
      throw new Error(`Simulation failed: ${sim.error}`);
    }

    if (!SorobanRpc.Api.isSimulationSuccess(sim)) {
      throw new Error("Simulation returned unexpected response");
    }

    const resultScVal = sim.result?.retval;
    if (!resultScVal) {
      throw new Error("No return value in simulation result");
    }

    const result = scValToNative(resultScVal);
    if (typeof result !== "boolean") {
      throw new Error("verify_vc returned a non-boolean result");
    }

    return result;
  }

  /**
   * Check if a subject address has at least one non-revoked verifiable credential.
   *
   * Uses a read-only simulation against the identity-oracle contract.
   *
   * @param subjectAddress - Stellar G... address of the subject
   * @returns true if subject has ≥ 1 non-revoked credential
   */
  async isVerified(subjectAddress: string): Promise<boolean> {
    const server = new SorobanRpc.Server(this.config.rpcUrl);
    const contract = new Contract(this.config.identityOracleId);

    const sourceAccount = new Account(this.config.simAccount, "0");
    const tx = new TransactionBuilder(sourceAccount, {
      fee: BASE_FEE,
      networkPassphrase: this.config.networkPassphrase,
    })
      .addOperation(
        contract.call("is_verified", new Address(subjectAddress).toScVal()),
      )
      .setTimeout(30)
      .build();

    const sim = await server.simulateTransaction(tx);

    if (SorobanRpc.Api.isSimulationError(sim)) {
      throw new Error(`Simulation failed: ${sim.error}`);
    }

    if (!SorobanRpc.Api.isSimulationSuccess(sim)) {
      throw new Error("Simulation returned unexpected response");
    }

    const resultScVal = sim.result?.retval;
    if (!resultScVal) {
      throw new Error("No return value in simulation result");
    }

    return scValToNative(resultScVal) as boolean;
  }

  /**
   * Get the number of active (non-revoked) verifiable credentials for a subject.
   *
   * Uses a read-only simulation against the identity-oracle contract.
   *
   * @param subjectAddress - Stellar G... address of the subject
   * @returns Count of non-revoked anchored VCs
   */
  async getVCCount(subjectAddress: string): Promise<number> {
    const server = new SorobanRpc.Server(this.config.rpcUrl);
    const contract = new Contract(this.config.identityOracleId);

    const sourceAccount = new Account(this.config.simAccount, "0");
    const tx = new TransactionBuilder(sourceAccount, {
      fee: BASE_FEE,
      networkPassphrase: this.config.networkPassphrase,
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
      throw new Error(`Simulation failed: ${sim.error}`);
    }

    if (!SorobanRpc.Api.isSimulationSuccess(sim)) {
      throw new Error("Simulation returned unexpected response");
    }

    const resultScVal = sim.result?.retval;
    if (!resultScVal) {
      throw new Error("No return value in simulation result");
    }

    return Number(scValToNative(resultScVal));
  }

  /**
   * Fetch the currently registered trusted issuers from the identity-oracle.
   *
   * Uses a read-only simulation against the identity-oracle contract.
   *
   * @returns Stellar G... addresses of registered issuers
   */
  async getRegisteredIssuers(): Promise<string[]> {
    const server = new SorobanRpc.Server(this.config.rpcUrl);
    const contract = new Contract(this.config.identityOracleId);

    const sourceAccount = new Account(this.config.simAccount, "0");
    const tx = new TransactionBuilder(sourceAccount, {
      fee: BASE_FEE,
      networkPassphrase: this.config.networkPassphrase,
    })
      .addOperation(contract.call("list_issuers"))
      .setTimeout(30)
      .build();

    const sim = await server.simulateTransaction(tx);

    if (SorobanRpc.Api.isSimulationError(sim)) {
      throw new Error(`Simulation failed: ${sim.error}`);
    }

    if (!SorobanRpc.Api.isSimulationSuccess(sim)) {
      throw new Error("Simulation returned unexpected response");
    }

    const resultScVal = sim.result?.retval;
    if (!resultScVal) {
      throw new Error("No return value in simulation result");
    }

    const issuers = scValToNative(resultScVal);
    if (!Array.isArray(issuers)) {
      throw new Error("list_issuers returned a non-array result");
    }

    return issuers.map((issuer) => String(issuer));
  }
}

/** Thrown when get_score is called for an address that has no computed score yet. */
export class ScoreNotComputedError extends Error {
  constructor(address?: string) {
    super(
      address
        ? `No score computed for address: ${address}`
        : "Score has not been computed",
    );
    this.name = "ScoreNotComputedError";
  }
}

/**
 * Parse a Soroban ScVal representing an Option<ScoreRecord>.
 * Returns the ScoreRecord if Some, throws ScoreNotComputedError if None.
 */
function parseScoreRecord(
  scVal: xdr.ScVal,
  subjectAddress: string,
): ScoreRecord {
  const native = scValToNative(scVal);
  // Option::None is represented as null/undefined by scValToNative
  if (native === null || native === undefined) {
    throw new ScoreNotComputedError(subjectAddress);
  }
  const raw = native as Record<string, unknown>;
  return {
    score: Number(raw["score"]),
    lastUpdated: Number(raw["last_updated"]),
    vcCount: Number(raw["vc_count"]),
    repaymentRate: Number(raw["repayment_rate"]),
    txVolume30d: BigInt(raw["tx_volume_30d"] as bigint),
  };
}

function parseScoringWeights(scVal: xdr.ScVal): ScoringWeights {
  const native = scValToNative(scVal);
  if (native === null || native === undefined || typeof native !== "object") {
    throw new Error("get_scoring_weights returned an invalid result");
  }

  const raw = native as Record<string, unknown>;
  return {
    vcWeight: Number(raw["vc_weight"]),
    txWeight: Number(raw["tx_weight"]),
    repaymentWeight: Number(raw["repayment_weight"]),
  };
}

async function waitForTransactionConfirmation(
  server: SorobanRpc.Server,
  txHash: string,
  attempts = 20,
  delayMs = 1000,
): Promise<void> {
  for (let attempt = 0; attempt < attempts; attempt++) {
    const result = await server.getTransaction(txHash);

    switch (result.status) {
      case "SUCCESS":
        return;
      case "FAILED": {
        const errorDetails = JSON.stringify(result);
        throw new Error(
          `computeScore transaction failed for ${txHash}: ${errorDetails}`,
        );
      }
      case "NOT_FOUND":
      case "PENDING":
        await sleep(delayMs);
        break;
      default:
        throw new Error(
          `Unexpected transaction status for ${txHash}: ${String(result.status)}`,
        );
    }
  }

  throw new Error(
    `Timed out waiting for computeScore transaction confirmation: ${txHash}`,
  );
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

export default StellarDIDCreditSDK;
