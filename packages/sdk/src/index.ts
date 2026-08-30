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

export type NetworkType = 'testnet' | 'mainnet' | 'futurenet' | 'custom';

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
  network?: NetworkType;
}

export type Unsubscribe = () => void;

export type SDKErrorCode =
  | "INVALID_VC_HASH"
  | "MISSING_REVOCATION_REGISTRY"
  | "NOT_REGISTERED_ISSUER"
  | "TRANSACTION_FAILED"
  | "TRANSACTION_TIMEOUT"
  | "COOLDOWN_ACTIVE";

// ---------------------------------------------------------------------------
// Contract error hierarchy
// ---------------------------------------------------------------------------

/** Base class for typed errors returned by Soroban smart contracts. */
export class ContractError extends Error {
  constructor(
    public readonly code: number,
    public readonly contractName: string,
    message: string,
  ) {
    super(message);
    this.name = "ContractError";
  }
}

export class IdentityOracleError extends ContractError {
  constructor(code: number, message: string) {
    super(code, "identity-oracle", message);
    this.name = "IdentityOracleError";
  }
}

export class CreditOracleError extends ContractError {
  constructor(code: number, message: string) {
    super(code, "credit-oracle", message);
    this.name = "CreditOracleError";
  }
}

export class RevocationRegistryError extends ContractError {
  constructor(code: number, message: string) {
    super(code, "revocation-registry", message);
    this.name = "RevocationRegistryError";
  }
}

export class GovernanceError extends ContractError {
  constructor(code: number, message: string) {
    super(code, "governance", message);
    this.name = "GovernanceError";
  }
}

// ---------------------------------------------------------------------------
// Error code maps (numeric code → human-readable variant name)
// ---------------------------------------------------------------------------

const IDENTITY_ORACLE_ERROR_CODES: Record<number, string> = {
  1: "AlreadyInitialized",
  2: "NotAuthorized",
  3: "IssuerNotRegistered",
  4: "InvalidCID",
  5: "NoPendingAdmin",
  6: "DuplicateVC",
  7: "VCNotFound",
  8: "ContractPaused",
  9: "InvalidRevocationRegistry",
  10: "VCLimitReached",
};

const CREDIT_ORACLE_ERROR_CODES: Record<number, string> = {
  1: "AlreadyInitialized",
  2: "NotAuthorized",
  3: "FeederNotRegistered",
  4: "LenderNotRegistered",
  5: "InvalidWeights",
  6: "NoPendingAdmin",
  7: "ComputeCooldownActive",
  8: "DisputeAlreadyPending",
  9: "DisputeNotFound",
  10: "InvalidInputKey",
  11: "InvalidIdentityOracle",
  12: "ContractPaused",
  13: "InvalidRecencyConfig",
  14: "TimelockNotExpired",
  15: "NoPendingWeights",
  16: "NotInitialized",
};

const REVOCATION_REGISTRY_ERROR_CODES: Record<number, string> = {
  1: "AlreadyInitialized",
  2: "NotAuthorized",
  3: "IssuerMismatch",
  4: "NoPendingAdmin",
  5: "BatchTooLarge",
  6: "ContractPaused",
  7: "ReentrancyDetected",
  8: "InvalidBatchLimit",
};

const GOVERNANCE_ERROR_CODES: Record<number, string> = {
  1: "AlreadyInitialized",
  2: "NotAuthorized",
  3: "ProposalNotFound",
  4: "ProposalExpired",
  5: "ProposalNotExpired",
  6: "ProposalAlreadyExecuted",
  7: "InvalidWeights",
  8: "InvalidQuorum",
  9: "InvalidVoteWeight",
  10: "QuorumNotMet",
  11: "TimelockNotExpired",
  12: "VoterNotRegistered",
  13: "InsufficientVoteWeight",
  14: "ProposalAlreadyCancelled",
};

const ERROR_CODE_MAPS: Record<string, Record<number, string>> = {
  "identity-oracle": IDENTITY_ORACLE_ERROR_CODES,
  "credit-oracle": CREDIT_ORACLE_ERROR_CODES,
  "revocation-registry": REVOCATION_REGISTRY_ERROR_CODES,
  governance: GOVERNANCE_ERROR_CODES,
};

