import {
  StellarDIDCreditSDK,
  GovernanceClient,
  SDKError,
  ScoreNotComputedError,
  MIN_SCORE,
  MAX_SCORE,
  ScoreRecord,
  ProtocolConfig,
  TxStats,
  ScoringWeights,
  RepaymentRecord,
  VCRecord,
  GovernanceProposal,
} from "./index";
import { xdr, Keypair } from "@stellar/stellar-sdk";

const mockSimulateTransaction = jest.fn();
const mockGetAccount = jest.fn();
const mockSendTransaction = jest.fn();
const mockGetTransaction = jest.fn();
const mockGetEvents = jest.fn();
const mockGetLatestLedger = jest.fn();
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
      scvSymbol: (symbol: string) => ({
        toXDR: () => `symbol:${symbol}`,
      }),
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
  nativeToScVal: (value: unknown, options?: { type?: unknown }) => ({
    value,
    type: options?.type,
  }),
  scValToNative: (scVal: { value?: unknown }) => scVal?.value,
  SorobanRpc: {
    Server: jest.fn().mockImplementation(() => ({
      getAccount: mockGetAccount,
      sendTransaction: mockSendTransaction,
      simulateTransaction: mockSimulateTransaction,
      getTransaction: mockGetTransaction,
      getEvents: mockGetEvents,
      getLatestLedger: mockGetLatestLedger,
    })),
    assembleTransaction: jest.fn().mockReturnValue({
      build: jest.fn().mockReturnValue({
        sign: jest.fn(),
      }),
    }),
    Api: {
      isSimulationError: (sim: { error?: string }) => Boolean(sim?.error),
      isSimulationSuccess: (sim: { result?: unknown }) => Boolean(sim?.result),
    },
  },
}));

const mockConfig = {
  identityOracleId: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
  creditOracleId: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
  revocationRegistryId:
    "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
  governanceId:
    "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
  networkPassphrase: "Test SDF Network ; September 2015",
  rpcUrl: "http://localhost:8000",
  simAccount: "GBUQWP3BOUZX34ULNQG23RQ6F4YUSXHTQSXE7XDZT4A65XJLQRGEZSM",
};

const subjectAddress =
  "GBUQWP3BOUZX34ULNQG23RQ6F4YUSXHTQSXE7XDZT4A65XJLQRGEZSM";

