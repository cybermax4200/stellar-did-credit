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

export interface ScoreRecord {
  score: number;
  lastUpdated: number;
  vcCount: number;
  repaymentRate: number;
  txVolume30d: bigint;
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
  async anchorDID(subjectKeypair: any, didDocCid: string): Promise<string> {
    const server = new SorobanRpc.Server(this.config.rpcUrl);
    const contract = new Contract(this.config.identityOracleId);

    const publicKey =
      subjectKeypair.publicKey instanceof Function
        ? subjectKeypair.publicKey()
        : subjectKeypair.publicKey;

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
   * @param vcHash - SHA-256 hash of the verifiable credential
   * @returns Transaction hash on successful submission
   */
  async issueVC(
    issuerKeypair: any,
    subjectAddress: string,
    vcHash: Buffer,
  ): Promise<string> {
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
        throw new ScoreNotComputedError();
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
   * Verify that a specific VC hash is valid and not revoked for the given subject.
   *
   * Uses a read-only simulation against the identity-oracle contract.
   *
   * @param subjectAddress - Stellar G... address of the subject
   * @param vcHash - SHA-256 hash of the credential
   * @returns true if the credential is valid and not revoked
   */
  async verifyVC(subjectAddress: string, vcHash: Buffer): Promise<boolean> {
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
 * Returns the ScoreRecord if Some, throws ScoreNotComputedError if None.
 */
function parseScoreRecord(scVal: xdr.ScVal, subjectAddress: string): ScoreRecord {
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

export default StellarDIDCreditSDK;
