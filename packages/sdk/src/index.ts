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

export interface GovernanceProposal {
  id: bigint;
  proposer: string;
  proposedWeights: ScoringWeights;
  votesFor: bigint;
  votesAgainst: bigint;
  expiryLedger: number;
  executionDelayLedgers: number;
  executed: boolean;
  cancelled: boolean;
  quorumRequired: bigint;
}

export interface ProtocolConfig {
  identityOracleId: string;
  creditOracleId: string;
  revocationRegistryId: string;
  governanceId?: string;
  networkPassphrase: string;
  rpcUrl: string;
  simAccount: string;
  timeoutSeconds?: number;
  maxRetries?: number;
  baseFee?: string;
  confirmationTimeoutMs?: number;
  pollIntervalMs?: number;
}

export type SDKErrorCode =
  | "INVALID_VC_HASH"
  | "MISSING_REVOCATION_REGISTRY"
  | "NOT_REGISTERED_ISSUER"
  | "TRANSACTION_FAILED"
  | "TRANSACTION_TIMEOUT";

export class SDKError extends Error {
  constructor(
    public readonly code: SDKErrorCode,
    message: string,
    options?: { cause?: unknown },
  ) {
    super(message);
    if (options?.cause !== undefined) {
      this.cause = options.cause;
    }
    this.name = "SDKError";
  }

  declare readonly cause?: unknown;
}

/** A Stellar keypair, or a minimal object exposing a public key. */
export type KeypairLike = Keypair | { publicKey: string };

export type GovernanceInteger = number | bigint;

/**
 * Client for the governance contract's proposal and weight-management flow.
 *
 * A successful proposal passes through two independent timelocks: the
 * governance execution delay, followed by the credit-oracle's fixed
 * approximately 24-hour timelock. Callers must wait for the first delay before
 * `execute`, then wait for the credit-oracle pending record's effective ledger
 * before calling `applyWeights`.
 */
export class GovernanceClient {
  private readonly server: SorobanRpc.Server;

  constructor(
    private readonly config: ProtocolConfig,
    server?: SorobanRpc.Server,
  ) {
    this.server = server ?? new SorobanRpc.Server(config.rpcUrl);
  }

  /**
   * Create a scoring-weight proposal and return its on-chain ID.
   *
   * This starts the governance voting window. If the proposal passes, callers
   * must still wait for its governance execution delay, call `execute`, wait
   * approximately 24 hours for the credit-oracle timelock, and then call
   * `applyWeights` before the new weights become active.
   */
  async createProposal(
    proposerKeypair: Keypair,
    weights: ScoringWeights,
    votingPeriodLedgers: number,
    executionDelayLedgers: number,
  ): Promise<bigint> {
    const proposer = getPublicKey(proposerKeypair);
    const contract = this.governanceContract();
    const result = await this.submitSignedTransaction(
      proposerKeypair,
      contract.call(
        "create_proposal",
        new Address(proposer).toScVal(),
        scoringWeightsToScVal(weights),
        nativeToScVal(votingPeriodLedgers, { type: "u32" }),
        nativeToScVal(executionDelayLedgers, { type: "u32" }),
      ),
      "createProposal",
    );

    if (!result.retval) {
      throw new Error("createProposal returned no proposal ID");
    }
    return BigInt(scValToNative(result.retval) as bigint | number | string);
  }

  /**
   * Cast a weighted vote on an open proposal.
   *
   * Voting only affects the proposal's governance phase. A successful vote
   * does not start the credit-oracle timelock; that happens only after the
   * voting period and governance execution delay have elapsed and `execute`
   * succeeds.
   */
  async vote(
    voterKeypair: Keypair,
    proposalId: GovernanceInteger,
    voteFor: boolean,
    voteWeight: GovernanceInteger,
  ): Promise<string> {
    const voter = getPublicKey(voterKeypair);
    const contract = this.governanceContract();
    return (
      await this.submitSignedTransaction(
        voterKeypair,
        contract.call(
          "vote",
          new Address(voter).toScVal(),
          nativeToScVal(toUnsignedBigInt(proposalId), { type: "u64" }),
          nativeToScVal(voteFor),
          nativeToScVal(toPositiveBigInt(voteWeight), { type: "i128" }),
        ),
        "vote",
      )
    ).hash;
  }

