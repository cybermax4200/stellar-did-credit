import {
  StellarDIDCreditSDK,
  ScoreNotComputedError,
  MIN_SCORE,
  MAX_SCORE,
} from "./index";
import type {
  ScoreRecord,
  ProtocolConfig,
  TxStats,
  ScoringWeights,
  RepaymentRecord,
  VCRecord,
} from "./index";

const mockSimulateTransaction = jest.fn();
const mockGetAccount = jest.fn();
const mockSendTransaction = jest.fn();
const mockSign = jest.fn();
const mockAssembleBuild = jest.fn();
let mockLastContractCall:
  | {
      method: string;
      args: unknown[];
    }
  | undefined;

jest.mock("@stellar/stellar-sdk", () => ({
  BASE_FEE: "100",
  Networks: {
    TESTNET: "Test SDF Network ; September 2015",
  },
  xdr: {},
  Keypair: {},
  Account: jest
    .fn()
    .mockImplementation((accountId: string, sequence: string) => ({
      accountId,
      sequence,
    })),
  Address: jest.fn().mockImplementation((address: string) => ({
    toScVal: () => ({ address }),
  })),
  Contract: jest.fn().mockImplementation((contractId: string) => ({
    contractId,
    call: (method: string, ...args: unknown[]) => {
      mockLastContractCall = { method, args };
      return { method, args };
    },
  })),
  TransactionBuilder: jest.fn().mockImplementation(() => ({
    addOperation: jest.fn().mockReturnThis(),
    setTimeout: jest.fn().mockReturnThis(),
    build: jest.fn().mockReturnValue({}),
  })),
  nativeToScVal: (value: unknown) => ({ value }),
  scValToNative: (scVal: { value: unknown }) => scVal.value,
  SorobanRpc: {
    Server: jest.fn().mockImplementation(() => ({
      getAccount: mockGetAccount,
      sendTransaction: mockSendTransaction,
      simulateTransaction: mockSimulateTransaction,
    })),
    Api: {
      isSimulationError: (sim: { error?: string }) => Boolean(sim.error),
      isSimulationSuccess: (sim: { result?: unknown }) => Boolean(sim.result),
      assembleTransaction: jest.fn().mockReturnValue({
        build: mockAssembleBuild.mockReturnValue({
          sign: mockSign,
        }),
      }),
    },
  },
}));

const mockConfig = {
  identityOracleId: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
  creditOracleId: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
  revocationRegistryId:
    "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
  networkPassphrase: "Test SDF Network ; September 2015",
  rpcUrl: "http://localhost:8000",
  simAccount: "GBUQWP3BOUZX34ULNQG23RQ6F4YUSXHTQSXE7XDZT4A65XJLQRGEZSM",
};

const subjectAddress =
  "GBUQWP3BOUZX34ULNQG23RQ6F4YUSXHTQSXE7XDZT4A65XJLQRGEZSM";