// ---------------------------------------------------------------------------
// Contract error parsing
// ---------------------------------------------------------------------------

const CONTRACT_ERROR_RE = /Error\(Contract,\s*#(\d+)\)/i;

/**
 * Parse a Soroban simulation or transaction error string and return the
 * numeric contract error code, or `null` if the string does not match the
 * `Error(Contract, #N)` pattern.
 */
export function parseContractErrorCode(errorString: string): number | null {
  const match = CONTRACT_ERROR_RE.exec(errorString);
  return match ? Number(match[1]) : null;
}

/**
 * Parse a Soroban error string and throw the appropriate typed contract error
 * for the given contract name.
 *
 * @param errorString - The raw error string from Soroban RPC
 * @param contractName - One of "identity-oracle", "credit-oracle",
 *   "revocation-registry", "governance"
 * @throws {IdentityOracleError | CreditOracleError | RevocationRegistryError |
 *   GovernanceError} when a recognized contract error code is found
 * @throws {Error} when the error string does not match a known contract error
 *   pattern (re-thrown as-is)
 */
export function throwContractError(
  errorString: string,
  contractName:
    | "identity-oracle"
    | "credit-oracle"
    | "revocation-registry"
    | "governance",
): never {
  const code = parseContractErrorCode(errorString);
  const codeMap = ERROR_CODE_MAPS[contractName];
  const variantName = code !== null && codeMap ? codeMap[code] : undefined;
  const message =
    code !== null && variantName
      ? `${variantName} (code ${code})`
      : errorString;

  switch (contractName) {
    case "identity-oracle":
      throw new IdentityOracleError(code ?? 0, message);
    case "credit-oracle":
      throw new CreditOracleError(code ?? 0, message);
    case "revocation-registry":
      throw new RevocationRegistryError(code ?? 0, message);
    case "governance":
      throw new GovernanceError(code ?? 0, message);
  }
}

export class SDKError extends Error {
  constructor(
    public readonly code: SDKErrorCode,
    message: string,
    options?: {
      cause?: unknown;
      transactionHash?: string;
      resultXdr?: string;
    },
  ) {
    super(message);
    if (options?.cause !== undefined) {
      this.cause = options.cause;
    }
    this.transactionHash = options?.transactionHash;
    this.resultXdr = options?.resultXdr;
    this.name = "SDKError";
  }

  declare readonly cause?: unknown;
  declare readonly transactionHash?: string;
  declare readonly resultXdr?: string;
}

/**
 * Network configurations for Stellar networks.
 */
const NETWORK_CONFIGS: Record<Exclude<NetworkType, 'custom'>, Partial<ProtocolConfig>> = {
  testnet: {
    networkPassphrase: "Test SDF Network ; September 2015",
    rpcUrl: "https://soroban-testnet.stellar.org",
    simAccount: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
  },
  mainnet: {
    networkPassphrase: "Public Global Stellar Network ; September 2015",
    rpcUrl: "https://soroban-rpc.mainnet.stellarchain.io",
    // Note: SIM_ACCOUNT for mainnet must be set explicitly to a funded account
    simAccount: "",
  },
  futurenet: {
    networkPassphrase: "Test SDF Future Network ; October 2022",
    rpcUrl: "https://rpc-futurenet.stellar.org",
    simAccount: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
  },
};

/** A Stellar keypair, or a minimal object exposing a public key. */
export type KeypairLike = Keypair | { publicKey: string };

/**
 * Create a ProtocolConfig with network-specific defaults.
 * 
 * @param network - Network type (testnet, mainnet, futurenet, custom)
 * @param overrides - Configuration overrides
 * @returns Complete ProtocolConfig with network defaults applied
 */
export function createNetworkConfig(
  network: NetworkType,
  overrides: Partial<ProtocolConfig> = {}
): Partial<ProtocolConfig> {
  if (network === 'custom') {
    return overrides;
  }
  
  const networkDefaults = NETWORK_CONFIGS[network];
  return {
    ...networkDefaults,
    ...overrides,
  };
}

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
    * Proposal IDs are 1-based: the first proposal has ID 1 and ID 0 is unused.
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
    * Proposal IDs start at 1; pass `1` or `1n` as `fromId` to include the first
    * proposal.
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
      throwContractError(sim.error, "governance");
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
      throwContractError(sim.error, "governance");
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

  constructor(config: ProtocolConfig) {
    // Apply network defaults if network is specified but URL fields are missing
    if (config.network && config.network !== 'custom') {
      const networkDefaults = NETWORK_CONFIGS[config.network];
      this.config = {
        ...networkDefaults,
        ...config,
      } as ProtocolConfig;
    } else {
      this.config = config;
    }

    this.server = new SorobanRpc.Server(this.config.rpcUrl);
    this.governance = new GovernanceClient(this.config, this.server);
  }

  private config: ProtocolConfig;

  /**
   * Anchor a DID document on-chain by storing its IPFS CID.
   *
   * Submits a signed transaction to the identity-oracle contract. Requires the subject
   * keypair to authorize the operation.
   *
   * @param subjectKeypair - Stellar keypair of the subject (private + public key)
   * @param didDocCid - IPFS CID of the DID document (e.g. "Qm...")
   * @param subjectAddress - Optional Stellar G... address of the subject for validation
   * @returns Transaction hash after successful ledger confirmation
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
      .setTimeout(this.config.timeoutSeconds ?? 30)
      .build();

    // Simulate to ensure the call succeeds
    const sim = await server.simulateTransaction(tx);

    if (SorobanRpc.Api.isSimulationError(sim)) {
      throwContractError(sim.error, "identity-oracle");
    }

    if (!SorobanRpc.Api.isSimulationSuccess(sim)) {
      throw new Error("Simulation returned unexpected response");
    }

    // Apply simulation result and prepare the transaction
    const preparedTx = SorobanRpc.assembleTransaction(tx, sim).build();
    preparedTx.sign(subjectKeypair as Keypair);

    const txHash = await sendTransactionWithRetry(
      server,
      preparedTx,
      this.config.maxRetries,
      (response) =>
        new Error(`Transaction submission failed: ${response.errorResult}`),
    );

    await waitForTransactionConfirmation(
      server,
      txHash,
      "anchorDID",
      getConfirmationTimeoutMs(this.config),
      getTransactionPollIntervalMs(this.config),
    );

    return txHash;
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
   * @returns Transaction hash after successful ledger confirmation
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
      .setTimeout(this.config.timeoutSeconds ?? 30)
      .build();

    // Simulate to ensure the call succeeds
    const sim = await server.simulateTransaction(tx);

    if (SorobanRpc.Api.isSimulationError(sim)) {
      throwContractError(sim.error, "identity-oracle");
    }

    if (!SorobanRpc.Api.isSimulationSuccess(sim)) {
      throw new Error("Simulation returned unexpected response");
    }

    // Apply simulation result and prepare the transaction
    const preparedTx = SorobanRpc.assembleTransaction(tx, sim).build();
    preparedTx.sign(issuerKeypair as Keypair);

    const txHash = await sendTransactionWithRetry(
      server,
      preparedTx,
      this.config.maxRetries,
      (response) =>
        new Error(`Transaction submission failed: ${response.errorResult}`),
    );

    await waitForTransactionConfirmation(
      server,
      txHash,
      "issueVC",
      getConfirmationTimeoutMs(this.config),
      getTransactionPollIntervalMs(this.config),
    );

    return txHash;
  }

  /**
   * Compute and persist a subject's credit score, then return the computed score.
   *
   * Submits a signed transaction to the credit-oracle contract, waits for ledger
   * confirmation, then fetches the persisted score via `getScore` and returns
   * the numeric score value.
   *
   * **Note on Cooldowns:** The `compute_score` contract method is protected by a
   * cooldown period (`ComputeCooldownLedgers`). If this method is called while the
   * cooldown is active (or immediately after a fresh deployment before the initial
   * cooldown has passed), the transaction will fail and throw an `SDKError` with
   * code `COOLDOWN_ACTIVE`.
   *
   * @param payerKeypair - Stellar keypair (or object with publicKey) paying the transaction fee
   * @param subjectAddress - Stellar G... address of the subject
   * @returns The computed score number (300–850)
   * @throws SDKError with code `COOLDOWN_ACTIVE` if the cooldown period is active
   * @throws SDKError with code `TRANSACTION_FAILED` if the transaction fails
   * @throws SDKError with code `TRANSACTION_TIMEOUT` if confirmation times out
   */
  async computeScore(
    payerKeypair: KeypairLike,
    subjectAddress: string,
  ): Promise<number> {
    const contract = new Contract(this.config.creditOracleId);

    const publicKey = getPublicKey(payerKeypair);

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
        throw new SDKError(
          "COOLDOWN_ACTIVE",
          "Cooldown period is active. Please wait for the cooldown ledgers to pass before recomputing the score.",
        );
      }
      throw new SDKError(
        "TRANSACTION_FAILED",
        `Simulation failed: ${sim.error}`,
      );
    }

    if (!SorobanRpc.Api.isSimulationSuccess(sim)) {
      throw new SDKError(
        "TRANSACTION_FAILED",
        "Simulation returned unexpected response",
      );
    }

    const preparedTx = SorobanRpc.assembleTransaction(tx, sim).build();
    preparedTx.sign(payerKeypair as Keypair);

    const txHash = await sendTransactionWithRetry(
      this.server,
      preparedTx,
      this.config.maxRetries,
      (submissionResponse) => {
        if (
          submissionResponse.errorResult &&
          String(submissionResponse.errorResult)
            .toLowerCase()
            .includes("cooldown")
        ) {
          return new SDKError(
            "COOLDOWN_ACTIVE",
            "Cooldown period is active. Please wait for the cooldown ledgers to pass before recomputing the score.",
          );
        }
        return new SDKError(
          "TRANSACTION_FAILED",
          `Transaction submission failed: ${String(submissionResponse.errorResult)}`,
        );
      },
    );

    try {
      await waitForTransactionConfirmation(
        this.server,
        txHash,
        "computeScore",
        getConfirmationTimeoutMs(this.config),
        this.config.pollIntervalMs ?? 1000,
      );
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      if (message.toLowerCase().includes("cooldown")) {
        throw new SDKError(
          "COOLDOWN_ACTIVE",
          "Cooldown period is active. Please wait for the cooldown ledgers to pass before recomputing the score.",
        );
      }
      throw error;
    }
    await waitForTransactionConfirmation(
      this.server,
      txHash,
      "computeScore",
      getConfirmationTimeoutMs(this.config),
      getTransactionPollIntervalMs(this.config),
    );

    try {
      const score = await this.getScore(subjectAddress);
      if (!score) {
        throw new ScoreNotComputedError(subjectAddress);
      }
      return score.score;
    } catch (error) {
      if (error instanceof SDKError) {
        throw error;
      }
      const message = error instanceof Error ? error.message : String(error);
      throw new SDKError(
        "TRANSACTION_FAILED",
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
        throwContractError(sim.error, "credit-oracle");
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
      throwContractError(sim.error, "identity-oracle");
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
      throwContractError(sim.error, "revocation-registry");
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

    const txHash = await sendTransactionWithRetry(
      server,
      preparedTx,
      this.config.maxRetries,
      (response) =>
        createRevokeError(
          `revokeVC submission failed; no revocation was applied: ${response.errorResult}`,
          response.errorResult,
        ),
    );

    try {
      await waitForTransactionConfirmation(
        server,
        txHash,
        "revokeVC",
        getConfirmationTimeoutMs(this.config),
        getTransactionPollIntervalMs(this.config),
      );
    } catch (error) {
      throw createRevokeError(
        `revokeVC failed; the atomic transaction rolled back both registry and identity-oracle changes: ${getErrorMessage(error)}`,
        error,
      );
    }

    return txHash;
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
      throwContractError(sim.error, "identity-oracle");
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
      if (isVerifyVCNegativeSimulationError(sim.error)) {
        return false;
      }
      throwContractError(sim.error, "identity-oracle");
    }

    if (!SorobanRpc.Api.isSimulationSuccess(sim)) {
      throw new Error("Simulation returned unexpected response");
    }

    const resultScVal = sim.result?.retval;
    if (!resultScVal) {
      throw new Error("No return value in simulation result");
    }

    const native = scValToNative(resultScVal);
    if (typeof native !== "boolean") {
      throw new Error("verify_vc returned a non-boolean result");
    }

    return native;
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
      throwContractError(sim.error, "identity-oracle");
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
      throwContractError(sim.error, "identity-oracle");
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
      throwContractError(sim.error, "identity-oracle");
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
      throwContractError(sim.error, "credit-oracle");
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
      throwContractError(sim.error, "identity-oracle");
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
      throwContractError(sim.error, "governance");
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

  /**
   * Poll for VCAnch events emitted by an identity-oracle contract.
   *
   * The first poll starts at the latest ledger returned by the RPC server.
   * Subsequent polls begin after the latest ledger seen by this subscription.
   *
   * @param contractId - Identity-oracle contract ID to monitor
   * @param callback - Called with the issuer, subject, and 32-byte VC hash
   * @returns A function that stops future polls for this subscription
   */
  onVCAnchored(
    contractId: string,
    callback: (issuer: string, subject: string, vcHash: Buffer) => void,
  ): Unsubscribe {
    return this.subscribeToEvents(
      contractId,
      "VCAnch",
      (value) => {
        const [issuer, subject, vcHash] = parseEventTuple(
          value,
          "VCAnch",
          3,
        );
        callback(String(issuer), String(subject), toBuffer(vcHash));
      },
    );
  }

  /**
   * Poll for Score events emitted by a credit-oracle contract.
   *
   * @param contractId - Credit-oracle contract ID to monitor
   * @param callback - Called with the subject address and computed score
   * @returns A function that stops future polls for this subscription
   */
  onScoreComputed(
    contractId: string,
    callback: (subject: string, score: number) => void,
  ): Unsubscribe {
    return this.subscribeToEvents(
      contractId,
      "Score",
      (value) => {
        const [subject, score] = parseEventTuple(value, "Score", 2);
        callback(String(subject), Number(score));
      },
    );
  }

  /**
   * Poll for Revoked events emitted by a revocation-registry contract.
   *
   * @param contractId - Revocation-registry contract ID to monitor
   * @param callback - Called with the issuer and 32-byte VC hash
   * @returns A function that stops future polls for this subscription
   */
  onVCRevoked(
    contractId: string,
    callback: (issuer: string, vcHash: Buffer) => void,
  ): Unsubscribe {
    return this.subscribeToEvents(
      contractId,
      "Revoked",
      (value) => {
        const [issuer, vcHash] = parseEventTuple(value, "Revoked", 2);
        callback(String(issuer), toBuffer(vcHash));
      },
    );
  }

  private subscribeToEvents(
    contractId: string,
    eventName: string,
    handleValue: (value: xdr.ScVal) => void,
  ): Unsubscribe {
    const pollIntervalMs = this.config.pollIntervalMs ?? 1000;
    if (!Number.isFinite(pollIntervalMs) || pollIntervalMs <= 0) {
      throw new Error("pollIntervalMs must be a positive number");
    }

    let active = true;
    let polling = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let lastSeenLedger: number | undefined;

    const poll = async (): Promise<void> => {
      if (!active || polling) {
        return;
      }

      polling = true;
      try {
        if (lastSeenLedger === undefined) {
          const latestLedger = await this.server.getLatestLedger();
          lastSeenLedger = latestLedger.sequence;
        }

        if (!active) {
          return;
        }

        const response = await this.server.getEvents({
          startLedger: lastSeenLedger,
          filters: [
            {
              type: "contract",
              contractIds: [contractId],
              topics: [[xdr.ScVal.scvSymbol(eventName).toXDR("base64")]],
            },
          ],
          limit: 100,
        });

        const latestEventLedger = response.events.reduce(
          (highestLedger, event) => Math.max(highestLedger, event.ledger),
          lastSeenLedger,
        );
        lastSeenLedger =
          Math.max(response.latestLedger, latestEventLedger) + 1;

        for (const event of response.events) {
          if (!active) {
            break;
          }
          handleValue(event.value);
        }
      } catch {
        // Keep polling after transient RPC failures. The ledger cursor is only
        // advanced after a successful getEvents response.
      } finally {
        polling = false;
        if (active) {
          timer = setTimeout(() => void poll(), pollIntervalMs);
        }
      }
    };

    void poll();

    return () => {
      active = false;
      if (timer !== undefined) {
        clearTimeout(timer);
        timer = undefined;
      }
    };
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

type SendTransactionErrorFactory = (
  response: SorobanRpc.Api.SendTransactionResponse,
) => Error;

async function sendTransactionWithRetry(
  server: SorobanRpc.Server,
  transaction: Parameters<SorobanRpc.Server["sendTransaction"]>[0],
  maxRetries = 3,
  errorFactory: SendTransactionErrorFactory,
): Promise<string> {
  const retries = normalizeMaxRetries(maxRetries);

  for (let attempt = 0; ; attempt++) {
    let response: SorobanRpc.Api.SendTransactionResponse;
    try {
      response = await server.sendTransaction(transaction);
    } catch (error) {
      if (!isRetryableError(error) || attempt >= retries) {
        throw error;
      }

      await sleep(getRetryDelayMs(attempt));
      continue;
    }

    // DUPLICATE means an earlier attempt already reached the RPC successfully.
    if (response.status === "PENDING" || response.status === "DUPLICATE") {
      return response.hash;
    }

    if (response.status !== "TRY_AGAIN_LATER" || attempt >= retries) {
      throw errorFactory(response);
    }

    await sleep(getRetryDelayMs(attempt));
  }
}

function parseEventTuple(
  scVal: xdr.ScVal,
  eventName: string,
  expectedLength: number,
): unknown[] {
  const native = scValToNative(scVal);
  if (!Array.isArray(native) || native.length !== expectedLength) {
    throw new Error(
      `${eventName} event data must be a tuple with ${expectedLength} values`,
    );
  }
  return native;
}

function toBuffer(value: unknown): Buffer {
  if (Buffer.isBuffer(value)) {
    return value;
  }
  if (value instanceof Uint8Array) {
    return Buffer.from(value);
  }
  throw new Error("event data contains an invalid byte value");
}

async function waitForTransactionConfirmation(
  server: SorobanRpc.Server,
  txHash: string,
  operationName: string,
  timeoutMs = 20_000,
  delayMs = 1000,
): Promise<void> {
  const normalizedTimeoutMs = Number.isFinite(timeoutMs)
    ? Math.max(0, timeoutMs)
    : 30_000;
  const pollDelayMs = Number.isFinite(delayMs) ? Math.max(1, delayMs) : 1000;
  const deadline = Date.now() + normalizedTimeoutMs;

  for (;;) {
    if (Date.now() >= deadline) {
      throwTransactionTimeout(operationName, txHash);
    }

    let result: Awaited<ReturnType<SorobanRpc.Server["getTransaction"]>>;
    try {
      result = await withTimeout(
        server.getTransaction(txHash),
        deadline - Date.now(),
        () => createTransactionTimeoutError(operationName, txHash),
      );
    } catch (error) {
      if (error instanceof SDKError) {
        throw error;
      }
      if (!isRetryableError(error)) {
        throw error;
      }
      if (Date.now() >= deadline) {
        throwTransactionTimeout(operationName, txHash);
      }
      await sleep(Math.min(pollDelayMs, deadline - Date.now()));
      continue;
    }

    switch (result.status as string) {
      case "SUCCESS":
        return;
      case "FAILED": {
        const resultXdr = extractResultXdr(result);
        throw new SDKError(
          "TRANSACTION_FAILED",
          `${operationName} transaction failed for ${txHash}; resultXdr: ${resultXdr ?? "unknown"}`,
          {
            cause: result,
            transactionHash: txHash,
            resultXdr,
          },
        );
      }
      case "NOT_FOUND":
      case "PENDING":
        if (Date.now() >= deadline) {
          throwTransactionTimeout(operationName, txHash);
        }
        await sleep(Math.min(pollDelayMs, deadline - Date.now()));
        break;
      default:
        throw new Error(
          `Unexpected transaction status for ${txHash}: ${String(result.status)}`,
        );
    }
  }
}

function createTransactionTimeoutError(
  operationName: string,
  txHash: string,
): SDKError {
  return new SDKError(
    "TRANSACTION_TIMEOUT",
    `Timed out waiting for ${operationName} transaction confirmation: ${txHash}`,
  );
}

function throwTransactionTimeout(operationName: string, txHash: string): never {
  throw createTransactionTimeoutError(operationName, txHash);
}

function getConfirmationTimeoutMs(config: ProtocolConfig): number {
  return (
    config.confirmationTimeoutMs ??
    (config.timeoutSeconds ?? 30) * 1000
  );
}

function getTransactionPollIntervalMs(config: ProtocolConfig): number {
  const configured = config.pollIntervalMs;
  if (!Number.isFinite(configured) || configured === undefined) {
    return 5000;
  }
  return Math.max(1, configured);
}

function extractResultXdr(value: unknown): string | undefined {
  if (typeof value !== "object" || value === null) {
    return undefined;
  }

  const candidate = value as Record<string, unknown>;
  const raw =
    candidate["resultXdr"] ??
    candidate["result_xdr"] ??
    candidate["errorResultXdr"] ??
    candidate["error_result_xdr"];
  return raw === undefined || raw === null ? undefined : String(raw);
}

function normalizeMaxRetries(maxRetries: number): number {
  return Number.isFinite(maxRetries) ? Math.max(0, Math.floor(maxRetries)) : 3;
}

function getRetryDelayMs(attempt: number): number {
  return 1000 * 2 ** attempt;
}

function isRetryableError(error: unknown): boolean {
  const candidate = error as {
    code?: unknown;
    status?: unknown;
    statusCode?: unknown;
    response?: { status?: unknown; statusCode?: unknown };
  } | null;

  const httpStatus = [
    candidate?.status,
    candidate?.statusCode,
    candidate?.response?.status,
    candidate?.response?.statusCode,
  ]
    .map((status) => Number(status))
    .find((status) => Number.isInteger(status) && status > 0);
  if (httpStatus !== undefined) {
    return [408, 429, 500, 502, 503, 504].includes(httpStatus);
  }

  const code = String(candidate?.code ?? "").toUpperCase();
  if (
    ["ECONNRESET", "ECONNREFUSED", "ENETUNREACH", "ETIMEDOUT", "EAI_AGAIN"].includes(
      code,
    )
  ) {
    return true;
  }

  const message =
    error instanceof Error
      ? error.message.toLowerCase()
      : String(error).toLowerCase();
  return /\b503\b|timeout|timed out|network|fetch failed|unavailable|socket/.test(message);
}

function createRevokeError(message: string, details: unknown): SDKError {
  if (containsIssuerMismatch(details)) {
    return new SDKError(
      "NOT_REGISTERED_ISSUER",
      "The issuer is not registered for this VC hash",
      { cause: details },
    );
  }

  if (
    details instanceof SDKError &&
    details.code === "TRANSACTION_TIMEOUT"
  ) {
    return new SDKError("TRANSACTION_TIMEOUT", message, {
      cause: details,
      transactionHash: details.transactionHash,
      resultXdr: details.resultXdr,
    });
  }

  if (
    details instanceof SDKError &&
    details.code === "TRANSACTION_FAILED"
  ) {
    if (containsIssuerMismatch(details.cause)) {
      return new SDKError(
        "NOT_REGISTERED_ISSUER",
        "The issuer is not registered for this VC hash",
        {
          cause: details,
          transactionHash: details.transactionHash,
          resultXdr: details.resultXdr,
        },
      );
    }

    return new SDKError("TRANSACTION_FAILED", message, {
      cause: details,
      transactionHash: details.transactionHash,
      resultXdr: details.resultXdr,
    });
  }

  return new SDKError("TRANSACTION_FAILED", message, { cause: details });
}

function containsIssuerMismatch(value: unknown): boolean {
  if (value instanceof RevocationRegistryError && value.code === 3) {
    return true;
  }
  if (value instanceof IdentityOracleError && value.code === 3) {
    return true;
  }
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

function isVerifyVCNegativeSimulationError(error: unknown): boolean {
  if (error instanceof IdentityOracleError) {
    return error.code === 7 || error.code === 8;
  }
  const text = getErrorMessage(error).toLowerCase();
  return (
    text.includes("contractpaused") ||
    text.includes("contract paused") ||
    /error\(contract,\s*#8\)/i.test(text) ||
    text.includes("vcnotfound") ||
    text.includes("unknown subject") ||
    text.includes("not found")
  );
}

function withTimeout<T>(
  promise: Promise<T>,
  timeoutMs: number,
  createError: () => Error,
): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(
      () => reject(createError()),
      Math.max(0, timeoutMs),
    );

    promise.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (error: unknown) => {
        clearTimeout(timer);
        reject(error);
      },
    );
  });
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

export default StellarDIDCreditSDK;