  /**
   * Execute a proposal after voting and the governance execution delay finish.
   *
   * For a passing proposal, this queues the new weights in the credit-oracle;
   * it does not activate them. Callers must wait approximately 24 hours, or
   * until the credit-oracle pending record's `effective_ledger`, before calling
   * `applyWeights` to complete the second timelock.
   */
  async execute(
    payerKeypair: Keypair,
    proposalId: GovernanceInteger,
  ): Promise<string> {
    const contract = this.governanceContract();
    return (
      await this.submitSignedTransaction(
        payerKeypair,
        contract.call(
          "execute",
          nativeToScVal(toUnsignedBigInt(proposalId), { type: "u64" }),
        ),
        "execute",
      )
    ).hash;
  }

  /**
   * Apply weights queued by a previously executed passing proposal.
   *
   * This call must be made only after the credit-oracle's fixed timelock has
   * expired, approximately 24 hours after `execute` at the normal five-second
   * ledger cadence. Calling earlier is rejected by the credit-oracle.
   */
  async applyWeights(payerKeypair: Keypair): Promise<string> {
    const contract = this.governanceContract();
    return (
      await this.submitSignedTransaction(
        payerKeypair,
        contract.call("apply_weights"),
        "applyWeights",
      )
    ).hash;
  }

  /**
   * Fetch one governance proposal, or `null` when the ID is not present.
   *
   * An `executed` proposal may still have weights pending in the
   * credit-oracle. The new weights become active only after the second,
   * approximately 24-hour timelock and a successful `applyWeights` call.
   */
  async getProposal(
    proposalId: GovernanceInteger,
  ): Promise<GovernanceProposal | null> {
    const contract = this.governanceContract();
    const retval = await this.simulateRead(
      contract.call(
        "get_proposal",
        nativeToScVal(toUnsignedBigInt(proposalId), { type: "u64" }),
      ),
    );
    const native = scValToNative(retval);
    return native === null || native === undefined
      ? null
      : parseGovernanceProposal(native);
  }

  /**
   * Fetch proposals by scanning the contract's monotonically increasing IDs.
   *
   * The governance contract exposes `get_proposal`, but no list endpoint, so
   * this helper performs up to `limit` read-only RPC simulations starting at
   * `fromId` and omits IDs that no longer have stored proposals.
   * Proposal execution state does not imply active weights until the
   * credit-oracle timelock has elapsed and `applyWeights` succeeds.
   */
  async listProposals(
    fromId: GovernanceInteger,
    limit: number,
  ): Promise<GovernanceProposal[]> {
    if (!Number.isInteger(limit) || limit < 0) {
      throw new Error("limit must be a non-negative integer");
    }

    const firstId = toUnsignedBigInt(fromId);
    const proposals: GovernanceProposal[] = [];
    for (let offset = 0n; offset < BigInt(limit); offset += 1n) {
      const proposal = await this.getProposal(firstId + offset);
      if (proposal) {
        proposals.push(proposal);
      }
    }
    return proposals;
  }

  private governanceContract(): Contract {
    if (!this.config.governanceId?.trim()) {
      throw new Error(
        "governanceId is required to use the governance client",
      );
    }
    return new Contract(this.config.governanceId);
  }

  private async simulateRead(operation: xdr.Operation): Promise<xdr.ScVal> {
    const sourceAccount = new Account(this.config.simAccount, "0");
    const tx = new TransactionBuilder(sourceAccount, {
      fee: this.config.baseFee ?? BASE_FEE,
      networkPassphrase: this.config.networkPassphrase,
    })
      .addOperation(operation)
      .setTimeout(this.config.timeoutSeconds ?? 30)
      .build();

    const sim = await this.server.simulateTransaction(tx);
    if (SorobanRpc.Api.isSimulationError(sim)) {
      throw new Error(`Simulation failed: ${sim.error}`);
    }
    if (!SorobanRpc.Api.isSimulationSuccess(sim)) {
      throw new Error("Simulation returned unexpected response");
    }
    const retval = sim.result?.retval;
    if (!retval) {
      throw new Error("No return value in simulation result");
    }
    return retval;
  }

