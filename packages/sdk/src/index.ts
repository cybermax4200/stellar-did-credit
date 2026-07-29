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
} from "@stellar/stellar-sdk";

export const MIN_SCORE = 300;
export const MAX_SCORE = 850;

export interface ScoreRecord {
  score: number;
  lastUpdated: number;
  vcCount: number;
  repaymentRate: number;
  txVolume30d: bigint;
  previousScore: number | null;
  /** Ledger sequence number when this score was last computed.
   *  Compare against the current ledger to assess freshness. */
  computedAtLedger: number;
  /** Whether the score is considered stale. Computed at read time
   *  by comparing computedAtLedger against the current ledger sequence.
   *  Always false for a freshly computed score. */
  stale: boolean;
}

export interface TxStats {
  volume30d: bigint;
  txCount30d: number;
  avgCounterparties: number;
}
export interface ScoringWeights {
  vcWeight: number;
  txWeight: number;
  repaymentWeight: number;
}
export interface RepaymentRecord {
  onTimeCount: number;
  totalCount: number;
  totalRepaid: bigint;
}
export interface VCRecord {
  vcHash: Buffer;
  issuer: string;
  anchoredAt: number;
  revoked: boolean;
}

export interface ProtocolConfig {
  identityOracleId: string;
  creditOracleId: string;
  revocationRegistryId: string;
  networkPassphrase: string;
  rpcUrl: string;
  simAccount: string;
  timeoutSeconds?: number;
  maxRetries?: number;
  baseFee?: string;
}

export class StellarDIDCreditSDK {
  private server: SorobanRpc.Server;

  constructor(private config: ProtocolConfig) {
    this.server = new SorobanRpc.Server(this.config.rpcUrl);
  }

