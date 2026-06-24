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
} from "@stellar/stellar-sdk";

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
}

/** Thrown when a score has not been computed for the requested address. */
export class ScoreNotComputedError extends Error {
  constructor() {
    super("Score has not been computed for this address. Call computeScore() first.");
    this.name = "ScoreNotComputedError";
  }
}

/** Zero-balance placeholder account used for read-only simulations. */
const SIM_ACCOUNT = "GXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX";

export class StellarDIDCreditSDK {
  constructor(private config: ProtocolConfig) {}

  async anchorDID(subjectKeypair: any, didDocCid: string): Promise<string> {
    throw new Error("not implemented");
  }

  async issueVC(
    issuerKeypair: any,
    subjectAddress: string,
    vcHash: Buffer,
  ): Promise<string> {
    throw new Error("not implemented");
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
    const sourceAccount = new Account(SIM_ACCOUNT, "0");
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
    return parseScoreRecord(resultScVal);
  }

  async verifyVC(subjectAddress: string, vcHash: Buffer): Promise<boolean> {
    throw new Error("not implemented");
  }

  async isVerified(subjectAddress: string): Promise<boolean> {
    throw new Error("not implemented");
  }
}

/**
 * Parse a Soroban ScVal representing a ScoreRecord struct into the TS interface.
 * The contract returns a struct as an ScMap with ScSymbol keys.
 */
function parseScoreRecord(scVal: xdr.ScVal): ScoreRecord {
  // scValToNative converts ScMap → plain object, ScU32 → number, ScI128 → bigint, etc.
  const raw = scValToNative(scVal) as Record<string, unknown>;

  return {
    score: Number(raw["score"]),
    lastUpdated: Number(raw["last_updated"]),
    vcCount: Number(raw["vc_count"]),
    repaymentRate: Number(raw["repayment_rate"]),
    txVolume30d: BigInt(raw["tx_volume_30d"] as bigint),
  };
}

export default StellarDIDCreditSDK;