  private async submitSignedTransaction(
    keypair: Keypair,
    operation: xdr.Operation,
    operationName: string,
  ): Promise<{ hash: string; retval?: xdr.ScVal }> {
    const publicKey = getPublicKey(keypair);
    const accountData = await this.server.getAccount(publicKey);
    const sourceAccount = new Account(
      publicKey,
      accountData.sequenceNumber(),
    );
    const tx = new TransactionBuilder(sourceAccount, {
      fee: this.config.baseFee ?? BASE_FEE,
      networkPassphrase: this.config.networkPassphrase,
    })
      .addOperation(operation)
      .setTimeout(this.config.timeoutSeconds ?? 30)
      .build();

    const sim = await this.server.simulateTransaction(tx);
    if (SorobanRpc.Api.isSimulationError(sim)) {
      throw new Error(`${operationName} simulation failed: ${sim.error}`);
    }
    if (!SorobanRpc.Api.isSimulationSuccess(sim)) {
      throw new Error(`${operationName} simulation returned unexpected response`);
    }

    const retval = sim.result?.retval;
    const preparedTx = SorobanRpc.assembleTransaction(tx, sim).build();
    preparedTx.sign(keypair);
    const response = await this.server.sendTransaction(preparedTx);
    if (response.status !== "PENDING") {
      throw new Error(
        `${operationName} transaction submission failed: ${response.errorResult}`,
      );
    }

    await waitForTransactionConfirmation(
      this.server,
      response.hash,
      operationName,
    );
    return { hash: response.hash, retval };
  }
}

export class StellarDIDCreditSDK {
  private server: SorobanRpc.Server;
  public readonly governance: GovernanceClient;

  constructor(private config: ProtocolConfig) {
    this.server = new SorobanRpc.Server(this.config.rpcUrl);
    this.governance = new GovernanceClient(this.config, this.server);
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
    subjectKeypair: KeypairLike,
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

    const server = this.server;
    const contract = new Contract(this.config.identityOracleId);

    // Get the current account sequence number
    const accountData = await server.getAccount(publicKey);
    const sourceAccount = new Account(publicKey, accountData.sequenceNumber());

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
    const preparedTx = SorobanRpc.assembleTransaction(tx, sim).build();
    preparedTx.sign(subjectKeypair as Keypair);

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
    issuerKeypair: KeypairLike,
    subjectAddress: string,
    vcHash: Buffer,
  ): Promise<string> {
    if (vcHash.length !== 32) {
      throw new Error("vcHash must be exactly 32 bytes");
    }

    const server = this.server;
    const contract = new Contract(this.config.identityOracleId);

    const publicKey =
      typeof issuerKeypair.publicKey === "function"
        ? issuerKeypair.publicKey()
        : issuerKeypair.publicKey;

    // Get the current account sequence number
    const accountData = await server.getAccount(publicKey);
    const sourceAccount = new Account(publicKey, accountData.sequenceNumber());

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
    const preparedTx = SorobanRpc.assembleTransaction(tx, sim).build();
    preparedTx.sign(issuerKeypair as Keypair);

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
    const sourceAccount = new Account(publicKey, accountData.sequenceNumber());

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

    const preparedTx = SorobanRpc.assembleTransaction(tx, sim).build();
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
    const server = this.server;
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
        return parseScoreRecord(resultScVal);
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
    const server = this.server;
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
   * The issuer signs one transaction calling `revocation_registry.revoke`.
   * The registry is responsible for synchronizing the identity-oracle state.
   * A submitted transaction is polled until it succeeds, fails, or reaches
   * the configured confirmation deadline.
   *
   * @param issuerKeypair - Stellar keypair of the credential issuer
   * @param vcHash - SHA-256 hash of the verifiable credential (must be exactly 32 bytes)
   * @returns Transaction hash after successful ledger confirmation
   * @throws SDKError with code `INVALID_VC_HASH` for a non-32-byte hash
   * @throws SDKError with code `NOT_REGISTERED_ISSUER` for `IssuerMismatch`
   */
  async revokeVC(
    issuerKeypair: KeypairLike,
    vcHash: Buffer,
  ): Promise<string> {
    if (vcHash.length !== 32) {
      throw new SDKError(
        "INVALID_VC_HASH",
        "vcHash must be exactly 32 bytes",
      );
    }

    if (!this.config.revocationRegistryId.trim()) {
      throw new SDKError(
        "MISSING_REVOCATION_REGISTRY",
        "revocationRegistryId is required to revoke a VC",
      );
    }

    const server = this.server;
    const registryContract = new Contract(this.config.revocationRegistryId);

    const publicKey =
      typeof issuerKeypair.publicKey === "function"
        ? issuerKeypair.publicKey()
        : issuerKeypair.publicKey;

    // Get the current account sequence number
    const accountData = await server.getAccount(publicKey);
    const sourceAccount = new Account(publicKey, accountData.sequenceNumber());

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
          hashScVal,
        ),
      )
      .setTimeout(this.config.timeoutSeconds ?? 30)
      .build();