  /**
   * Anchor a DID document on-chain by storing its IPFS CID.
   *
   * Submits a signed transaction to the identity-oracle contract. Requires the subject
   * keypair to authorize the operation.
   *
   * @param subjectKeypair - Stellar keypair of the subject (private + public key)
   * @param didDocCid - IPFS CID of the DID document (e.g. "Qm...")
   * @param subjectAddress - Optional Stellar G... address of the subject for validation
   * @returns Transaction hash on successful submission
   * @throws Error if subjectAddress is provided and does not match subjectKeypair's public key
   */
  async anchorDID(
    subjectKeypair: any,
    didDocCid: string,
    subjectAddress?: string,
  ): Promise<string> {
    const publicKey =
      typeof subjectKeypair.publicKey === "function"
        ? subjectKeypair.publicKey()
        : subjectKeypair.publicKey;

    if (subjectAddress && publicKey !== subjectAddress) {
      throw new Error("subjectKeypair public key does not match subject");
    }

    const server = new SorobanRpc.Server(this.config.rpcUrl);
    const contract = new Contract(this.config.identityOracleId);

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
    const preparedTx = (SorobanRpc.Api as any).assembleTransaction(
      tx,
      sim,
    ).build();
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
   * @param vcHash - SHA-256 hash of the verifiable credential (must be exactly 32 bytes)
   * @returns Transaction hash on successful submission
   */
  async issueVC(
    issuerKeypair: any,
    subjectAddress: string,
    vcHash: Buffer,
  ): Promise<string> {
    if (vcHash.length !== 32) {
      throw new Error("vcHash must be exactly 32 bytes");
    }

    const server = new SorobanRpc.Server(this.config.rpcUrl);
    const contract = new Contract(this.config.identityOracleId);

    const publicKey =
      issuerKeypair.publicKey instanceof Function
        ? issuerKeypair.publicKey()
        : issuerKeypair.publicKey;

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
    const preparedTx = (SorobanRpc.Api as any).assembleTransaction(
      tx,
      sim,
    ).build();
    preparedTx.sign(issuerKeypair);

    // Submit to the network
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
   * **Note on Cooldowns:** The `compute_score` contract method is protected by a 
   * cooldown period (`ComputeCooldownLedgers`). If this method is called while the 
   * cooldown is active (or immediately after a fresh deployment before the initial 
   * cooldown has passed), the transaction will fail.
   *
   * @param payerKeypair - Stellar keypair paying the transaction fee
   * @param subjectAddress - Stellar G... address of the subject
   * @returns Persisted ScoreRecord after the compute_score transaction is confirmed
   * @throws Error if the transaction fails due to the cooldown period being active
   */
  async computeScore(
    payerKeypair: Keypair,
    subjectAddress: string,
  ): Promise<ScoreRecord> {
    const contract = new Contract(this.config.creditOracleId);

    const publicKey = payerKeypair.publicKey();

    const accountData = await this.server.getAccount(publicKey);
    const sourceAccount = new Account(publicKey, (accountData as any).sequence);

    const tx = new TransactionBuilder(sourceAccount, {
          fee: this.config.baseFee ?? BASE_FEE,
          networkPassphrase: this.config.networkPassphrase,
        })
      .addOperation(
        contract.call("compute_score", new Address(subjectAddress).toScVal()),
      )
      .setTimeout(this.config.timeoutSeconds ?? 30)
      .build();

    const sim = await this.server.simulateTransaction(tx);

    if (SorobanRpc.Api.isSimulationError(sim)) {
      if (sim.error && sim.error.toLowerCase().includes("cooldown")) {
        throw new Error(`computeScore failed: Cooldown period is active. Please wait for the cooldown ledgers to pass before recomputing the score.`);
      }
      throw new Error(`Simulation failed: ${sim.error}`);
    }

    if (!SorobanRpc.Api.isSimulationSuccess(sim)) {
      throw new Error("Simulation returned unexpected response");
    }

    const preparedTx = (SorobanRpc.Api as any).assembleTransaction(
      tx,
      sim,
    ).build();
    preparedTx.sign(payerKeypair);

    const response = await this.server.sendTransaction(preparedTx);

    if (response.status !== "PENDING") {
      if (response.errorResult && String(response.errorResult).toLowerCase().includes("cooldown")) {
        throw new Error(`Transaction submission failed: Cooldown period is active. Please wait for the cooldown ledgers to pass before recomputing the score.`);
      }
      throw new Error(`Transaction submission failed: ${String(response.errorResult)}`);
    }

    await waitForTransactionConfirmation(
      this.server,
      response.hash,
      "computeScore",
    );

    try {
      const score = await this.getScore(subjectAddress);
      if (!score) {
        throw new ScoreNotComputedError(subjectAddress);
      }
      return score;
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      throw new Error(
        `computeScore transaction succeeded and was confirmed, but fetching the stored score for ${subjectAddress} failed: ${message}`,
      );
    }
  }

  /**
   * Fetch the on-chain ScoreRecord for a subject address from the credit-oracle.
   *
   * Uses a read-only simulation (no signing required) against the configured RPC endpoint.
   *
   * @param subjectAddress - Stellar G... address of the subject
   * @returns Parsed ScoreRecord, or null if the score has not been computed
   */
  async getScore(subjectAddress: string): Promise<ScoreRecord | null> {
    const server = new SorobanRpc.Server(this.config.rpcUrl);
    const contract = new Contract(this.config.creditOracleId);
    const sourceAccount = new Account(this.config.simAccount, "0");

    let attempts = 0;
    const maxAttempts = (this.config.maxRetries ?? 0) + 1;

    while (attempts < maxAttempts) {
      const tx = new TransactionBuilder(sourceAccount, {
        fee: this.config.baseFee || BASE_FEE,
        networkPassphrase: this.config.networkPassphrase,
      })
        .addOperation(
          contract.call("get_score", new Address(subjectAddress).toScVal()),
        )
        .setTimeout(this.config.timeoutSeconds ?? 30)
        .build();

      const sim = await server.simulateTransaction(tx);

      if (SorobanRpc.Api.isSimulationError(sim)) {
        if (sim.error && sim.error.includes("score not computed")) {
          return null;
        }
        throw new Error(`Simulation failed: ${sim.error}`);
      }

      if (SorobanRpc.Api.isSimulationSuccess(sim)) {
        const resultScVal = sim.result?.retval;
        if (!resultScVal) {
          throw new Error("No return value in simulation result");
        }
        return parseScoreRecord(resultScVal, subjectAddress);
      }

      attempts++;
      if (attempts < maxAttempts) {
        await new Promise((resolve) => setTimeout(resolve, 500 * attempts));
      }
    }

    throw new Error("Simulation returned unexpected response");
  }

  /**
   * Fetch the DID document CID anchored for a subject address from the identity-oracle.
   *
   * Uses a read-only simulation (no signing required) against the configured RPC endpoint.
   *
   * @param subjectAddress - Stellar G... address of the subject
   * @returns The IPFS CID of the anchored DID document, or null if no DID is anchored
   */
  async getDIDDocument(subjectAddress: string): Promise<string | null> {
    const server = new SorobanRpc.Server(this.config.rpcUrl);
    const contract = new Contract(this.config.identityOracleId);
    const sourceAccount = new Account(this.config.simAccount, "0");

    const tx = new TransactionBuilder(sourceAccount, {
      fee: this.config.baseFee || BASE_FEE,
      networkPassphrase: this.config.networkPassphrase,
    })
      .addOperation(
        contract.call("get_did_document", new Address(subjectAddress).toScVal()),
      )
      .setTimeout(this.config.timeoutSeconds ?? 30)
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

    const native = scValToNative(resultScVal);
    // Option<String> — null/undefined means no DID anchored
    if (native === null || native === undefined) {
      return null;
    }
    return native as string;
  }

  /**
   * Revoke a verifiable credential by its hash.
   *
   * Submits one Soroban operation to the revocation-registry. The registry calls
   * `mark_vc_revoked` on the identity-oracle in the same contract invocation.
   * Soroban transactions execute atomically, so a failure in either contract
   * discards every state change made by the invocation.
   *
   * @see https://developers.stellar.org/docs/learn/fundamentals/contract-development/contract-interactions/transaction-simulation
   *
   * @param issuerKeypair - Stellar keypair of the credential issuer
   * @param subjectAddress - Stellar G... address of the credential subject
   * @param vcHash - SHA-256 hash of the verifiable credential (must be exactly 32 bytes)
   * @returns Transaction hash after successful ledger confirmation
   */
  async revokeVC(
    issuerKeypair: any,
    subjectAddress: string,
    vcHash: Buffer,
  ): Promise<string> {
    if (vcHash.length !== 32) {
      throw new Error("vcHash must be exactly 32 bytes");
    }

    const server = new SorobanRpc.Server(this.config.rpcUrl);
    const registryContract = new Contract(this.config.revocationRegistryId);

    const publicKey =
      issuerKeypair.publicKey instanceof Function
        ? issuerKeypair.publicKey()
        : issuerKeypair.publicKey;

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
        registryContract.call(
          "revoke",
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
      throw new Error(
        `revokeVC simulation failed; no revocation state was changed: ${sim.error}`,
      );
    }

    if (!SorobanRpc.Api.isSimulationSuccess(sim)) {
      throw new Error(
        "revokeVC simulation returned an unexpected response; no revocation state was changed",
      );
    }

    // Apply simulation result and prepare the transaction
    const preparedTx = (SorobanRpc.Api as any).assembleTransaction(
      tx,
      sim,
    ).build();
    preparedTx.sign(issuerKeypair);

    // Submit to the network
    const response = await server.sendTransaction(preparedTx);

    if (response.status !== "PENDING") {
      throw new Error(
        `revokeVC submission failed; no revocation was applied: ${response.errorResult}`,
      );
    }

    try {
      await waitForTransactionConfirmation(server, response.hash, "revokeVC");
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      throw new Error(
        `revokeVC failed; the atomic transaction rolled back both registry and identity-oracle changes: ${message}`,
      );
    }

    return response.hash;
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
   * Verify whether a subject has a matching active verifiable credential anchor.
   *
   * Uses a read-only simulation against the identity-oracle contract.
   *
   * @param subjectAddress - Stellar G... address of the credential subject
   * @param vcHash - SHA-256 hash of the verifiable credential (must be exactly 32 bytes)
   * @returns true if the subject has an active, non-revoked VC with the given hash
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

    return scValToNative(resultScVal) as boolean;
  }

  /**
   * Returns the number of active (non-revoked) verifiable credentials for a subject.
   *
   * Uses a read-only simulation against the identity-oracle contract.
   *
   * @param subjectAddress - Stellar G... address of the subject
   * @returns The count of active non-revoked VCs
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
        contract.call("get_active_vc_count", new Address(subjectAddress).toScVal()),
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

    return scValToNative(resultScVal) as number;
  }

  /**
   * Fetch the scoring weights currently configured on the credit-oracle contract.
   *
   * Uses a read-only simulation (no signing required).
   *
   * @returns The current ScoringWeights configuration
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
   * Returns the list of all currently registered (non-deregistered) trusted issuers.
   *
   * Uses a read-only simulation against the identity-oracle contract.
   *
   * @returns Array of Stellar G... addresses of registered issuers
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

    const native = scValToNative(resultScVal);
    return (native as unknown[]).map((addr) => String(addr));
  }
}

/** Thrown when get_score is called for an address that has no computed score yet. */
export class ScoreNotComputedError extends Error {
  constructor(address?: string) {
    super(address ? `No score computed for address: ${address}` : "Score has not been computed");
    this.name = "ScoreNotComputedError";
  }
}

/**
 * Parse a Soroban ScVal representing an Option<ScoreRecord>.
 * Returns the ScoreRecord if Some, returns null if None.
 */
function parseScoreRecord(scVal: xdr.ScVal, subjectAddress: string): ScoreRecord | null {
  const native = scValToNative(scVal);
  // Option::None is represented as null/undefined by scValToNative
  if (native === null || native === undefined) {
    return null;
  }
  const raw = native as Record<string, unknown>;
  return {
    score: Number(raw["score"]),
    lastUpdated: Number(raw["last_updated"]),
    vcCount: Number(raw["vc_count"]),
    repaymentRate: Number(raw["repayment_rate"]),
    txVolume30d: BigInt(raw["tx_volume_30d"] as bigint),
    previousScore:
      raw["previous_score"] != null
        ? Number(raw["previous_score"])
        : null,
    computedAtLedger: Number(raw["computed_at_ledger"]),
    stale: Boolean(raw["stale"]),
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
  operationName: string,
  attempts = 20,
  delayMs = 1000,
): Promise<void> {
  for (let attempt = 0; attempt < attempts; attempt++) {
    const result = await server.getTransaction(txHash);

    const status = result.status as string;

    switch (status) {
      case "SUCCESS":
        return;
      case "FAILED": {
        const errorDetails = JSON.stringify(result);
        throw new Error(
          `${operationName} transaction failed for ${txHash}: ${errorDetails}`,
        );
      }
      case "NOT_FOUND":
      case "PENDING":
        await sleep(delayMs);
        break;
      default:
        throw new Error(
          `Unexpected transaction status for ${txHash}: ${String((result as unknown as { status: string }).status)}`,
        );
    }
  }

  throw new Error(
    `Timed out waiting for ${operationName} transaction confirmation: ${txHash}`,
  );
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

export default StellarDIDCreditSDK;