const subjectKeypair = {
  publicKey: () => subjectAddress,
};

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
    mockGetEvents.mockReset();
    mockGetLatestLedger.mockReset();
    mockContractCalls.length = 0;
    mockLastContractCall = undefined;
    mockGetAccount.mockResolvedValue({ sequenceNumber: () => "1" });
    mockSimulateTransaction.mockResolvedValue({ result: {} });
    mockSendTransaction.mockResolvedValue({
      status: "PENDING",
      hash: "mock-tx-hash",
    });
    mockGetTransaction.mockResolvedValue({ status: "SUCCESS" });
    (jest.requireMock("@stellar/stellar-sdk").SorobanRpc.Server as jest.Mock).mockClear();
  });

  describe("governance", () => {
    const governanceWeights: ScoringWeights = {
      vcWeight: 50,
      txWeight: 25,
      repaymentWeight: 25,
    };

    beforeEach(() => {
      mockGetTransaction.mockResolvedValue({ status: "SUCCESS" });
    });

    it("creates a proposal with the governance contract and returns its ID", async () => {
      mockSimulateTransaction.mockResolvedValue({
        result: { retval: { value: 7n } },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const proposalId = await sdk.governance.createProposal(
        subjectKeypair as never,
        governanceWeights,
        100,
        50,
      );

      expect(proposalId).toBe(7n);
      expect(mockGetAccount).toHaveBeenCalledWith(subjectAddress);
      expect(mockGetTransaction).toHaveBeenCalledWith("mock-tx-hash");
      expect(mockContractCalls[0]).toMatchObject({
        contractId: mockConfig.governanceId,
        method: "create_proposal",
      });
      expect(mockContractCalls[0]?.args).toHaveLength(4);
      expect(mockContractCalls[0]?.args[2]).toMatchObject({
        value: 100,
        type: "u32",
      });
      expect(mockContractCalls[0]?.args[3]).toMatchObject({
        value: 50,
        type: "u32",
      });
    });

    it("casts a vote with explicit u64 and i128 arguments", async () => {
      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(
        sdk.governance.vote(subjectKeypair as never, 7n, true, 100n),
      ).resolves.toBe("mock-tx-hash");

      expect(mockContractCalls[0]).toMatchObject({
        contractId: mockConfig.governanceId,
        method: "vote",
      });
      expect(mockContractCalls[0]?.args).toHaveLength(4);
      expect(mockContractCalls[0]?.args[1]).toMatchObject({
        value: 7n,
        type: "u64",
      });
      expect(mockContractCalls[0]?.args[3]).toMatchObject({
        value: 100n,
        type: "i128",
      });
      expect(mockGetAccount).toHaveBeenCalledWith(subjectAddress);
    });

    it("decodes a governance proposal and returns null for an absent ID", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        result: {
          retval: {
            value: {
              id: 7n,
              proposer: subjectAddress,
              proposed_weights: {
                vc_weight: 50,
                tx_weight: 25,
                repayment_weight: 25,
              },
              votes_for: 100n,
              votes_against: 20n,
              expiry_ledger: 120,
              execution_delay_ledgers: 50,
              executed: false,
              cancelled: false,
              quorum_required: 100n,
            },
          },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const proposal = await sdk.governance.getProposal(7n);

      expect(proposal).toEqual<GovernanceProposal>({
        id: 7n,
        proposer: subjectAddress,
        proposedWeights: governanceWeights,
        votesFor: 100n,
        votesAgainst: 20n,
        expiryLedger: 120,
        executionDelayLedgers: 50,
        executed: false,
        cancelled: false,
        quorumRequired: 100n,
      });
      expect(mockLastContractCall?.method).toBe("get_proposal");

      mockSimulateTransaction.mockResolvedValueOnce({
        result: { retval: { value: null } },
      });
      await expect(sdk.governance.getProposal(999n)).resolves.toBeNull();
    });

    it("executes and applies weights through signed governance calls", async () => {
      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(sdk.governance.execute(subjectKeypair as never, 7n)).resolves
        .toBe("mock-tx-hash");
      await expect(sdk.governance.applyWeights(subjectKeypair as never)).resolves
        .toBe("mock-tx-hash");

      expect(mockContractCalls.map((call) => call.method)).toEqual([
        "execute",
        "apply_weights",
      ]);
    });

    it("lists proposals by scanning proposal IDs", async () => {
      mockSimulateTransaction
        .mockResolvedValueOnce({
          result: {
            retval: {
              value: {
                id: 3n,
                proposer: subjectAddress,
                proposed_weights: {
                  vc_weight: 50,
                  tx_weight: 25,
                  repayment_weight: 25,
                },
                votes_for: 1n,
                votes_against: 0n,
                expiry_ledger: 10,
                execution_delay_ledgers: 0,
                executed: false,
                cancelled: false,
                quorum_required: 1n,
              },
            },
          },
        })
        .mockResolvedValueOnce({ result: { retval: { value: null } } });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const proposals = await sdk.governance.listProposals(3n, 2);

      expect(proposals).toHaveLength(1);
      expect(proposals[0]?.id).toBe(3n);
      expect(mockContractCalls.map((call) => call.method)).toEqual([
        "get_proposal",
        "get_proposal",
      ]);
    });

    it("exports GovernanceClient and requires governanceId", async () => {
      expect(GovernanceClient).toBeDefined();
      const sdk = new StellarDIDCreditSDK({
        ...mockConfig,
        governanceId: undefined,
      });

      await expect(sdk.governance.getProposal(1n)).rejects.toThrow(
        "governanceId is required",
      );
    });
  });

  describe("RPC server instance reuse", () => {
    it("constructs a single SorobanRpc.Server across multiple SDK calls", async () => {
      const { SorobanRpc } = jest.requireMock("@stellar/stellar-sdk");
      const serverMock = SorobanRpc.Server as jest.Mock;

      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: {
            value: {
              score: 612,
              last_updated: 1_700_000_000,
              vc_count: 3,
              repayment_rate: 8000,
              tx_volume_30d: 1_000_000n,
              previous_score: null,
              computed_at_ledger: 1234567,
              stale: false,
            },
          },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      expect(serverMock).toHaveBeenCalledTimes(1);

      await sdk.getScore(subjectAddress);
      await sdk.getDIDDocument(subjectAddress);
      await sdk.getVCCount(subjectAddress);
      await sdk.getWeights();
      await sdk.isVerified(subjectAddress);

      expect(serverMock).toHaveBeenCalledTimes(1);
    });
  });

  describe("event subscriptions", () => {
    const issuer = "GISSUERAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    const vcHash = Buffer.alloc(32, 7);

    beforeEach(() => {
      jest.useFakeTimers();
      mockGetLatestLedger.mockResolvedValue({ sequence: 100 });
    });

    afterEach(() => {
      jest.useRealTimers();
    });

    it("polls VCAnch pages, decodes each event, and stops after unsubscribe", async () => {
      mockGetEvents
        .mockResolvedValueOnce({
          latestLedger: 101,
          events: [
            {
              ledger: 100,
              value: {
                value: [issuer, subjectAddress, vcHash],
              },
            },
          ],
        })
        .mockResolvedValueOnce({
          latestLedger: 102,
          events: [
            {
              ledger: 102,
              value: {
                value: [issuer, subjectAddress, Buffer.alloc(32, 8)],
              },
            },
          ],
        });

      const sdk = new StellarDIDCreditSDK({
        ...mockConfig,
        pollIntervalMs: 10,
      });
      const received: Array<[string, string, Buffer]> = [];
      const unsubscribe = sdk.onVCAnchored(
        mockConfig.identityOracleId,
        (eventIssuer, eventSubject, eventHash) => {
          received.push([eventIssuer, eventSubject, eventHash]);
        },
      );

      await Promise.resolve();
      await Promise.resolve();
      expect(received).toEqual([[issuer, subjectAddress, vcHash]]);
      expect(mockGetEvents).toHaveBeenCalledWith(
        expect.objectContaining({
          startLedger: 100,
          filters: [
            {
              type: "contract",
              contractIds: [mockConfig.identityOracleId],
              topics: [["symbol:VCAnch"]],
            },
          ],
        }),
      );

      await jest.advanceTimersByTimeAsync(10);
      expect(received).toEqual([
        [issuer, subjectAddress, vcHash],
        [issuer, subjectAddress, Buffer.alloc(32, 8)],
      ]);
      expect(mockGetEvents).toHaveBeenLastCalledWith(
        expect.objectContaining({ startLedger: 102 }),
      );

      unsubscribe();
      await jest.advanceTimersByTimeAsync(10);
      expect(mockGetEvents).toHaveBeenCalledTimes(2);
    });

    it("decodes Score and Revoked event tuples", async () => {
      mockGetEvents
        .mockResolvedValueOnce({
          latestLedger: 100,
          events: [
            {
              ledger: 100,
              value: { value: [subjectAddress, 612] },
            },
          ],
        })
        .mockResolvedValueOnce({
          latestLedger: 100,
          events: [
            {
              ledger: 100,
              value: { value: [issuer, vcHash] },
            },
          ],
        });

      const sdk = new StellarDIDCreditSDK({
        ...mockConfig,
        pollIntervalMs: 10,
      });
      const scoreCallback = jest.fn();
      const revokeCallback = jest.fn();
      const stopScore = sdk.onScoreComputed(
        mockConfig.creditOracleId,
        scoreCallback,
      );
      const stopRevoke = sdk.onVCRevoked(
        mockConfig.revocationRegistryId,
        revokeCallback,
      );

      await Promise.resolve();
      await Promise.resolve();
      expect(scoreCallback).toHaveBeenCalledWith(subjectAddress, 612);
      expect(revokeCallback).toHaveBeenCalledWith(issuer, vcHash);

      stopScore();
      stopRevoke();
    });

    it("rejects an invalid polling interval", () => {
      const sdk = new StellarDIDCreditSDK({
        ...mockConfig,
        pollIntervalMs: 0,
      });

      expect(() =>
        sdk.onScoreComputed(mockConfig.creditOracleId, jest.fn()),
      ).toThrow("pollIntervalMs must be a positive number");
    });
  });

  describe("getDIDDocument", () => {
    it("returns null when no DID is anchored", async () => {
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

    it("returns the CID when a DID is anchored", async () => {
      const expectedCid = "ipfs://QmTestDID123";
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

    it("throws when simulation returns an error", async () => {
      mockSimulateTransaction.mockResolvedValue({
        error: "contract error",
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(sdk.getDIDDocument(subjectAddress)).rejects.toThrow(
        "Simulation failed: contract error",
      );
    });
  });

  describe("revokeVC", () => {
    it("calls the registry once and waits for the atomic revocation to succeed", async () => {
      mockGetTransaction.mockResolvedValue({ status: "SUCCESS" });
      const sdk = new StellarDIDCreditSDK(mockConfig);
      const vcHash = Buffer.alloc(32, 9);

      const result = await sdk.revokeVC(issuerKeypair as never, vcHash);

      expect(result).toBe("mock-tx-hash");
      expect(mockGetAccount).toHaveBeenCalledWith(issuerKeypair.publicKey());
      expect(mockSendTransaction).toHaveBeenCalled();
      expect(mockGetTransaction).toHaveBeenCalledWith("mock-tx-hash");
      expect(mockContractCalls).toHaveLength(1);
      expect(mockContractCalls[0]).toMatchObject({
        contractId: mockConfig.revocationRegistryId,
        method: "revoke",
      });
      expect(mockContractCalls[0]?.args).toHaveLength(2);
    });

    it("rejects non-32-byte credential hashes", async () => {
      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(
        sdk.revokeVC(issuerKeypair as never, Buffer.alloc(31)),
      ).rejects.toMatchObject({
        name: "SDKError",
        code: "INVALID_VC_HASH",
        message: "vcHash must be exactly 32 bytes",
      });
      expect(mockGetAccount).not.toHaveBeenCalled();
      expect(mockSendTransaction).not.toHaveBeenCalled();
    });

    it("rejects a missing revocation registry configuration", async () => {
      const sdk = new StellarDIDCreditSDK({
        ...mockConfig,
        revocationRegistryId: "",
      });

      await expect(
        sdk.revokeVC(issuerKeypair as never, Buffer.alloc(32)),
      ).rejects.toMatchObject({
        name: "SDKError",
        code: "MISSING_REVOCATION_REGISTRY",
      });
      expect(mockGetAccount).not.toHaveBeenCalled();
      expect(mockSendTransaction).not.toHaveBeenCalled();
    });

    it("maps IssuerMismatch simulation failures to NOT_REGISTERED_ISSUER", async () => {
      mockSimulateTransaction.mockResolvedValue({
        error: "IssuerMismatch",
      });
      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(
        sdk.revokeVC(issuerKeypair as never, Buffer.alloc(32, 4)),
      ).rejects.toMatchObject({
        name: "SDKError",
        code: "NOT_REGISTERED_ISSUER",
      });
      expect(mockSendTransaction).not.toHaveBeenCalled();
    });

    it("maps confirmed IssuerMismatch failures to NOT_REGISTERED_ISSUER", async () => {
      mockGetTransaction.mockResolvedValue({
        status: "FAILED",
        errorResult: "IssuerMismatch",
      });
      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(
        sdk.revokeVC(issuerKeypair as never, Buffer.alloc(32, 4)),
      ).rejects.toMatchObject({
        name: "SDKError",
        code: "NOT_REGISTERED_ISSUER",
      });
    });

    it("polls a pending transaction until it succeeds", async () => {
      mockGetTransaction
        .mockResolvedValueOnce({ status: "PENDING" })
        .mockResolvedValueOnce({ status: "SUCCESS" });
      const sdk = new StellarDIDCreditSDK({
        ...mockConfig,
        confirmationTimeoutMs: 100,
        pollIntervalMs: 1,
      });

      await expect(
        sdk.revokeVC(issuerKeypair as never, Buffer.alloc(32, 7)),
      ).resolves.toBe("mock-tx-hash");
      expect(mockGetTransaction).toHaveBeenCalledTimes(2);
    });

    it("throws a typed error when confirmation times out", async () => {
      mockGetTransaction.mockResolvedValue({ status: "PENDING" });
      const sdk = new StellarDIDCreditSDK({
        ...mockConfig,
        confirmationTimeoutMs: 0,
        pollIntervalMs: 1,
      });

      await expect(
        sdk.revokeVC(issuerKeypair as never, Buffer.alloc(32, 8)),
      ).rejects.toMatchObject({
        name: "SDKError",
        code: "TRANSACTION_TIMEOUT",
      });
    });

    it("reports simulation failures before changing either contract", async () => {
      mockSimulateTransaction.mockResolvedValue({
        error: "Error(Contract, #4)",
      });
      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(
        sdk.revokeVC(issuerKeypair as never, Buffer.alloc(32, 4)),
      ).rejects.toThrow(
        "revokeVC simulation failed; no revocation state was changed: Error(Contract, #4)",
      );
      expect(mockSendTransaction).not.toHaveBeenCalled();
    });

    it("reports a confirmed contract failure as an atomic rollback", async () => {
      mockSendTransaction.mockResolvedValue({
        status: "PENDING",
        hash: "failed-revoke-hash",
      });
      mockGetTransaction.mockResolvedValue({
        status: "FAILED",
        errorResult: "Error(Contract, #4)",
      });
      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(
        sdk.revokeVC(issuerKeypair as never, Buffer.alloc(32, 4)),
      ).rejects.toThrow(
        "revokeVC failed; the atomic transaction rolled back both registry and identity-oracle changes",
      );
    });

    it("exports SDKError with its code", () => {
      const error = new SDKError("INVALID_VC_HASH", "invalid hash");

      expect(error).toBeInstanceOf(Error);
      expect(error.name).toBe("SDKError");
      expect(error.code).toBe("INVALID_VC_HASH");
    });

    it("retries transient submission failures before confirming revocation", async () => {
      jest.useFakeTimers();
      mockSendTransaction
        .mockRejectedValueOnce(
          Object.assign(new Error("RPC request failed with 503"), {
            response: { status: 503 },
          }),
        )
        .mockResolvedValueOnce({
          status: "PENDING",
          hash: "retried-revoke-hash",
        });

      const sdk = new StellarDIDCreditSDK({ ...mockConfig, maxRetries: 1 });
      const promise = sdk.revokeVC(
        issuerKeypair as never,
        Buffer.alloc(32, 4),
      );

      await jest.advanceTimersByTimeAsync(1000);

      await expect(promise).resolves.toBe("retried-revoke-hash");
      expect(mockSendTransaction).toHaveBeenCalledTimes(2);
      expect(mockGetTransaction).toHaveBeenCalledWith("retried-revoke-hash");
    });
  });

  describe("anchorDID", () => {
    it("throws a descriptive error when subjectKeypair public key does not match subject", async () => {
      const sdk = new StellarDIDCreditSDK(mockConfig);
      const wrongAddress = "GWRONGADDRESS12345678901234567890123456789012345678901234";

      await expect(
        sdk.anchorDID(subjectKeypair as never, "QmExampleCid", wrongAddress),
      ).rejects.toThrow("subjectKeypair public key does not match subject");
      expect(mockGetAccount).not.toHaveBeenCalled();
      expect(mockSendTransaction).not.toHaveBeenCalled();
    });

    it("submits successfully when subjectKeypair matches explicit subject address", async () => {
      const sdk = new StellarDIDCreditSDK(mockConfig);

      const result = await sdk.anchorDID(
        subjectKeypair as never,
        "QmExampleCid",
        subjectAddress,
      );

      expect(result).toBe("mock-tx-hash");
      expect(mockGetAccount).toHaveBeenCalledWith(subjectAddress);
      expect(mockSendTransaction).toHaveBeenCalled();
    });

    it("throws when simulation returns an explicit error", async () => {
      mockSimulateTransaction.mockResolvedValue({ error: "anchor_did rejected" });

      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(
        sdk.anchorDID(subjectKeypair as never, "QmExampleCid"),
      ).rejects.toThrow("Simulation failed: anchor_did rejected");
      expect(mockGetAccount).toHaveBeenCalledWith(subjectAddress);
      expect(mockSendTransaction).not.toHaveBeenCalled();
    });

    it("throws when transaction submission returns FAILED status", async () => {
      mockSendTransaction.mockResolvedValue({
        status: "FAILED",
        errorResult: "tx_bad_auth",
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(
        sdk.anchorDID(subjectKeypair as never, "QmExampleCid"),
      ).rejects.toThrow("Transaction submission failed: tx_bad_auth");
      expect(mockGetAccount).toHaveBeenCalledWith(subjectAddress);
      expect(mockSendTransaction).toHaveBeenCalled();
    });

    it("submits successfully and returns tx hash", async () => {
      const sdk = new StellarDIDCreditSDK(mockConfig);

      const result = await sdk.anchorDID(subjectKeypair as never, "QmExampleCid");

      expect(result).toBe("mock-tx-hash");
      expect(mockGetAccount).toHaveBeenCalledWith(subjectAddress);
      expect(mockSendTransaction).toHaveBeenCalled();
      expect(mockContractCalls[0]).toMatchObject({
        contractId: mockConfig.identityOracleId,
        method: "anchor_did",
      });
    });

    it("polls pending status until SUCCESS and then resolves with tx hash", async () => {
      jest.useFakeTimers();
      mockSendTransaction.mockResolvedValue({
        status: "PENDING",
        hash: "anchor-poll-success-hash",
      });
      mockGetTransaction
        .mockResolvedValueOnce({ status: "PENDING" })
        .mockResolvedValueOnce({ status: "PENDING" })
        .mockResolvedValueOnce({ status: "SUCCESS" });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const promise = sdk.anchorDID(subjectKeypair as never, "QmExampleCid");

      await Promise.resolve();
      await jest.advanceTimersByTimeAsync(5000);
      await jest.advanceTimersByTimeAsync(5000);

      await expect(promise).resolves.toBe("anchor-poll-success-hash");
      expect(mockGetTransaction).toHaveBeenCalledTimes(3);
    });

    it("throws SDKError with transaction hash and result XDR when confirmation fails", async () => {
      mockSendTransaction.mockResolvedValue({
        status: "PENDING",
        hash: "anchor-failed-hash",
      });
      mockGetTransaction.mockResolvedValue({
        status: "FAILED",
        resultXdr: "AAAAFAILEDXDR",
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(
        sdk.anchorDID(subjectKeypair as never, "QmExampleCid"),
      ).rejects.toMatchObject({
        code: "TRANSACTION_FAILED",
        transactionHash: "anchor-failed-hash",
        resultXdr: "AAAAFAILEDXDR",
      });
    });

    it("uses three default retries with exponential backoff", async () => {
      jest.useFakeTimers();
      mockSendTransaction
        .mockRejectedValueOnce(
          Object.assign(new Error("service unavailable"), { status: 503 }),
        )
        .mockRejectedValueOnce(
          Object.assign(new Error("service unavailable"), { status: 503 }),
        )
        .mockRejectedValueOnce(
          Object.assign(new Error("service unavailable"), { status: 503 }),
        )
        .mockResolvedValueOnce({
          status: "PENDING",
          hash: "retried-anchor-hash",
        });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const promise = sdk.anchorDID(subjectKeypair as never, "QmExampleCid");

      await jest.advanceTimersByTimeAsync(0);
      expect(mockSendTransaction).toHaveBeenCalledTimes(1);

      await jest.advanceTimersByTimeAsync(1000);
      expect(mockSendTransaction).toHaveBeenCalledTimes(2);

      await jest.advanceTimersByTimeAsync(2000);
      expect(mockSendTransaction).toHaveBeenCalledTimes(3);

      await jest.advanceTimersByTimeAsync(4000);

      await expect(promise).resolves.toBe("retried-anchor-hash");
      expect(mockSendTransaction).toHaveBeenCalledTimes(4);
      expect(mockGetTransaction).toHaveBeenCalledWith("retried-anchor-hash");
    });

    it("throws SDKError with TRANSACTION_TIMEOUT when confirmation never responds", async () => {
      jest.useFakeTimers();
      mockGetTransaction.mockReturnValue(new Promise(() => undefined));

      const sdk = new StellarDIDCreditSDK({ ...mockConfig, timeoutSeconds: 1 });
      const promise = sdk
        .anchorDID(subjectKeypair as never, "QmExampleCid")
        .catch((caught: unknown) => caught);

      await jest.advanceTimersByTimeAsync(1000);
      const error = await promise;

      expect(error).toBeInstanceOf(SDKError);
      expect(error).toMatchObject({ code: "TRANSACTION_TIMEOUT" });
      expect(mockGetTransaction).toHaveBeenCalledTimes(1);
    });

    it("uses the default 30s confirmation timeout", async () => {
      jest.useFakeTimers();
      mockGetTransaction.mockResolvedValue({ status: "PENDING" });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const promise = sdk
        .anchorDID(subjectKeypair as never, "QmExampleCid")
        .catch((caught: unknown) => caught);

      await Promise.resolve();
      await jest.advanceTimersByTimeAsync(30_000);
      const error = await promise;

      expect(error).toBeInstanceOf(SDKError);
      expect(error).toMatchObject({ code: "TRANSACTION_TIMEOUT" });
    });

    it("does not retry permanent submission errors", async () => {
      mockSendTransaction.mockRejectedValue(
        Object.assign(new Error("bad request"), { response: { status: 400 } }),
      );

      const sdk = new StellarDIDCreditSDK({ ...mockConfig, maxRetries: 3 });

      await expect(
        sdk.anchorDID(subjectKeypair as never, "QmExampleCid"),
      ).rejects.toThrow("bad request");
      expect(mockSendTransaction).toHaveBeenCalledTimes(1);
    });
  });

  describe("issueVC", () => {
    it("throws when simulation returns an explicit error", async () => {
      mockSimulateTransaction.mockResolvedValue({ error: "anchor_vc rejected" });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const vcHash = Buffer.alloc(32, 5);

      await expect(
        sdk.issueVC(issuerKeypair as never, subjectAddress, vcHash),
      ).rejects.toThrow("Simulation failed: anchor_vc rejected");
      expect(mockGetAccount).toHaveBeenCalledWith(issuerKeypair.publicKey());
      expect(mockSendTransaction).not.toHaveBeenCalled();
    });

    it("throws when transaction submission returns FAILED status", async () => {
      mockSendTransaction.mockResolvedValue({
        status: "FAILED",
        errorResult: "tx_bad_auth",
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const vcHash = Buffer.alloc(32, 6);

      await expect(
        sdk.issueVC(issuerKeypair as never, subjectAddress, vcHash),
      ).rejects.toThrow("Transaction submission failed: tx_bad_auth");
      expect(mockGetAccount).toHaveBeenCalledWith(issuerKeypair.publicKey());
      expect(mockSendTransaction).toHaveBeenCalled();
    });

    it("submits successfully and returns tx hash", async () => {
      const sdk = new StellarDIDCreditSDK(mockConfig);
      const vcHash = Buffer.alloc(32, 1);

      const result = await sdk.issueVC(issuerKeypair as never, subjectAddress, vcHash);

      expect(result).toBe("mock-tx-hash");
      expect(mockGetAccount).toHaveBeenCalledWith(issuerKeypair.publicKey());
      expect(mockSendTransaction).toHaveBeenCalled();
      expect(mockContractCalls[0]).toMatchObject({
        contractId: mockConfig.identityOracleId,
        method: "anchor_vc",
      });
    });

    it("retries TRY_AGAIN_LATER responses before confirming the transaction", async () => {
      jest.useFakeTimers();
      mockSendTransaction
        .mockResolvedValueOnce({
          status: "TRY_AGAIN_LATER",
          hash: "retry-later-hash",
        })
        .mockResolvedValueOnce({
          status: "PENDING",
          hash: "retried-vc-hash",
        });

      const sdk = new StellarDIDCreditSDK({ ...mockConfig, maxRetries: 1 });
      const promise = sdk.issueVC(
        issuerKeypair as never,
        subjectAddress,
        Buffer.alloc(32, 1),
      );

      await jest.advanceTimersByTimeAsync(1000);

      await expect(promise).resolves.toBe("retried-vc-hash");
      expect(mockSendTransaction).toHaveBeenCalledTimes(2);
      expect(mockGetTransaction).toHaveBeenCalledWith("retried-vc-hash");
    });

    it("rejects non-32-byte credential hashes", async () => {
      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(
        sdk.issueVC(issuerKeypair as never, subjectAddress, Buffer.alloc(31)),
      ).rejects.toThrow("vcHash must be exactly 32 bytes");
      expect(mockGetAccount).not.toHaveBeenCalled();
      expect(mockSendTransaction).not.toHaveBeenCalled();
    });
  });

  describe("verifyVC", () => {
    it("returns true for a valid hash", async () => {
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

    it("returns false for a revoked hash", async () => {
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

    it("throws on simulation error", async () => {
      mockSimulateTransaction.mockResolvedValue({ error: "verify error" });

      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(
        sdk.verifyVC(subjectAddress, Buffer.alloc(32)),
      ).rejects.toThrow("Simulation failed: verify error");
    });
  });

  describe("computeScore", () => {
    it("returns an updated ScoreRecord after successful compute + confirmation", async () => {
      mockGetAccount.mockResolvedValue({ sequenceNumber: () => "10" });
      mockSendTransaction.mockResolvedValue({
        status: "PENDING",
        hash: "tx-compute-hash",
      });
      mockGetTransaction.mockResolvedValue({ status: "SUCCESS" });
      mockSimulateTransaction
        .mockResolvedValueOnce({ result: { retval: { value: null } } })
        .mockResolvedValueOnce({
          result: {
            retval: {
              value: {
                score: 558,
                last_updated: 1_710_000_000,
                vc_count: 2,
                repayment_rate: 8500,
                tx_volume_30d: 2_000_000n,
                previous_score: null,
                computed_at_ledger: 1000000,
                stale: false,
              },
            },
          },
        });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.computeScore(
        { publicKey: () => subjectAddress } as unknown as Keypair,
        subjectAddress,
      );

      expect(result).toMatchObject({
        score: 558,
        lastUpdated: 1_710_000_000,
        vcCount: 2,
        repaymentRate: 8500,
        txVolume30d: 2_000_000n,
        stale: false,
      });
      expect(mockSendTransaction).toHaveBeenCalledTimes(1);
      expect(mockGetTransaction).toHaveBeenCalledWith("tx-compute-hash");
      expect(mockLastContractCall?.method).toBe("get_score");
    });

    it("polls getTransaction until SUCCESS before reading the stored score", async () => {
      jest.useFakeTimers();
      mockGetAccount.mockResolvedValue({ sequenceNumber: () => "123" });
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
                previous_score: null,
                computed_at_ledger: 1000000,
                stale: false,
              },
            },
          },
        });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const computePromise = sdk.computeScore(
        { publicKey: () => subjectAddress } as unknown as Keypair,
        subjectAddress,
      );

      await Promise.resolve();
      await jest.advanceTimersByTimeAsync(5000);

      await expect(computePromise).resolves.toMatchObject({
        score: 612,
        lastUpdated: 1_700_000_000,
        vcCount: 3,
        repaymentRate: 8000,
        txVolume30d: 1_000_000n,
        stale: false,
      });
      expect(mockGetTransaction).toHaveBeenCalledTimes(2);
      expect(mockLastContractCall?.method).toBe("get_score");
    });

    it("throws a descriptive error when the stored score is missing after confirmation", async () => {
      mockGetAccount.mockResolvedValue({ sequenceNumber: () => "123" });
      mockSendTransaction.mockResolvedValue({
        status: "PENDING",
        hash: "tx-hash-2",
      });
      mockGetTransaction.mockResolvedValue({ status: "SUCCESS" });
      mockSimulateTransaction
        .mockResolvedValueOnce({ result: {} })
        .mockResolvedValueOnce({
          result: {
            retval: xdr.ScVal.scvVoid(),
          },
        });

      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(
        sdk.computeScore(
          { publicKey: () => subjectAddress } as unknown as Keypair,
          subjectAddress,
        ),
      ).rejects.toThrow(
        `computeScore transaction succeeded and was confirmed, but fetching the stored score for ${subjectAddress} failed: No score computed for address: ${subjectAddress}`,
      );
      expect(mockGetTransaction).toHaveBeenCalledTimes(1);
    });

    it("throws a descriptive error when the submitted transaction FAILS", async () => {
      mockGetAccount.mockResolvedValue({ sequenceNumber: () => "123" });
      mockSendTransaction.mockResolvedValue({
        status: "PENDING",
        hash: "tx-hash-3",
      });
      mockGetTransaction.mockResolvedValue({
        status: "FAILED",
        resultXdr: "AAAAFAILXDR",
      });
      mockSimulateTransaction.mockResolvedValue({
        result: { retval: { value: null } },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(
        sdk.computeScore(
          { publicKey: () => subjectAddress } as unknown as Keypair,
          subjectAddress,
        ),
      ).rejects.toMatchObject({
        code: "TRANSACTION_FAILED",
        transactionHash: "tx-hash-3",
        resultXdr: "AAAAFAILXDR",
      });
      expect(mockGetTransaction).toHaveBeenCalledTimes(1);
    });

    it("throws when computeScore simulation returns an explicit error", async () => {
      mockGetAccount.mockResolvedValue({ sequenceNumber: () => "123" });
      mockSimulateTransaction.mockResolvedValue({ error: "compute_score rejected" });

      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(
        sdk.computeScore(
          { publicKey: () => subjectAddress } as unknown as Keypair,
          subjectAddress,
        ),
      ).rejects.toThrow("Simulation failed: compute_score rejected");
      expect(mockSendTransaction).not.toHaveBeenCalled();
    });

    it("retries transient submission failures before reading the confirmed score", async () => {
      jest.useFakeTimers();
      mockSendTransaction
        .mockRejectedValueOnce(
          Object.assign(new Error("request timed out"), { code: "ETIMEDOUT" }),
        )
        .mockResolvedValueOnce({
          status: "PENDING",
          hash: "retried-compute-hash",
        });
      mockSimulateTransaction
        .mockResolvedValueOnce({ result: { retval: { value: null } } })
        .mockResolvedValueOnce({
          result: {
            retval: {
              value: {
                score: 640,
                last_updated: 1_710_000_000,
                vc_count: 4,
                repayment_rate: 9000,
                tx_volume_30d: 3_000_000n,
                previous_score: 620,
                computed_at_ledger: 1000001,
                stale: false,
              },
            },
          },
        });

      const sdk = new StellarDIDCreditSDK({ ...mockConfig, maxRetries: 1 });
      const promise = sdk.computeScore(
        { publicKey: () => subjectAddress } as unknown as Keypair,
        subjectAddress,
      );

      await jest.advanceTimersByTimeAsync(1000);

      await expect(promise).resolves.toMatchObject({ score: 640 });
      expect(mockSendTransaction).toHaveBeenCalledTimes(2);
      expect(mockGetTransaction).toHaveBeenCalledWith("retried-compute-hash");
    });
  });

  describe("getVCCount", () => {
    it("returns active VC count", async () => {
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

    it("returns zero when no active VCs", async () => {
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

    it("throws on simulation error", async () => {
      mockSimulateTransaction.mockResolvedValue({ error: "rpc error" });

      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(sdk.getVCCount(subjectAddress)).rejects.toThrow(
        "Simulation failed: rpc error",
      );
    });
  });

  describe("getVCs", () => {
    it("returns parsed VC records from get_vc_details", async () => {
      const vcHash = Buffer.alloc(32, 7);
      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: {
            value: [
              {
                vc_hash: vcHash,
                issuer: issuerKeypair.publicKey(),
                anchored_at: 1_700_000_000,
                revoked: false,
              },
              {
                vc_hash: Buffer.alloc(32, 8),
                issuer: issuerKeypair.publicKey(),
                anchored_at: 1_700_000_001,
                revoked: true,
              },
            ],
          },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.getVCs(subjectAddress);

      expect(result).toHaveLength(2);
      expect(result[0]).toEqual({
        vcHash,
        issuer: issuerKeypair.publicKey(),
        anchoredAt: 1_700_000_000,
        revoked: false,
      });
      expect(result[1]?.revoked).toBe(true);
      expect(mockLastContractCall?.method).toBe("get_vc_details");
      expect(mockLastContractCall?.args).toHaveLength(1);
    });

    it("returns an empty array when no VCs are anchored", async () => {
      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: { value: [] },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.getVCs(subjectAddress);

      expect(result).toEqual([]);
    });

    it("throws on simulation error", async () => {
      mockSimulateTransaction.mockResolvedValue({ error: "rpc error" });

      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(sdk.getVCs(subjectAddress)).rejects.toThrow(
        "Simulation failed: rpc error",
      );
    });
  });

  describe("getCredentialType", () => {
    it("returns the credential type label for a valid hash", async () => {
      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: { value: "kyc" },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.getCredentialType(subjectAddress, Buffer.alloc(32, 3));

      expect(result).toBe("kyc");
      expect(mockLastContractCall?.method).toBe("get_vc_credential_type");
      expect(mockLastContractCall?.args).toHaveLength(2);
    });

    it("defaults to generic when no type is stored", async () => {
      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: { value: "generic" },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.getCredentialType(subjectAddress, Buffer.alloc(32, 4));

      expect(result).toBe("generic");
    });

    it("rejects non-32-byte credential hashes", async () => {
      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(
        sdk.getCredentialType(subjectAddress, Buffer.alloc(31)),
      ).rejects.toThrow("vcHash must be exactly 32 bytes");
      expect(mockSimulateTransaction).not.toHaveBeenCalled();
    });

    it("throws on simulation error", async () => {
      mockSimulateTransaction.mockResolvedValue({ error: "rpc error" });

      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(
        sdk.getCredentialType(subjectAddress, Buffer.alloc(32)),
      ).rejects.toThrow("Simulation failed: rpc error");
    });
  });

  describe("isVerified", () => {
    it("returns true when subject has active VCs", async () => {
      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: { value: true },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.isVerified(subjectAddress);

      expect(result).toBe(true);
      expect(mockLastContractCall?.method).toBe("is_verified");
      expect(mockLastContractCall?.args).toHaveLength(1);
    });

    it("returns false when subject has no active VCs", async () => {
      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: { value: false },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.isVerified(subjectAddress);

      expect(result).toBe(false);
      expect(mockLastContractCall?.method).toBe("is_verified");
    });

    it("throws on simulation error", async () => {
      mockSimulateTransaction.mockResolvedValue({ error: "rpc error" });

      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(sdk.isVerified(subjectAddress)).rejects.toThrow(
        "Simulation failed: rpc error",
      );
    });
  });

  describe("getWeights", () => {
    it("returns scoring weights from the credit-oracle", async () => {
      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: {
            value: {
              vc_weight: 40,
              tx_weight: 30,
              repayment_weight: 30,
            },
          },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.getWeights();

      expect(result).toEqual({
        vcWeight: 40,
        txWeight: 30,
        repaymentWeight: 30,
      });
      expect(mockLastContractCall?.method).toBe("get_scoring_weights");
    });

    it("throws on simulation error", async () => {
      mockSimulateTransaction.mockResolvedValue({ error: "rpc error" });

      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(sdk.getWeights()).rejects.toThrow(
        "Simulation failed: rpc error",
      );
    });
  });

  describe("getRegisteredIssuers", () => {
    it("returns list of registered issuer addresses", async () => {
      const issuers = ["GISSUER1", "GISSUER2"];
      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: { value: issuers },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.getRegisteredIssuers();

      expect(result).toEqual(issuers);
      expect(mockLastContractCall?.method).toBe("list_issuers");
    });

    it("returns empty array when no issuers registered", async () => {
      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: { value: [] },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.getRegisteredIssuers();

      expect(result).toEqual([]);
    });

    it("throws on simulation error", async () => {
      mockSimulateTransaction.mockResolvedValue({ error: "rpc error" });

      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(sdk.getRegisteredIssuers()).rejects.toThrow(
        "Simulation failed: rpc error",
      );
    });
  });

  describe("getScore", () => {
    it("returns null for a fresh subject with no computed score", async () => {
      const sdk = new StellarDIDCreditSDK(mockConfig);

      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: xdr.ScVal.scvVoid(),
        },
      });

      const result = await sdk.getScore(subjectAddress);
      expect(result).toBeNull();
    });

    it("returns null when simulation reports score not computed", async () => {
      const sdk = new StellarDIDCreditSDK(mockConfig);

      mockSimulateTransaction.mockResolvedValue({
        error: "score not computed",
      });

      const result = await sdk.getScore(subjectAddress);
      expect(result).toBeNull();
    });

    it("returns parsed ScoreRecord from simulation result", async () => {
      const sdk = new StellarDIDCreditSDK(mockConfig);

      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: {
            value: {
              score: 750,
              last_updated: 1_700_000_000,
              vc_count: 5,
              repayment_rate: 9500,
              tx_volume_30d: 10_000_000n,
              previous_score: null,
              computed_at_ledger: 1234567,
              stale: false,
            },
          },
        },
      });

      const result = await sdk.getScore(subjectAddress);
      expect(result).toMatchObject({
        score: 750,
        lastUpdated: 1_700_000_000,
        vcCount: 5,
        txVolume30d: 10_000_000n,
        stale: false,
      });
    });

    it("throws on unknown simulation error", async () => {
      const sdk = new StellarDIDCreditSDK(mockConfig);

      mockSimulateTransaction.mockResolvedValue({
        error: "some other error",
      });

      await expect(sdk.getScore(subjectAddress)).rejects.toThrow(
        "Simulation failed: some other error",
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

  describe("ProtocolConfig — timeoutSeconds, maxRetries, baseFee", () => {
    it("applies custom timeoutSeconds to TransactionBuilder", async () => {
      const scoreRetval = {
        value: {
          score: 300,
          last_updated: 0,
          vc_count: 0,
          repayment_rate: 0,
          tx_volume_30d: 0n,
          previous_score: null,
          computed_at_ledger: 0,
          stale: false,
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

    it("retries and succeeds after n transient failures", async () => {
      jest.useFakeTimers();
      const TRANSIENT = {};
      const SUCCESS = {
        result: {
          retval: {
            value: {
              score: 500,
              last_updated: 1_700_000_000,
              vc_count: 1,
              repayment_rate: 7000,
              tx_volume_30d: 500_000n,
              previous_score: null,
              computed_at_ledger: 0,
              stale: false,
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

      await jest.advanceTimersByTimeAsync(2000);

      const result = await promise;
      expect(result?.score).toBe(500);
      expect(mockSimulateTransaction).toHaveBeenCalledTimes(3);
    });

    it("throws after maxRetries exhausted with transient responses", async () => {
      const TRANSIENT = {};
      mockSimulateTransaction.mockResolvedValue(TRANSIENT);

      const sdk = new StellarDIDCreditSDK({ ...mockConfig, maxRetries: 0 });
      await expect(sdk.getScore(subjectAddress)).rejects.toThrow(
        "Simulation returned unexpected response",
      );
      expect(mockSimulateTransaction).toHaveBeenCalledTimes(1);
    });

    it("forwards custom baseFee to TransactionBuilder", async () => {
      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: {
            value: {
              score: 300,
              last_updated: 0,
              vc_count: 0,
              repayment_rate: 0,
              tx_volume_30d: 0n,
              previous_score: null,
              computed_at_ledger: 0,
              stale: false,
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
    expect(weights.vcWeight + weights.txWeight + weights.repaymentWeight).toBe(100);
  });

  it("exports RepaymentRecord with counters and total repayment volume", () => {
    const record: RepaymentRecord = {
      onTimeCount: 8,
      totalCount: 10,
      totalRepaid: 50_000_000_000n,
    };

    expect(typeof record.onTimeCount).toBe("number");
    expect(typeof record.totalCount).toBe("number");
    expect(typeof record.totalRepaid).toBe("bigint");
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
      previousScore: null,
      computedAtLedger: 1234567,
      stale: false,
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
    const _txStats: TxStats = { volume30d: 0n, txCount30d: 0, avgCounterparties: 0 };
    const _weights: ScoringWeights = { vcWeight: 40, txWeight: 30, repaymentWeight: 30 };
    const _repayment: RepaymentRecord = { onTimeCount: 0, totalCount: 0, totalRepaid: 0n };
    const _vc: VCRecord = { vcHash: Buffer.alloc(32), issuer: "G", anchoredAt: 0, revoked: false };
    const _score: ScoreRecord = { score: 300, lastUpdated: 0, vcCount: 0, repaymentRate: 0, txVolume30d: 0n, previousScore: null, computedAtLedger: 0, stale: false };
    const _config: ProtocolConfig = { identityOracleId: "", creditOracleId: "", revocationRegistryId: "", networkPassphrase: "", rpcUrl: "", simAccount: "" };
    const _govProp: GovernanceProposal = { id: 1n, proposer: "G", proposedWeights: _weights, votesFor: 0n, votesAgainst: 0n, expiryLedger: 0, executionDelayLedgers: 0, executed: false, cancelled: false, quorumRequired: 100n };
    expect(_txStats).toBeDefined();
    expect(_weights).toBeDefined();
    expect(_repayment).toBeDefined();
    expect(_vc).toBeDefined();
    expect(_score).toBeDefined();
    expect(_config).toBeDefined();
    expect(_govProp).toBeDefined();
  });
});

describe("listProposals", () => {
  it("throws error if governanceId is not configured", async () => {
    const sdk = new StellarDIDCreditSDK({
      ...mockConfig,
      governanceId: undefined,
    });
    await expect(sdk.listProposals(1, 10)).rejects.toThrow(
      "governanceId is not configured in ProtocolConfig",
    );
  });

  it("calls list_proposals contract method and parses result correctly", async () => {
    const configWithGov = {
      ...mockConfig,
      governanceId: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAGOV1",
    };
    const sdk = new StellarDIDCreditSDK(configWithGov);

    const mockProposalsRaw = [
      {
        id: 1n,
        proposer: "GBUQWP3BOUZX34ULNQG23RQ6F4YUSXHTQSXE7XDZT4A65XJLQRGEZSM",
        proposed_weights: {
          vc_weight: 40,
          tx_weight: 30,
          repayment_weight: 30,
        },
        votes_for: 100n,
        votes_against: 0n,
        expiry_ledger: 1000,
        execution_delay_ledgers: 100,
        executed: false,
        cancelled: false,
        quorum_required: 100n,
      },
    ];

    mockSimulateTransaction.mockResolvedValueOnce({
      result: { retval: { value: mockProposalsRaw } },
    });

    const proposals = await sdk.listProposals(1, 10, true);

    expect(proposals).toHaveLength(1);
    expect(proposals[0]).toEqual({
      id: 1n,
      proposer: "GBUQWP3BOUZX34ULNQG23RQ6F4YUSXHTQSXE7XDZT4A65XJLQRGEZSM",
      proposedWeights: {
        vcWeight: 40,
        txWeight: 30,
        repaymentWeight: 30,
      },
      votesFor: 100n,
      votesAgainst: 0n,
      expiryLedger: 1000,
      executionDelayLedgers: 100,
      executed: false,
      cancelled: false,
      quorumRequired: 100n,
    });
  });
});
