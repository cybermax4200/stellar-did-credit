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
import { xdr } from "@stellar/stellar-sdk";

const mockSimulateTransaction = jest.fn();
const mockGetAccount = jest.fn();
const mockSendTransaction = jest.fn();
const mockGetTransaction = jest.fn();
const mockContractCalls: Array<{
  contractId: string;
  method: string;
  args: unknown[];
}> = [];
let mockLastContractCall:
  | {
      contractId: string;
      method: string;
      args: unknown[];
    }
  | undefined;

jest.mock("@stellar/stellar-sdk", () => ({
  BASE_FEE: "100",
  Networks: {
    TESTNET: "Test SDF Network ; September 2015",
  },
  xdr: {
    ScValType: {
      scvVoid: () => "scvVoid",
    },
    ScVal: {
      scvVoid: () => ({ switch: () => "scvVoid" }),
    },
  },
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
      const call = { contractId, method, args };
      mockLastContractCall = call;
      mockContractCalls.push(call);
      return { method, args };
    },
  })),
  TransactionBuilder: jest.fn().mockImplementation(() => ({
    addOperation: jest.fn().mockReturnThis(),
    setTimeout: jest.fn().mockReturnThis(),
    build: jest.fn().mockReturnValue({ operations: [] }),
  })),
  nativeToScVal: (value: unknown) => ({ value }),
  scValToNative: (scVal: { value: unknown }) => scVal.value,
  SorobanRpc: {
    Server: jest.fn().mockImplementation(() => ({
      simulateTransaction: mockSimulateTransaction,
      getAccount: mockGetAccount,
      sendTransaction: mockSendTransaction,
      getTransaction: mockGetTransaction,
    })),
    Api: {
      isSimulationError: (sim: { error?: string }) => Boolean(sim.error),
      isSimulationSuccess: (sim: { result?: unknown }) => Boolean(sim.result),
    },
  },
}));