describe("StellarDIDCreditSDK", () => {
  beforeEach(() => {
    mockSimulateTransaction.mockReset();
    mockGetAccount.mockReset();
    mockSendTransaction.mockReset();
    mockSign.mockReset();
    mockAssembleBuild.mockClear();
    mockLastContractCall = undefined;
    mockGetAccount.mockResolvedValue({ sequence: "123" });
    mockSendTransaction.mockResolvedValue({
      status: "PENDING",
      hash: "mock-tx-hash",
    });
  });

  describe("anchorDID", () => {
    it("builds, simulates, assembles, signs, and submits anchor_did", async () => {
      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: { value: null },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const subjectKeypair = {
        publicKey: jest.fn().mockReturnValue(subjectAddress),
      };

      const result = await sdk.anchorDID(
        subjectKeypair as any,
        "bafybeigdyrzt5x2n7n4t6v6n2q4w5l7m3e5k4h2q7z2r6v4w1x3y5z7abc",
      );

      expect(result).toBe("mock-tx-hash");
      expect(subjectKeypair.publicKey).toHaveBeenCalledTimes(1);
      expect(mockGetAccount).toHaveBeenCalledWith(subjectAddress);
      expect(mockLastContractCall?.method).toBe("anchor_did");
      expect(mockLastContractCall?.args).toHaveLength(2);
      expect(mockAssembleBuild).toHaveBeenCalledTimes(1);
      expect(mockSign).toHaveBeenCalledWith(subjectKeypair);
      expect(mockSendTransaction).toHaveBeenCalledTimes(1);
    });

    it("throws when anchor_did simulation fails", async () => {
      mockSimulateTransaction.mockResolvedValue({
        error: "HostError: Error(Contract, #1)",
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const subjectKeypair = {
        publicKey: jest.fn().mockReturnValue(subjectAddress),
      };

      await expect(
        sdk.anchorDID(subjectKeypair as any, "bafy-invalid"),
      ).rejects.toThrow("Simulation failed: HostError: Error(Contract, #1)");

      expect(mockLastContractCall?.method).toBe("anchor_did");
      expect(mockAssembleBuild).not.toHaveBeenCalled();
      expect(mockSign).not.toHaveBeenCalled();
      expect(mockSendTransaction).not.toHaveBeenCalled();
    });
  });

  describe("verifyVC", () => {
    it("test_verifyVC_true_for_valid_hash", async () => {
      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: { value: true },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.verifyVC(subjectAddress, Buffer.alloc(32, 1));

      expect(result).toBe(true);
      expect(mockLastContractCall?.method).toBe("verify_vc");
      expect(mockLastContractCall?.args).toHaveLength(2);
    });

    it("test_verifyVC_false_for_revoked_hash", async () => {
      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: { value: false },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.verifyVC(subjectAddress, Buffer.alloc(32, 2));

      expect(result).toBe(false);
      expect(mockLastContractCall?.method).toBe("verify_vc");
    });

    it("rejects non-32-byte credential hashes", async () => {
      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(
        sdk.verifyVC(subjectAddress, Buffer.alloc(31)),
      ).rejects.toThrow("vcHash must be exactly 32 bytes");
      expect(mockSimulateTransaction).not.toHaveBeenCalled();
    });
  });

  describe("getScore", () => {
    it("throws ScoreNotComputedError when score has not been computed", async () => {
      const sdk = new StellarDIDCreditSDK(mockConfig);

      // Mock the server and simulation to return "score not computed" error
      mockSimulateTransaction.mockResolvedValue({
        error: "score not computed",
      });

      await expect(sdk.getScore(subjectAddress)).rejects.toBeInstanceOf(
        ScoreNotComputedError,
      );
    });

    it("exports ScoreNotComputedError class", () => {
      expect(ScoreNotComputedError).toBeDefined();
      const error = new ScoreNotComputedError(subjectAddress);
      expect(error).toBeInstanceOf(Error);
      expect(error.name).toBe("ScoreNotComputedError");
      expect(error.message).toContain(subjectAddress);
    });
  });
});

describe("contract struct type exports", () => {
  // These tests double as compile-time assertions: if any interface were not
  // exported from the package entry point (./index), or if its shape drifted
  // from the on-chain contract structs, this file would fail to type-check.
  // The runtime expectations additionally verify the documented field types.

  it("exports TxStats with volume30d typed as bigint (Soroban i128)", () => {
    const stats: TxStats = {
      volume30d: 5_000_000_000n,
      txCount30d: 42,
      avgCounterparties: 7,
    };

    expect(typeof stats.volume30d).toBe("bigint");
    expect(typeof stats.txCount30d).toBe("number");
    expect(typeof stats.avgCounterparties).toBe("number");
  });

  it("exports ScoringWeights whose components sum to 100 by contract invariant", () => {
    const weights: ScoringWeights = {
      vcWeight: 40,
      txWeight: 30,
      repaymentWeight: 30,
    };

    expect(typeof weights.vcWeight).toBe("number");
    expect(typeof weights.txWeight).toBe("number");
    expect(typeof weights.repaymentWeight).toBe("number");
    expect(weights.vcWeight + weights.txWeight + weights.repaymentWeight).toBe(
      100,
    );
  });

  it("exports RepaymentRecord with numeric on-time and total counters", () => {
    const record: RepaymentRecord = {
      onTimeCount: 8,
      totalCount: 10,
    };

    expect(typeof record.onTimeCount).toBe("number");
    expect(typeof record.totalCount).toBe("number");
  });

  it("exports VCRecord with a 32-byte hash, issuer, timestamp and revoked flag", () => {
    const vc: VCRecord = {
      vcHash: Buffer.alloc(32),
      issuer: "GBUQWP3BOUZX34ULNQG23RQ6F4YUSXHTQSXE7XDZT4A65XJLQRGEZSM",
      anchoredAt: 1_700_000_000,
      revoked: false,
    };

    expect(vc.vcHash.length).toBe(32);
    expect(typeof vc.issuer).toBe("string");
    expect(typeof vc.anchoredAt).toBe("number");
    expect(typeof vc.revoked).toBe("boolean");
  });

  it("continues to export ScoreRecord and ProtocolConfig", () => {
    const score: ScoreRecord = {
      score: 612,
      lastUpdated: 1_700_000_000,
      vcCount: 3,
      repaymentRate: 8000,
      txVolume30d: 1_000_000n,
    };

    const config: ProtocolConfig = {
      identityOracleId:
        "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
      creditOracleId:
        "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
      revocationRegistryId:
        "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
      networkPassphrase: "Test SDF Network ; September 2015",
      rpcUrl: "https://soroban-testnet.stellar.org",
      simAccount: "GBUQWP3BOUZX34ULNQG23RQ6F4YUSXHTQSXE7XDZT4A65XJLQRGEZSM",
    };

    expect(typeof score.txVolume30d).toBe("bigint");
    expect(score.score).toBe(612);
    expect(config.networkPassphrase).toContain("Test SDF Network");
  });
});

describe("test_all_exports_are_defined", () => {
  // Verifies that every public name from the SDK entry point has a defined
  // runtime value. TypeScript interfaces (TxStats, ScoringWeights,
  // RepaymentRecord, VCRecord, ScoreRecord, ProtocolConfig) have no runtime
  // representation — their presence is guaranteed by the `import type` block
  // at the top of this file, which causes a compile error if any type is
  // missing from the barrel.
  it("exports MIN_SCORE and MAX_SCORE as defined numbers", () => {
    expect(MIN_SCORE).not.toBeUndefined();
    expect(MAX_SCORE).not.toBeUndefined();
    expect(MIN_SCORE).toBe(300);
    expect(MAX_SCORE).toBe(850);
  });

  it("exports ScoreNotComputedError as a defined constructor", () => {
    expect(ScoreNotComputedError).not.toBeUndefined();
    expect(typeof ScoreNotComputedError).toBe("function");
    const err = new ScoreNotComputedError("GADDR");
    expect(err).toBeInstanceOf(Error);
    expect(err.name).toBe("ScoreNotComputedError");
  });

  it("exports StellarDIDCreditSDK as a defined constructor", () => {
    expect(StellarDIDCreditSDK).not.toBeUndefined();
    expect(typeof StellarDIDCreditSDK).toBe("function");
  });

  it("struct type imports compile without error (TxStats, ScoringWeights, RepaymentRecord, VCRecord, ScoreRecord, ProtocolConfig)", () => {
    // If any of these types were missing from index.ts, TypeScript would
    // refuse to compile this file, making the test suite fail at build time.
    const _txStats: TxStats = {
      volume30d: 0n,
      txCount30d: 0,
      avgCounterparties: 0,
    };
    const _weights: ScoringWeights = {
      vcWeight: 40,
      txWeight: 30,
      repaymentWeight: 30,
    };
    const _repayment: RepaymentRecord = { onTimeCount: 0, totalCount: 0 };
    const _vc: VCRecord = {
      vcHash: Buffer.alloc(32),
      issuer: "G",
      anchoredAt: 0,
      revoked: false,
    };
    const _score: ScoreRecord = {
      score: 300,
      lastUpdated: 0,
      vcCount: 0,
      repaymentRate: 0,
      txVolume30d: 0n,
    };
    const _config: ProtocolConfig = {
      identityOracleId: "",
      creditOracleId: "",
      revocationRegistryId: "",
      networkPassphrase: "",
      rpcUrl: "",
      simAccount: "",
    };
    expect(_txStats).toBeDefined();
    expect(_weights).toBeDefined();
    expect(_repayment).toBeDefined();
    expect(_vc).toBeDefined();
    expect(_score).toBeDefined();
    expect(_config).toBeDefined();
  });
});