    // Simulate to ensure the call succeeds
    const sim = await server.simulateTransaction(tx);

    if (SorobanRpc.Api.isSimulationError(sim)) {
      throw createRevokeError(
        `revokeVC simulation failed; no revocation state was changed: ${sim.error}`,
        sim.error,
      );
    }

    if (!SorobanRpc.Api.isSimulationSuccess(sim)) {
      throw new SDKError(
        "TRANSACTION_FAILED",
        "revokeVC simulation returned an unexpected response; no revocation state was changed",
      );
    }

    // Apply simulation result and prepare the transaction
    const preparedTx = SorobanRpc.assembleTransaction(tx, sim).build();
    preparedTx.sign(issuerKeypair as Keypair);

    // Submit to the network
    const response = await server.sendTransaction(preparedTx);

    if (response.status !== "PENDING") {
      throw createRevokeError(
        `revokeVC submission failed; no revocation was applied: ${response.errorResult}`,
        response.errorResult,
      );
    }

    try {
      await waitForTransactionConfirmation(
        server,
        response.hash,
        "revokeVC",
        this.config.confirmationTimeoutMs ??
          (this.config.timeoutSeconds ?? 30) * 1000,
        this.config.pollIntervalMs ?? 1000,
      );
    } catch (error) {
      throw createRevokeError(
        `revokeVC failed; the atomic transaction rolled back both registry and identity-oracle changes: ${getErrorMessage(error)}`,
        error,
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
    const server = this.server;
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

    const server = this.server;
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
    const server = this.server;
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
   * Fetch the list of verifiable credential anchors for a subject from the
   * identity-oracle, including revoked entries.
   *
   * Uses a read-only simulation (no signing required).
   *
   * @param subjectAddress - Stellar G... address of the subject
   * @returns Array of VCRecord entries, or an empty array if the subject has
   *          no anchored credentials
   */
  async getVCs(subjectAddress: string): Promise<VCRecord[]> {
    const server = this.server;
    const contract = new Contract(this.config.identityOracleId);
    const sourceAccount = new Account(this.config.simAccount, "0");

    const tx = new TransactionBuilder(sourceAccount, {
      fee: BASE_FEE,
      networkPassphrase: this.config.networkPassphrase,
    })
      .addOperation(
        contract.call("get_vc_details", new Address(subjectAddress).toScVal()),
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

    return parseVCRecordList(resultScVal);
  }

  /**
   * Fetch the credential type label anchored for a subject's VC hash from the
   * identity-oracle.
   *
   * Uses a read-only simulation (no signing required).
   *
   * @param subjectAddress - Stellar G... address of the credential subject
   * @param vcHash - SHA-256 hash of the verifiable credential (must be exactly 32 bytes)
   * @returns The credential type label (e.g. "generic", "kyc", "employment")
   */
  async getCredentialType(
    subjectAddress: string,
    vcHash: Buffer,
  ): Promise<string> {
    if (vcHash.length !== 32) {
      throw new Error("vcHash must be exactly 32 bytes");
    }

    const server = this.server;
    const contract = new Contract(this.config.identityOracleId);
    const sourceAccount = new Account(this.config.simAccount, "0");

    const hashScVal = nativeToScVal(new Uint8Array(vcHash), { type: "bytes" });

    const tx = new TransactionBuilder(sourceAccount, {
      fee: BASE_FEE,
      networkPassphrase: this.config.networkPassphrase,
    })
      .addOperation(
        contract.call(
          "get_vc_credential_type",
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

    return String(scValToNative(resultScVal));
  }

  /**
   * Fetch the scoring weights currently configured on the credit-oracle contract.
   *
   * Uses a read-only simulation (no signing required).
   *
   * @returns The current ScoringWeights configuration
   */
  async getWeights(): Promise<ScoringWeights> {
    const server = this.server;
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
    const server = this.server;
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

  /**
   * List governance proposals starting from `fromId` up to `limit`.
   *
   * Uses a read-only simulation against the governance contract.
   *
   * @param fromId - Proposal ID to start listing from
   * @param limit - Maximum number of proposals to fetch (capped at 20 on-chain)
   * @param includeInactive - Whether to include executed or cancelled proposals (default false)
   * @returns Array of GovernanceProposal objects
   */
  async listProposals(
    fromId: number | bigint,
    limit: number,
    includeInactive = false,
  ): Promise<GovernanceProposal[]> {
    if (!this.config.governanceId) {
      throw new Error("governanceId is not configured in ProtocolConfig");
    }

    const server = this.server;
    const contract = new Contract(this.config.governanceId);
    const sourceAccount = new Account(this.config.simAccount, "0");

    const tx = new TransactionBuilder(sourceAccount, {
      fee: BASE_FEE,
      networkPassphrase: this.config.networkPassphrase,
    })
      .addOperation(
        contract.call(
          "list_proposals",
          nativeToScVal(BigInt(fromId), { type: "u64" }),
          nativeToScVal(limit, { type: "u32" }),
          nativeToScVal(includeInactive, { type: "bool" }),
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

    return parseGovernanceProposalList(resultScVal);
  }
}

/** Thrown when get_score is called for an address that has no computed score yet. */
export class ScoreNotComputedError extends Error {
  constructor(address?: string) {
    super(address ? `No score computed for address: ${address}` : "Score has not been computed");
    this.name = "ScoreNotComputedError";
  }
}

function getPublicKey(keypair: KeypairLike): string {
  return typeof keypair.publicKey === "function"
    ? keypair.publicKey()
    : keypair.publicKey;
}

function toUnsignedBigInt(value: GovernanceInteger): bigint {
  assertSafeInteger(value);
  const result = BigInt(value);
  if (result < 0n) {
    throw new Error("integer values must be non-negative");
  }
  return result;
}

function toPositiveBigInt(value: GovernanceInteger): bigint {
  assertSafeInteger(value);
  const result = BigInt(value);
  if (result <= 0n) {
    throw new Error("voteWeight must be positive");
  }
  return result;
}

function assertSafeInteger(value: GovernanceInteger): void {
  if (typeof value === "number" && !Number.isSafeInteger(value)) {
    throw new Error("number values must be safe integers; use bigint instead");
  }
}

function scoringWeightsToScVal(weights: ScoringWeights): xdr.ScVal {
  return nativeToScVal(
    {
      vc_weight: weights.vcWeight,
      tx_weight: weights.txWeight,
      repayment_weight: weights.repaymentWeight,
    },
    {
      type: {
        vc_weight: ["symbol", "u32"],
        tx_weight: ["symbol", "u32"],
        repayment_weight: ["symbol", "u32"],
      },
    },
  );
}

/**
 * Parse a Soroban ScVal representing an Option<ScoreRecord>.
 * Returns the ScoreRecord if Some, returns null if None.
 */
function parseScoreRecord(scVal: xdr.ScVal): ScoreRecord | null {
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

function parseGovernanceProposal(native: unknown): GovernanceProposal {
  if (typeof native !== "object" || native === null) {
    throw new Error("get_proposal returned an invalid result");
  }

  const raw = native as Record<string, unknown>;
  const weights = raw["proposed_weights"];
  if (typeof weights !== "object" || weights === null) {
    throw new Error("get_proposal returned invalid proposed weights");
  }

  const rawWeights = weights as Record<string, unknown>;
  return {
    id: BigInt(raw["id"] as bigint | number | string),
    proposer: String(raw["proposer"]),
    proposedWeights: {
      vcWeight: Number(rawWeights["vc_weight"]),
      txWeight: Number(rawWeights["tx_weight"]),
      repaymentWeight: Number(rawWeights["repayment_weight"]),
    },
    votesFor: BigInt(raw["votes_for"] as bigint | number | string),
    votesAgainst: BigInt(
      raw["votes_against"] as bigint | number | string,
    ),
    expiryLedger: Number(raw["expiry_ledger"]),
    executionDelayLedgers: Number(raw["execution_delay_ledgers"]),
    executed: Boolean(raw["executed"]),
    cancelled: Boolean(raw["cancelled"]),
    quorumRequired: BigInt(
      raw["quorum_required"] as bigint | number | string,
    ),
  };
}

/**
 * Parse a Soroban ScVal representing a `Vec<VCRecord>`.
 * The identity-oracle serializes `vc_hash` (BytesN<32>) as raw bytes, so
 * the value is normalized to a Buffer for the exported VCRecord type.
 */
function parseVCRecordList(scVal: xdr.ScVal): VCRecord[] {
  const native = scValToNative(scVal);
  if (native === null || native === undefined) {
    return [];
  }
  return (native as unknown[]).map((entry) => {
    const raw = entry as Record<string, unknown>;
    const vcHash = raw["vc_hash"] as Buffer | Uint8Array | undefined;
    return {
      vcHash: Buffer.isBuffer(vcHash)
        ? vcHash
        : Buffer.from(vcHash ?? new Uint8Array()),
      issuer: String(raw["issuer"]),
      anchoredAt: Number(raw["anchored_at"]),
      revoked: Boolean(raw["revoked"]),
    };
  });
}

/**
 * Parse a Soroban ScVal representing a `Vec<GovernanceProposal>`.
 */
function parseGovernanceProposalList(scVal: xdr.ScVal): GovernanceProposal[] {
  const native = scValToNative(scVal);
  if (native === null || native === undefined) {
    return [];
  }
  return (native as unknown[]).map((entry) => {
    const raw = entry as Record<string, unknown>;
    const weights = raw["proposed_weights"] as Record<string, unknown>;
    return {
      id: BigInt(raw["id"] as bigint | number | string),
      proposer: String(raw["proposer"]),
      proposedWeights: {
        vcWeight: Number(weights["vc_weight"]),
        txWeight: Number(weights["tx_weight"]),
        repaymentWeight: Number(weights["repayment_weight"]),
      },
      votesFor: BigInt(raw["votes_for"] as bigint | number | string),
      votesAgainst: BigInt(raw["votes_against"] as bigint | number | string),
      expiryLedger: Number(raw["expiry_ledger"]),
      executionDelayLedgers: Number(raw["execution_delay_ledgers"]),
      executed: Boolean(raw["executed"]),
      cancelled: Boolean(raw["cancelled"]),
      quorumRequired: BigInt(raw["quorum_required"] as bigint | number | string),
    };
  });
}

async function waitForTransactionConfirmation(
  server: SorobanRpc.Server,
  txHash: string,
  operationName: string,
  timeoutMs = 20_000,
  delayMs = 1000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  let firstAttempt = true;

  while (firstAttempt || Date.now() <= deadline) {
    firstAttempt = false;
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
      case "PENDING": {
        const remainingMs = deadline - Date.now();
        if (remainingMs <= 0) {
          throw new Error(
            `Timed out waiting for ${operationName} transaction confirmation: ${txHash}`,
          );
        }
        await sleep(Math.min(delayMs, remainingMs));
        break;
      }
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

function createRevokeError(message: string, details: unknown): SDKError {
  if (containsIssuerMismatch(details)) {
    return new SDKError(
      "NOT_REGISTERED_ISSUER",
      "The issuer is not registered for this VC hash",
      { cause: details },
    );
  }

  if (message.includes("Timed out waiting")) {
    return new SDKError("TRANSACTION_TIMEOUT", message, { cause: details });
  }

  return new SDKError("TRANSACTION_FAILED", message, { cause: details });
}

function containsIssuerMismatch(value: unknown): boolean {
  const text = getErrorMessage(value).toLowerCase();
  return (
    text.includes("issuermismatch") ||
    text.includes("issuer mismatch") ||
    /error\(contract,\s*#3\)/i.test(text)
  );
}

function getErrorMessage(value: unknown): string {
  if (value instanceof Error) {
    return value.message;
  }
  if (typeof value === "string") {
    return value;
  }
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

export default StellarDIDCreditSDK;