jest.mock("@stellar/stellar-sdk/rpc", () => ({
  assembleTransaction: jest.fn().mockReturnValue({
    build: jest.fn().mockReturnValue({
      sign: jest.fn(),
    }),
  }),
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

const issuerKeypair = {
  publicKey: () => "GISSUERAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
};

describe("StellarDIDCreditSDK", () => {
  beforeEach(() => {
    jest.useRealTimers();
    mockSimulateTransaction.mockReset();
    mockGetAccount.mockReset();
    mockSendTransaction.mockReset();
    mockGetTransaction.mockReset();
    mockContractCalls.length = 0;
    mockLastContractCall = undefined;
    mockGetAccount.mockResolvedValue({ sequence: "1" });
    mockSimulateTransaction.mockResolvedValue({ result: {} });
    mockSendTransaction.mockResolvedValue({
      status: "PENDING",
      hash: "mock-tx-hash",
    });
  });

  describe("getDIDDocument", () => {
    it("test_getDIDDocument_returns_cid", async () => {
      const expectedCid = "QmXYZ123abc...";
      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: { value: expectedCid },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.getDIDDocument(subjectAddress);

      expect(result).toBe(expectedCid);
      expect(mockLastContractCall?.method).toBe("get_did_document");
      expect(mockLastContractCall?.args).toHaveLength(1);
    });

    it("test_getDIDDocument_returns_null", async () => {
      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: { value: null },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.getDIDDocument(subjectAddress);

      expect(result).toBeNull();
      expect(mockLastContractCall?.method).toBe("get_did_document");
    });
  });

  describe("revokeVC", () => {
    it("test_revokeVC_calls_revoke_and_mark_vc_revoked", async () => {
      const sdk = new StellarDIDCreditSDK(mockConfig);
      const vcHash = Buffer.alloc(32, 9);

      const result = await sdk.revokeVC(
        issuerKeypair as never,
        subjectAddress,
        vcHash,
      );

      expect(result).toBe("mock-tx-hash");
      expect(mockGetAccount).toHaveBeenCalledWith(issuerKeypair.publicKey());
      expect(mockSendTransaction).toHaveBeenCalled();
      expect(mockContractCalls).toHaveLength(2);
      expect(mockContractCalls[0]).toMatchObject({
        contractId: mockConfig.revocationRegistryId,
        method: "revoke",
      });
      expect(mockContractCalls[0]?.args).toHaveLength(2);
      expect(mockContractCalls[1]).toMatchObject({
        contractId: mockConfig.identityOracleId,
        method: "mark_vc_revoked",
      });
      expect(mockContractCalls[1]?.args).toHaveLength(3);
    });

    it("rejects non-32-byte credential hashes", async () => {
      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(
        sdk.revokeVC(issuerKeypair as never, subjectAddress, Buffer.alloc(31)),
      ).rejects.toThrow("vcHash must be exactly 32 bytes");
      expect(mockGetAccount).not.toHaveBeenCalled();
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

  describe("computeScore", () => {
    it("test_computeScore_returns_updated_record", async () => {
      // Verify that computeScore submits compute_score, waits for confirmation,
      // then returns the updated ScoreRecord from getScore.
      mockGetAccount.mockResolvedValue({ sequence: "10" });
      mockSendTransaction.mockResolvedValue({
        status: "PENDING",
        hash: "tx-compute-hash",
      });
      mockGetTransaction.mockResolvedValue({ status: "SUCCESS" });
      mockSimulateTransaction
        .mockResolvedValueOnce({ result: { retval: { value: null } } }) // compute_score sim
        .mockResolvedValueOnce({
          result: {
            retval: {
              value: {
                score: 558,
                last_updated: 1_710_000_000,
                vc_count: 2,
                repayment_rate: 8500,
                tx_volume_30d: 2_000_000n,
              },
            },
          },
        }); // getScore sim

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.computeScore(
        { publicKey: () => subjectAddress } as any,
        subjectAddress,
      );

      expect(result).toEqual({
        score: 558,
        lastUpdated: 1_710_000_000,
        vcCount: 2,
        repaymentRate: 8500,
        txVolume30d: 2_000_000n,
      });
      expect(mockSendTransaction).toHaveBeenCalledTimes(1);
      expect(mockGetTransaction).toHaveBeenCalledWith("tx-compute-hash");
      expect(mockLastContractCall?.method).toBe("get_score");
    });

    it("polls getTransaction until SUCCESS before reading the stored score", async () => {
      jest.useFakeTimers();
      mockGetAccount.mockResolvedValue({ sequence: "123" });
      mockSendTransaction.mockResolvedValue({
        status: "PENDING",
        hash: "tx-hash-1",
      });
      mockGetTransaction
        .mockResolvedValueOnce({ status: "PENDING" })
        .mockResolvedValueOnce({ status: "SUCCESS" });
      mockSimulateTransaction
        .mockResolvedValueOnce({ result: { retval: { value: null } } })
        .mockResolvedValueOnce({
          result: {
            retval: {
              value: {
                score: 612,
                last_updated: 1_700_000_000,
                vc_count: 3,
                repayment_rate: 8000,
                tx_volume_30d: 1_000_000n,
              },
            },
          },
        });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const computePromise = sdk.computeScore(
        { publicKey: () => subjectAddress } as any,
        subjectAddress,
      );

      await Promise.resolve();
      await jest.advanceTimersByTimeAsync(1000);

      await expect(computePromise).resolves.toEqual({
        score: 612,
        lastUpdated: 1_700_000_000,
        vcCount: 3,
        repaymentRate: 8000,
        txVolume30d: 1_000_000n,
      });
      expect(mockGetTransaction).toHaveBeenCalledTimes(2);
      expect(mockLastContractCall?.method).toBe("get_score");
    });

    it("throws a descriptive error when fetching the stored score fails after confirmation", async () => {
      mockGetAccount.mockResolvedValue({ sequence: "123" });
      mockSendTransaction.mockResolvedValue({
        status: "PENDING",
        hash: "tx-hash-2",
      });
      mockGetTransaction.mockResolvedValue({ status: "SUCCESS" });
      mockSimulateTransaction
        .mockResolvedValueOnce({ result: {} })
        .mockResolvedValueOnce({ error: "score not computed" });

      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(
        sdk.computeScore(
          { publicKey: () => subjectAddress } as any,
          subjectAddress,
        ),
      ).rejects.toThrow(
        `computeScore transaction succeeded and was confirmed, but fetching the stored score for ${subjectAddress} failed: No score computed for address: ${subjectAddress}`,
      );
      expect(mockGetTransaction).toHaveBeenCalledTimes(1);
    });

    it("throws a descriptive error when the submitted transaction FAILS", async () => {
      mockGetAccount.mockResolvedValue({ sequence: "123" });
      mockSendTransaction.mockResolvedValue({
        status: "PENDING",
        hash: "tx-hash-3",
      });
      mockGetTransaction.mockResolvedValue({
        status: "FAILED",
        errorResult: "tx_bad_auth",
      });
      mockSimulateTransaction.mockResolvedValue({
        result: { retval: { value: null } },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(
        sdk.computeScore(
          { publicKey: () => subjectAddress } as any,
          subjectAddress,
        ),
      ).rejects.toThrow(
        'computeScore transaction failed for tx-hash-3: {"status":"FAILED","errorResult":"tx_bad_auth"}',
      );
      expect(mockGetTransaction).toHaveBeenCalledTimes(1);
    });
  });

  describe("getVCCount", () => {
    it("test_getVCCount_returns_active_count", async () => {
      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: { value: 3 },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.getVCCount(subjectAddress);

      expect(result).toBe(3);
      expect(mockLastContractCall?.method).toBe("get_active_vc_count");
      expect(mockLastContractCall?.args).toHaveLength(1);
    });

    it("test_getVCCount_returns_zero", async () => {
      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: { value: 0 },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.getVCCount(subjectAddress);

      expect(result).toBe(0);
      expect(mockLastContractCall?.method).toBe("get_active_vc_count");
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

    it("returns null when contract returns None (scvVoid)", async () => {
      const sdk = new StellarDIDCreditSDK(mockConfig);

      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: xdr.ScVal.scvVoid(),
        },
      });

      const result = await sdk.getScore(subjectAddress);
      expect(result).toBeNull();
    });

    it("exports ScoreNotComputedError class", () => {
      expect(ScoreNotComputedError).toBeDefined();
      const error = new ScoreNotComputedError(subjectAddress);
      expect(error).toBeInstanceOf(Error);
      expect(error.name).toBe("ScoreNotComputedError");
      expect(error.message).toContain(subjectAddress);
    });
  });

  describe("ProtocolConfig — timeoutSeconds, maxRetries, baseFee", () => {
    it("test_timeout_applied_to_transaction_builder", async () => {
      // Arrange: custom timeoutSeconds; provide a valid sim response so getScore completes.
      const scoreRetval = {
        value: {
          score: 300,
          last_updated: 0,
          vc_count: 0,
          repayment_rate: 0,
          tx_volume_30d: 0n,
        },
      };
      mockSimulateTransaction.mockResolvedValue({
        result: { retval: scoreRetval },
      });

      const { TransactionBuilder } = jest.requireMock("@stellar/stellar-sdk");
      const setTimeoutSpy = jest.fn().mockReturnThis();
      TransactionBuilder.mockImplementationOnce(() => ({
        addOperation: jest.fn().mockReturnThis(),
        setTimeout: setTimeoutSpy,
        build: jest.fn().mockReturnValue({ operations: [] }),
      }));

      const sdk = new StellarDIDCreditSDK({ ...mockConfig, timeoutSeconds: 60 });
      await sdk.getScore(subjectAddress);

      expect(setTimeoutSpy).toHaveBeenCalledWith(60);
    });

    it("test_retry_succeeds_after_n_failures", async () => {
      jest.useFakeTimers();
      // Arrange: first 2 calls return a non-success/non-error response
      // (simulates a transient RPC glitch), 3rd call succeeds.
      const TRANSIENT = {}; // neither error nor success
      const SUCCESS = {
        result: {
          retval: {
            value: {
              score: 500,
              last_updated: 1_700_000_000,
              vc_count: 1,
              repayment_rate: 7000,
              tx_volume_30d: 500_000n,
            },
          },
        },
      };
      mockSimulateTransaction
        .mockResolvedValueOnce(TRANSIENT)
        .mockResolvedValueOnce(TRANSIENT)
        .mockResolvedValueOnce(SUCCESS);

      const sdk = new StellarDIDCreditSDK({ ...mockConfig, maxRetries: 3 });
      const promise = sdk.getScore(subjectAddress);

      // Advance timers past the two backoff sleeps (500ms + 1000ms)
      await jest.advanceTimersByTimeAsync(2000);

      const result = await promise;
      expect(result?.score).toBe(500);
      // simulateTransaction must have been called 3 times (2 transient + 1 success)
      expect(mockSimulateTransaction).toHaveBeenCalledTimes(3);
    });

    it("test_retry_exhausted_throws_after_maxRetries", async () => {
      // Use real timers but set maxRetries=0 so there are no sleeps at all —
      // the loop runs exactly once (attempt 0), fails, and throws immediately.
      const TRANSIENT = {};
      mockSimulateTransaction.mockResolvedValue(TRANSIENT);

      const sdk = new StellarDIDCreditSDK({ ...mockConfig, maxRetries: 0 });
      await expect(sdk.getScore(subjectAddress)).rejects.toThrow(
        "Simulation returned unexpected response",
      );
      // maxRetries=0 → 1 total attempt (just attempt 0)
      expect(mockSimulateTransaction).toHaveBeenCalledTimes(1);
    });

    it("test_custom_baseFee_forwarded_to_transaction_builder", async () => {
      // Provide a valid sim response so getScore completes without error.
      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: {
            value: {
              score: 300,
              last_updated: 0,
              vc_count: 0,
              repayment_rate: 0,
              tx_volume_30d: 0n,
            },
          },
        },
      });

      const { TransactionBuilder } = jest.requireMock("@stellar/stellar-sdk");
      let capturedFee: string | undefined;
      TransactionBuilder.mockImplementationOnce(
        (_account: unknown, opts: { fee: string }) => {
          capturedFee = opts.fee;
          return {
            addOperation: jest.fn().mockReturnThis(),
            setTimeout: jest.fn().mockReturnThis(),
            build: jest.fn().mockReturnValue({ operations: [] }),
          };
        },
      );

      const sdk = new StellarDIDCreditSDK({ ...mockConfig, baseFee: "500" });
      await sdk.getScore(subjectAddress);

      expect(capturedFee).toBe("500");
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
