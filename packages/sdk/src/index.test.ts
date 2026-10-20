import {
  StellarDIDCreditSDK,
  GovernanceClient,
  SDKError,
  ScoreNotComputedError,
  ContractError,
  IdentityOracleError,
  CreditOracleError,
  RevocationRegistryError,
  GovernanceError,
  parseContractErrorCode,
  throwContractError,
  MIN_SCORE,
  MAX_SCORE,
  parseScoreRecord,
  ScoreRecord,
  ProtocolConfig,
  TxStats,
  ScoringWeights,
  RepaymentRecord,
  VCRecord,
  GovernanceProposal,
  BatchResult,
  DisputeEvent,
  ScoreEvent,
} from "./index";
import { xdr, Keypair } from "@stellar/stellar-sdk";

const mockSimulateTransaction = jest.fn();
const mockGetAccount = jest.fn();
const mockSendTransaction = jest.fn();
const mockGetTransaction = jest.fn();
const mockGetEvents = jest.fn();
const mockGetLatestLedger = jest.fn();
const mockGetLedgerEntries = jest.fn();
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
    LedgerKey: {
      contractData: (data: unknown) => ({ data }),
    },
    LedgerKeyContractData: jest.fn().mockImplementation((data: unknown) => data),
    ContractDataDurability: {
      persistent: () => "persistent",
      temporary: () => "temporary",
    },
    ScVal: {
      scvVoid: () => ({ switch: () => "scvVoid" }),
      scvSymbol: (symbol: string) => ({
        toXDR: () => `symbol:${symbol}`,
      }),
      scvVec: (elements: unknown[]) => elements,
      scvLedgerKeyContractInstance: () => ({ value: "ContractInstance" }),
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
    toScAddress: () => ({ address }),
  })),
  Contract: jest.fn().mockImplementation((contractId: string) => ({
    contractId,
    call: (method: string, ...args: unknown[]) => {
      const call = { contractId, method, args };
      mockLastContractCall = call;
      mockContractCalls.push(call);
      return { method, args };
    },
    address: () => ({
      toScAddress: () => ({ address: contractId }),
    }),
  })),
  TransactionBuilder: jest.fn().mockImplementation(() => ({
    addOperation: jest.fn().mockReturnThis(),
    setTimeout: jest.fn().mockReturnThis(),
    build: jest.fn().mockReturnValue({ operations: [] }),
  })),
  nativeToScVal: (value: unknown, options?: { type?: unknown }) => ({
    value,
    type: options?.type,
    address: () => ({
      toXDR: () => Buffer.from(String(value)),
    }),
  }),
  scValToNative: (scVal: { value?: unknown }) => scVal?.value,
  hash: (data: Uint8Array | Buffer) => Buffer.from(data).subarray(0, 32),
  SorobanRpc: {
    Server: jest.fn().mockImplementation(() => ({
      getAccount: mockGetAccount,
      sendTransaction: mockSendTransaction,
      simulateTransaction: mockSimulateTransaction,
      getTransaction: mockGetTransaction,
      getEvents: mockGetEvents,
      getLatestLedger: mockGetLatestLedger,
      getLedgerEntries: mockGetLedgerEntries,
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
  governanceId: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
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
    (
      jest.requireMock("@stellar/stellar-sdk").SorobanRpc.Server as jest.Mock
    ).mockClear();
  });

  describe("governance", () => {
    const adminAddress = "GADMINAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    const adminKeypair = { publicKey: () => adminAddress };

    describe("admin methods", () => {
      beforeEach(() => {
        mockGetTransaction.mockResolvedValue({ status: "SUCCESS" });
      });

      it("registers a voter", async () => {
        mockSimulateTransaction.mockResolvedValue({
          result: { retval: { value: undefined } },
        });

        const sdk = new StellarDIDCreditSDK(mockConfig);
        const result = await sdk.governance.registerVoter(
          adminKeypair as never,
          subjectAddress,
          100n,
        );

        expect(result).toBe("mock-tx-hash");
        expect(mockGetAccount).toHaveBeenCalledWith(adminAddress);
        expect(mockGetTransaction).toHaveBeenCalledWith("mock-tx-hash");
        expect(mockContractCalls[0]).toMatchObject({
          contractId: mockConfig.governanceId,
          method: "register_voter",
        });
        expect(mockContractCalls[0]?.args).toHaveLength(3);
        expect(mockContractCalls[0]?.args[2]).toMatchObject({
          value: 100n,
          type: "i128",
        });
      });

      it("updates a voter weight", async () => {
        mockSimulateTransaction.mockResolvedValue({
          result: { retval: { value: undefined } },
        });

        const sdk = new StellarDIDCreditSDK(mockConfig);
        const result = await sdk.governance.updateVoterWeight(
          adminKeypair as never,
          subjectAddress,
          200n,
        );

        expect(result).toBe("mock-tx-hash");
        expect(mockContractCalls[0]).toMatchObject({
          contractId: mockConfig.governanceId,
          method: "update_voter_weight",
        });
        expect(mockContractCalls[0]?.args[2]).toMatchObject({
          value: 200n,
          type: "i128",
        });
      });

      it("sets the quorum", async () => {
        mockSimulateTransaction.mockResolvedValue({
          result: { retval: { value: undefined } },
        });

        const sdk = new StellarDIDCreditSDK(mockConfig);
        const result = await sdk.governance.setQuorum(
          adminKeypair as never,
          1000n,
        );

        expect(result).toBe("mock-tx-hash");
        expect(mockContractCalls[0]).toMatchObject({
          contractId: mockConfig.governanceId,
          method: "set_quorum",
        });
        expect(mockContractCalls[0]?.args[1]).toMatchObject({
          value: 1000n,
          type: "i128",
        });
      });
    });

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

    it("decodes a cancelled proposal with its proposer address", async () => {
      const proposerAddress =
        "GBUQWP3BOUZX34ULNQG23RQ6F4YUSXHTQSXE7XDZT4A65XJLQRGEZSM";
      mockSimulateTransaction.mockResolvedValueOnce({
        result: {
          retval: {
            value: {
              id: 12n,
              proposer: proposerAddress,
              proposed_weights: {
                vc_weight: 50,
                tx_weight: 25,
                repayment_weight: 25,
              },
              votes_for: 80n,
              votes_against: 40n,
              expiry_ledger: 200,
              execution_delay_ledgers: 50,
              executed: false,
              cancelled: true,
              quorum_required: 100n,
            },
          },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const proposal = await sdk.governance.getProposal(12n);

      expect(proposal).toEqual<GovernanceProposal>({
        id: 12n,
        proposer: proposerAddress,
        proposedWeights: governanceWeights,
        votesFor: 80n,
        votesAgainst: 40n,
        expiryLedger: 200,
        executionDelayLedgers: 50,
        executed: false,
        cancelled: true,
        quorumRequired: 100n,
      });
      expect(proposal?.cancelled).toBe(true);
      expect(proposal?.proposer).toBe(proposerAddress);
    });

    it("executes and applies weights through signed governance calls", async () => {
      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(
        sdk.governance.execute(subjectKeypair as never, 7n),
      ).resolves.toBe("mock-tx-hash");
      await expect(
        sdk.governance.applyWeights(subjectKeypair as never),
      ).resolves.toBe("mock-tx-hash");

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

    it("returns all proposals when listProposals() is called without args", async () => {
      mockContractCalls.length = 0;
      mockGetLedgerEntries.mockResolvedValueOnce({
        entries: [
          {
            val: {
              contractData: {
                val: {
                  storage: [
                    {
                      key: { value: "NextProposalId" },
                      val: { value: 4n },
                    },
                  ],
                },
              },
            },
          },
        ],
      });

      const makeMockProposal = (id: bigint) => ({
        result: {
          retval: {
            value: {
              id,
              proposer: subjectAddress,
              proposed_weights: {
                vc_weight: 40,
                tx_weight: 30,
                repayment_weight: 30,
              },
              votes_for: 10n,
              votes_against: 0n,
              expiry_ledger: 100,
              execution_delay_ledgers: 0,
              executed: false,
              cancelled: false,
              quorum_required: 10n,
            },
          },
        },
      });

      mockSimulateTransaction
        .mockResolvedValueOnce(makeMockProposal(1n))
        .mockResolvedValueOnce(makeMockProposal(2n))
        .mockResolvedValueOnce(makeMockProposal(3n));

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const proposals = await sdk.governance.listProposals();

      expect(proposals).toHaveLength(3);
      expect(proposals.map((p) => p.id)).toEqual([1n, 2n, 3n]);
      expect(mockContractCalls.map((call) => call.method)).toEqual([
        "get_proposal",
        "get_proposal",
        "get_proposal",
      ]);
    });

    it("returns proposals 2-6 when listProposals(2n, 5) is called", async () => {
      mockContractCalls.length = 0;
      mockGetLedgerEntries.mockResolvedValueOnce({
        entries: [
          {
            val: {
              contractData: {
                val: {
                  storage: [
                    {
                      key: { value: "NextProposalId" },
                      val: { value: 10n },
                    },
                  ],
                },
              },
            },
          },
        ],
      });

      const makeMockProposal = (id: bigint) => ({
        result: {
          retval: {
            value: {
              id,
              proposer: subjectAddress,
              proposed_weights: {
                vc_weight: 40,
                tx_weight: 30,
                repayment_weight: 30,
              },
              votes_for: 10n,
              votes_against: 0n,
              expiry_ledger: 100,
              execution_delay_ledgers: 0,
              executed: false,
              cancelled: false,
              quorum_required: 10n,
            },
          },
        },
      });

      mockSimulateTransaction
        .mockResolvedValueOnce(makeMockProposal(2n))
        .mockResolvedValueOnce(makeMockProposal(3n))
        .mockResolvedValueOnce(makeMockProposal(4n))
        .mockResolvedValueOnce(makeMockProposal(5n))
        .mockResolvedValueOnce(makeMockProposal(6n));

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const proposals = await sdk.governance.listProposals(2n, 5);

      expect(proposals).toHaveLength(5);
      expect(proposals.map((p) => p.id)).toEqual([2n, 3n, 4n, 5n, 6n]);
      expect(mockContractCalls.map((call) => call.method)).toEqual([
        "get_proposal",
        "get_proposal",
        "get_proposal",
        "get_proposal",
        "get_proposal",
      ]);
    });

    it("handles limit 0 and validates invalid limit in listProposals", async () => {
      const sdk = new StellarDIDCreditSDK(mockConfig);
      const emptyProposals = await sdk.governance.listProposals(1n, 0);
      expect(emptyProposals).toEqual([]);

      await expect(sdk.governance.listProposals(1n, -1)).rejects.toThrow(
        "limit must be a non-negative integer",
      );
      await expect(sdk.governance.listProposals(1n, 1.5)).rejects.toThrow(
        "limit must be a non-negative integer",
      );
    });

    it("handles getNextProposalId fallback gracefully", async () => {
      mockGetLedgerEntries.mockRejectedValueOnce(new Error("RPC failure"));
      const sdk = new StellarDIDCreditSDK(mockConfig);
      const nextId = await sdk.governance.getNextProposalId();
      expect(nextId).toBe(1n);
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

      mockSimulateTransaction
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
                computed_at_ledger: 1234567,
                stale: false,
              },
            },
          },
        })
        .mockResolvedValueOnce({
          result: { retval: { value: "ipfs://QmTestDID123" } },
        })
        .mockResolvedValueOnce({
          result: { retval: { value: 3 } },
        })
        .mockResolvedValueOnce({
          result: {
            retval: {
              value: {
                vc_weight: 40,
                tx_weight: 30,
                repayment_weight: 30,
              },
            },
          },
        })
        .mockResolvedValueOnce({
          result: { retval: { value: true } },
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

    describe("subscribeToDisputeEvents", () => {
      const otherSubject =
        "GOTHERSUBJECTAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

      const disputeEvent = (
        eventType: string,
        eventSubject: string,
        inputKey: string,
      ) => ({
        ledger: 100,
        topic: [{ value: eventType }],
        value: { value: [eventSubject, inputKey] },
      });

      it.each<DisputeEvent>([
        {
          eventType: "DsptFild",
          subject: subjectAddress,
          inputKey: "tx_stats",
          status: "Pending",
        },
        {
          eventType: "DsptRslv",
          subject: subjectAddress,
          inputKey: "repayment",
          status: "Resolved",
        },
        {
          eventType: "DsptRjct",
          subject: subjectAddress,
          inputKey: "vc_count",
          status: "Rejected",
        },
      ])("delivers $eventType with status $status", async (expected) => {
        mockGetEvents.mockResolvedValueOnce({
          latestLedger: 100,
          events: [
            disputeEvent(expected.eventType, subjectAddress, expected.inputKey),
          ],
        });

        const sdk = new StellarDIDCreditSDK({
          ...mockConfig,
          pollIntervalMs: 10,
        });
        const received: DisputeEvent[] = [];
        const unsubscribe = sdk.subscribeToDisputeEvents(subjectAddress, (event) =>
          received.push(event),
        );

        await jest.advanceTimersByTimeAsync(0);

        expect(received).toEqual([expected]);

        unsubscribe();
      });

      it("queries all three dispute topics on the credit-oracle in one poll", async () => {
        mockGetEvents.mockResolvedValueOnce({ latestLedger: 100, events: [] });

        const creditOracleId =
          "CCREDITORACLEAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        const sdk = new StellarDIDCreditSDK({
          ...mockConfig,
          creditOracleId,
          pollIntervalMs: 10,
        });
        const unsubscribe = sdk.subscribeToDisputeEvents(
          subjectAddress,
          jest.fn(),
        );

        await jest.advanceTimersByTimeAsync(0);

        expect(mockGetEvents).toHaveBeenCalledTimes(1);
        expect(mockGetEvents).toHaveBeenCalledWith(
          expect.objectContaining({
            startLedger: 100,
            filters: [
              {
                type: "contract",
                contractIds: [creditOracleId],
                topics: [
                  ["symbol:DsptFild"],
                  ["symbol:DsptRslv"],
                  ["symbol:DsptRjct"],
                ],
              },
            ],
          }),
        );

        unsubscribe();
      });

      it.each([
        ["DsptFild", "Pending"],
        ["DsptRslv", "Resolved"],
        ["DsptRjct", "Rejected"],
      ])("filters %s disputes by subject", async (eventType, status) => {
        mockGetEvents.mockResolvedValueOnce({
          latestLedger: 100,
          events: [
            disputeEvent(eventType, otherSubject, "tx_stats"),
            disputeEvent(eventType, subjectAddress, "vc_count"),
            disputeEvent(eventType, otherSubject, "repayment"),
          ],
        });

        const sdk = new StellarDIDCreditSDK({
          ...mockConfig,
          pollIntervalMs: 10,
        });
        const callback = jest.fn();
        const unsubscribe = sdk.subscribeToDisputeEvents(subjectAddress, callback);

        await jest.advanceTimersByTimeAsync(0);

        expect(callback).toHaveBeenCalledTimes(1);
        expect(callback).toHaveBeenCalledWith({
          eventType,
          subject: subjectAddress,
          inputKey: "vc_count",
          status,
        });

        unsubscribe();
      });

      it.each(["Score", "VCAnch", "Revoked", "Unknown"])(
        "ignores the unrelated %s topic without skipping valid disputes",
        async (topic) => {
          mockGetEvents.mockResolvedValueOnce({
            latestLedger: 100,
            events: [
              disputeEvent(topic, subjectAddress, "tx_stats"),
              disputeEvent("DsptRslv", subjectAddress, "repayment"),
            ],
          });
          const sdk = new StellarDIDCreditSDK(mockConfig);
          const callback = jest.fn();
          const unsubscribe = sdk.subscribeToDisputeEvents(subjectAddress, callback);

          await jest.advanceTimersByTimeAsync(0);

          expect(callback).toHaveBeenCalledTimes(1);
          expect(callback).toHaveBeenCalledWith({
            eventType: "DsptRslv",
            subject: subjectAddress,
            inputKey: "repayment",
            status: "Resolved",
          });
          unsubscribe();
        },
      );

      it("waits through empty polls before delivering a dispute", async () => {
        mockGetEvents
          .mockResolvedValueOnce({ latestLedger: 100, events: [] })
          .mockResolvedValueOnce({
            latestLedger: 101,
            events: [
              {
                ...disputeEvent("DsptFild", subjectAddress, "tx_stats"),
                ledger: 101,
              },
            ],
          });
        const sdk = new StellarDIDCreditSDK({ ...mockConfig, pollIntervalMs: 10 });
        const callback = jest.fn();
        const unsubscribe = sdk.subscribeToDisputeEvents(subjectAddress, callback);

        await jest.advanceTimersByTimeAsync(0);
        expect(callback).not.toHaveBeenCalled();
        expect(mockGetEvents).toHaveBeenCalledTimes(1);

        await jest.advanceTimersByTimeAsync(10);
        expect(mockGetEvents).toHaveBeenCalledTimes(2);
        expect(callback).toHaveBeenCalledTimes(1);
        expect(callback).toHaveBeenCalledWith({
          eventType: "DsptFild",
          subject: subjectAddress,
          inputKey: "tx_stats",
          status: "Pending",
        });
        unsubscribe();
      });

      it("delivers a mixed dispute batch in order for the requested subject", async () => {
        mockGetEvents.mockResolvedValueOnce({
          latestLedger: 100,
          events: [
            disputeEvent("DsptFild", otherSubject, "tx_stats"),
            disputeEvent("DsptFild", subjectAddress, "tx_stats"),
            disputeEvent("DsptRslv", otherSubject, "repayment"),
            disputeEvent("DsptRslv", subjectAddress, "tx_stats"),
            disputeEvent("DsptRjct", otherSubject, "vc_count"),
            disputeEvent("DsptRjct", subjectAddress, "repayment"),
          ],
        });
        const sdk = new StellarDIDCreditSDK(mockConfig);
        const received: DisputeEvent[] = [];
        const unsubscribe = sdk.subscribeToDisputeEvents(subjectAddress, (event) =>
          received.push(event),
        );

        await jest.advanceTimersByTimeAsync(0);

        expect(received).toEqual([
          {
            eventType: "DsptFild",
            subject: subjectAddress,
            inputKey: "tx_stats",
            status: "Pending",
          },
          {
            eventType: "DsptRslv",
            subject: subjectAddress,
            inputKey: "tx_stats",
            status: "Resolved",
          },
          {
            eventType: "DsptRjct",
            subject: subjectAddress,
            inputKey: "repayment",
            status: "Rejected",
          },
        ]);
        unsubscribe();
      });

      it.each([
        ["DsptRslv", "Resolved"],
        ["DsptRjct", "Rejected"],
      ])("delivers filing followed by %s across polls", async (eventType, status) => {
        mockGetEvents
          .mockResolvedValueOnce({
            latestLedger: 100,
            events: [disputeEvent("DsptFild", subjectAddress, "tx_stats")],
          })
          .mockResolvedValueOnce({
            latestLedger: 101,
            events: [
              {
                ...disputeEvent(eventType, subjectAddress, "tx_stats"),
                ledger: 101,
              },
            ],
          });
        const sdk = new StellarDIDCreditSDK({ ...mockConfig, pollIntervalMs: 10 });
        const callback = jest.fn();
        const unsubscribe = sdk.subscribeToDisputeEvents(subjectAddress, callback);

        await jest.advanceTimersByTimeAsync(0);
        expect(callback).toHaveBeenCalledTimes(1);
        expect(callback).toHaveBeenNthCalledWith(1, {
          eventType: "DsptFild",
          subject: subjectAddress,
          inputKey: "tx_stats",
          status: "Pending",
        });

        await jest.advanceTimersByTimeAsync(10);
        expect(callback).toHaveBeenCalledTimes(2);
        expect(callback).toHaveBeenNthCalledWith(2, {
          eventType,
          subject: subjectAddress,
          inputKey: "tx_stats",
          status,
        });
        expect(mockGetEvents).toHaveBeenNthCalledWith(
          2,
          expect.objectContaining({ startLedger: 101 }),
        );
        unsubscribe();
      });

      it("can unsubscribe while the initial ledger request is pending", async () => {
        let resolveLedger!: (ledger: { sequence: number }) => void;
        mockGetLatestLedger.mockReturnValueOnce(
          new Promise<{ sequence: number }>((resolve) => {
            resolveLedger = resolve;
          }),
        );
        const sdk = new StellarDIDCreditSDK({ ...mockConfig, pollIntervalMs: 10 });
        const callback = jest.fn();
        const unsubscribe = sdk.subscribeToDisputeEvents(subjectAddress, callback);

        expect(mockGetLatestLedger).toHaveBeenCalledTimes(1);
        unsubscribe();
        resolveLedger({ sequence: 100 });
        await jest.advanceTimersByTimeAsync(30);

        expect(mockGetEvents).not.toHaveBeenCalled();
        expect(callback).not.toHaveBeenCalled();
        expect(jest.getTimerCount()).toBe(0);
      });

      it("ignores an in-flight dispute response after unsubscribe", async () => {
        const response = {
          latestLedger: 100,
          events: [
            disputeEvent("DsptFild", subjectAddress, "tx_stats"),
            disputeEvent("DsptRslv", subjectAddress, "repayment"),
            disputeEvent("DsptRjct", subjectAddress, "vc_count"),
          ],
        };
        let resolveEvents!: (value: typeof response) => void;
        mockGetEvents.mockReturnValueOnce(
          new Promise<typeof response>((resolve) => {
            resolveEvents = resolve;
          }),
        );
        const sdk = new StellarDIDCreditSDK({ ...mockConfig, pollIntervalMs: 10 });
        const callback = jest.fn();
        const unsubscribe = sdk.subscribeToDisputeEvents(subjectAddress, callback);

        await jest.advanceTimersByTimeAsync(0);
        expect(mockGetEvents).toHaveBeenCalledTimes(1);
        unsubscribe();
        resolveEvents(response);
        await jest.advanceTimersByTimeAsync(30);

        expect(callback).not.toHaveBeenCalled();
        expect(mockGetEvents).toHaveBeenCalledTimes(1);
        expect(jest.getTimerCount()).toBe(0);
      });

      it("stops the current dispute batch when unsubscribed inside the callback", async () => {
        mockGetEvents.mockResolvedValueOnce({
          latestLedger: 100,
          events: [
            disputeEvent("DsptFild", subjectAddress, "tx_stats"),
            disputeEvent("DsptRslv", subjectAddress, "tx_stats"),
            disputeEvent("DsptRjct", subjectAddress, "repayment"),
          ],
        });
        const sdk = new StellarDIDCreditSDK({ ...mockConfig, pollIntervalMs: 10 });
        const callback = jest.fn(() => unsubscribe());
        const unsubscribe = sdk.subscribeToDisputeEvents(subjectAddress, callback);

        await jest.advanceTimersByTimeAsync(30);

        expect(callback).toHaveBeenCalledTimes(1);
        expect(callback).toHaveBeenCalledWith({
          eventType: "DsptFild",
          subject: subjectAddress,
          inputKey: "tx_stats",
          status: "Pending",
        });
        expect(mockGetEvents).toHaveBeenCalledTimes(1);
        expect(jest.getTimerCount()).toBe(0);
      });

      it("keeps another subject subscribed when one subscription is stopped", async () => {
        const initialResponse = {
          latestLedger: 100,
          events: [
            disputeEvent("DsptFild", subjectAddress, "tx_stats"),
            disputeEvent("DsptFild", otherSubject, "repayment"),
          ],
        };
        mockGetEvents
          .mockResolvedValueOnce(initialResponse)
          .mockResolvedValueOnce(initialResponse)
          .mockResolvedValueOnce({
            latestLedger: 101,
            events: [
              {
                ...disputeEvent("DsptRslv", subjectAddress, "tx_stats"),
                ledger: 101,
              },
              {
                ...disputeEvent("DsptRjct", otherSubject, "repayment"),
                ledger: 101,
              },
            ],
          });
        const sdk = new StellarDIDCreditSDK({ ...mockConfig, pollIntervalMs: 10 });
        const firstCallback = jest.fn();
        const secondCallback = jest.fn();
        const stopFirst = sdk.subscribeToDisputeEvents(
          subjectAddress,
          firstCallback,
        );
        const stopSecond = sdk.subscribeToDisputeEvents(
          otherSubject,
          secondCallback,
        );

        await jest.advanceTimersByTimeAsync(0);
        expect(firstCallback).toHaveBeenCalledTimes(1);
        expect(secondCallback).toHaveBeenCalledTimes(1);
        expect(jest.getTimerCount()).toBe(2);

        stopFirst();
        expect(jest.getTimerCount()).toBe(1);
        await jest.advanceTimersByTimeAsync(10);
        expect(firstCallback).toHaveBeenCalledTimes(1);
        expect(secondCallback).toHaveBeenCalledTimes(2);
        expect(secondCallback).toHaveBeenLastCalledWith({
          eventType: "DsptRjct",
          subject: otherSubject,
          inputKey: "repayment",
          status: "Rejected",
        });
        expect(mockGetEvents).toHaveBeenCalledTimes(3);

        stopSecond();
        await jest.advanceTimersByTimeAsync(30);
        expect(mockGetEvents).toHaveBeenCalledTimes(3);
        expect(jest.getTimerCount()).toBe(0);
      });

      it("stops polling after unsubscribe", async () => {
        mockGetEvents
          .mockResolvedValueOnce({ latestLedger: 100, events: [] })
          .mockResolvedValueOnce({ latestLedger: 101, events: [] })
          .mockResolvedValue({
            latestLedger: 102,
            events: [
              {
                ...disputeEvent("DsptFild", subjectAddress, "tx_stats"),
                ledger: 102,
              },
            ],
          });

        const sdk = new StellarDIDCreditSDK({
          ...mockConfig,
          pollIntervalMs: 10,
        });
        const callback = jest.fn();
        const unsubscribe = sdk.subscribeToDisputeEvents(subjectAddress, callback);

        await jest.advanceTimersByTimeAsync(0);
        expect(mockGetEvents).toHaveBeenCalledTimes(1);
        await jest.advanceTimersByTimeAsync(10);
        expect(mockGetEvents).toHaveBeenCalledTimes(2);
        expect(jest.getTimerCount()).toBe(1);

        unsubscribe();
        expect(jest.getTimerCount()).toBe(0);
        await jest.advanceTimersByTimeAsync(30);
        expect(mockGetEvents).toHaveBeenCalledTimes(2);
        expect(callback).not.toHaveBeenCalled();
      });
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

      await expect(sdk.getDIDDocument(subjectAddress)).rejects.toMatchObject({
        name: "IdentityOracleError",
        code: 0,
        contractName: "identity-oracle",
        message: "contract error",
      });
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
        name: "RevocationRegistryError",
        code: 0,
        contractName: "revocation-registry",
        message: "IssuerMismatch",
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
      ).rejects.toMatchObject({
        name: "RevocationRegistryError",
        code: 4,
        contractName: "revocation-registry",
      });
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
      const promise = sdk.revokeVC(issuerKeypair as never, Buffer.alloc(32, 4));

      await jest.advanceTimersByTimeAsync(1000);

      await expect(promise).resolves.toBe("retried-revoke-hash");
      expect(mockSendTransaction).toHaveBeenCalledTimes(2);
      expect(mockGetTransaction).toHaveBeenCalledWith("retried-revoke-hash");
    });
  });

  describe("anchorDID", () => {
    it("throws a descriptive error when subjectKeypair public key does not match subject", async () => {
      const sdk = new StellarDIDCreditSDK(mockConfig);
      const wrongAddress =
        "GWRONGADDRESS12345678901234567890123456789012345678901234";

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
      mockSimulateTransaction.mockResolvedValue({
        error: "anchor_did rejected",
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(
        sdk.anchorDID(subjectKeypair as never, "QmExampleCid"),
      ).rejects.toMatchObject({
        name: "IdentityOracleError",
        code: 0,
        contractName: "identity-oracle",
        message: "anchor_did rejected",
      });
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

      const result = await sdk.anchorDID(
        subjectKeypair as never,
        "QmExampleCid",
      );

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
      mockSimulateTransaction.mockResolvedValue({
        error: "anchor_vc rejected",
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const vcHash = Buffer.alloc(32, 5);

      await expect(
        sdk.issueVC(issuerKeypair as never, subjectAddress, vcHash),
      ).rejects.toMatchObject({
        name: "IdentityOracleError",
        code: 0,
        contractName: "identity-oracle",
        message: "anchor_vc rejected",
      });
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

      const result = await sdk.issueVC(
        issuerKeypair as never,
        subjectAddress,
        vcHash,
      );

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

  describe("batchAnchorVCs", () => {
    beforeEach(() => {
      mockSimulateTransaction.mockReset();
      mockGetAccount.mockReset();
      mockSendTransaction.mockReset();
      mockGetTransaction.mockReset();
      mockContractCalls.length = 0;
      mockGetAccount.mockResolvedValue({ sequenceNumber: () => "1" });
      mockSimulateTransaction.mockResolvedValue({ result: {} });
      mockSendTransaction.mockResolvedValue({
        status: "PENDING",
        hash: "mock-tx-hash",
      });
      mockGetTransaction.mockResolvedValue({ status: "SUCCESS" });
    });

    it("chunks entries into transactions of at most 10 operations", async () => {
      const entries = Array.from({ length: 11 }, (_, index) => ({
        subject: subjectAddress,
        vcHash: Buffer.alloc(32, index + 1),
        ...(index === 0 ? { type: "kyc" } : {}),
      }));

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.batchAnchorVCs(issuerKeypair as never, entries);

      expect(result.success).toBe(true);
      expect(result.results).toHaveLength(2);
      expect(result.results[0]?.vcHashes).toHaveLength(10);
      expect(result.results[1]?.vcHashes).toHaveLength(1);
      expect(mockSendTransaction).toHaveBeenCalledTimes(2);
      expect(mockContractCalls.filter((call) => call.method === "anchor_vc"))
        .toHaveLength(10);
      expect(
        mockContractCalls.filter((call) => call.method === "anchor_vc_typed"),
      ).toHaveLength(1);
    });

    it("reports a failed chunk and continues with later chunks", async () => {
      const entries = Array.from({ length: 11 }, (_, index) => ({
        subject: subjectAddress,
        vcHash: Buffer.alloc(32, index + 1),
      }));
      mockSimulateTransaction
        .mockResolvedValueOnce({ error: "anchor_vc rejected" })
        .mockResolvedValueOnce({ result: {} });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.batchAnchorVCs(issuerKeypair as never, entries);

      expect(result.success).toBe(false);
      expect(result.failedChunks).toBe(1);
      expect(result.results).toHaveLength(2);
      expect(result.results[0]?.status).toBe("failed");
      expect(result.results[1]?.status).toBe("success");
      expect(mockSendTransaction).toHaveBeenCalledTimes(1);
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

    it("returns false when verify_vc reports a credential as revoked", async () => {
      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: { value: false },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.verifyVC(subjectAddress, Buffer.alloc(32, 12));

      expect(result).toBe(false);
      expect(mockLastContractCall?.method).toBe("verify_vc");
      expect(mockLastContractCall?.args).toHaveLength(2);
    });

    it("returns false when the contract is paused", async () => {
      mockSimulateTransaction.mockResolvedValue({
        error: "Error(Contract, #8)",
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.verifyVC(subjectAddress, Buffer.alloc(32, 9));

      expect(result).toBe(false);
      expect(mockLastContractCall?.method).toBe("verify_vc");
    });

    it("returns false for an unknown subject with no VC records", async () => {
      mockSimulateTransaction.mockResolvedValue({
        error: "unknown subject",
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.verifyVC(
        "GUNKNOWN1234567890123456789012345678901234567890123456",
        Buffer.alloc(32, 10),
      );

      expect(result).toBe(false);
      expect(mockLastContractCall?.method).toBe("verify_vc");
    });

    it("uses simAccount as read-only source account", async () => {
      const { TransactionBuilder } = jest.requireMock("@stellar/stellar-sdk");
      (TransactionBuilder as jest.Mock).mockClear();
      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: { value: true },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      await sdk.verifyVC(subjectAddress, Buffer.alloc(32, 11));

      expect(TransactionBuilder).toHaveBeenCalledWith(
        expect.objectContaining({
          accountId: mockConfig.simAccount,
          sequence: "0",
        }),
        expect.objectContaining({
          fee: "100",
          networkPassphrase: mockConfig.networkPassphrase,
        }),
      );
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
      ).rejects.toMatchObject({
        name: "IdentityOracleError",
        code: 0,
        contractName: "identity-oracle",
        message: "verify error",
      });
    });
  });

  describe("computeScore", () => {
    it("returns the computed score number after successful compute + confirmation", async () => {
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
                score: 612,
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

      expect(result).toBe(612);
      expect(typeof result).toBe("number");
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

      await expect(computePromise).resolves.toBe(612);
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

    it("throws SDKError with COOLDOWN_ACTIVE when cooldown is active on simulation", async () => {
      mockGetAccount.mockResolvedValue({ sequenceNumber: () => "123" });
      mockSimulateTransaction.mockResolvedValue({
        error: "Error(Contract, #7): ComputeCooldownActive",
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(
        sdk.computeScore(
          { publicKey: () => subjectAddress } as unknown as Keypair,
          subjectAddress,
        ),
      ).rejects.toMatchObject({
        name: "SDKError",
        code: "COOLDOWN_ACTIVE",
        message: expect.stringContaining("Cooldown period is active"),
      });
      expect(mockSendTransaction).not.toHaveBeenCalled();
    });

    it("throws SDKError with COOLDOWN_ACTIVE when cooldown error in submission", async () => {
      mockGetAccount.mockResolvedValue({ sequenceNumber: () => "123" });
      mockSendTransaction.mockResolvedValue({
        status: "PENDING",
        hash: "tx-cooldown-hash",
      });
      mockGetTransaction.mockResolvedValue({
        status: "FAILED",
        errorResult: "Error(Contract, #7): ComputeCooldownActive",
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
        name: "SDKError",
        code: "COOLDOWN_ACTIVE",
        message: expect.stringContaining("Cooldown period is active"),
      });
    });

    it("throws when computeScore simulation returns an explicit error", async () => {
      mockGetAccount.mockResolvedValue({ sequenceNumber: () => "123" });
      mockSimulateTransaction.mockResolvedValue({
        error: "compute_score rejected",
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(
        sdk.computeScore(
          { publicKey: () => subjectAddress } as unknown as Keypair,
          subjectAddress,
        ),
      ).rejects.toMatchObject({
        name: "CreditOracleError",
        code: 0,
        contractName: "credit-oracle",
        message: "compute_score rejected",
      });
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

      await expect(promise).resolves.toBe(640);
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

      await expect(sdk.getVCCount(subjectAddress)).rejects.toMatchObject({
        name: "IdentityOracleError",
        code: 0,
        contractName: "identity-oracle",
        message: "rpc error",
      });
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

      await expect(sdk.getVCs(subjectAddress)).rejects.toMatchObject({
        name: "IdentityOracleError",
        code: 0,
        contractName: "identity-oracle",
        message: "rpc error",
      });
    });
  });

  describe("listVCs", () => {
    it("returns the same VC records as getVCs including revoked", async () => {
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
      const result = await sdk.listVCs(subjectAddress);

      expect(result).toHaveLength(2);
      expect(result[1]?.revoked).toBe(true);
      expect(mockLastContractCall?.method).toBe("get_vc_details");
    });
  });

  describe("listRevokedByIssuer", () => {
    it("returns revoked credential hashes as Buffers", async () => {
      const revokedHashes = [Buffer.alloc(32, 1), Buffer.alloc(32, 2)];
      mockSimulateTransaction.mockResolvedValue({
        result: { retval: { value: revokedHashes } },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.listRevokedByIssuer(issuerKeypair.publicKey());

      expect(result).toEqual(revokedHashes);
      expect(result.every((hash) => Buffer.isBuffer(hash))).toBe(true);
      expect(mockLastContractCall).toMatchObject({
        contractId: mockConfig.revocationRegistryId,
        method: "list_revoked_for_issuer",
      });
      expect(mockLastContractCall?.args).toHaveLength(1);
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
      const result = await sdk.getCredentialType(
        subjectAddress,
        Buffer.alloc(32, 3),
      );

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
      const result = await sdk.getCredentialType(
        subjectAddress,
        Buffer.alloc(32, 4),
      );

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
      ).rejects.toMatchObject({
        name: "IdentityOracleError",
        code: 0,
        contractName: "identity-oracle",
        message: "rpc error",
      });
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

      await expect(sdk.isVerified(subjectAddress)).rejects.toMatchObject({
        name: "IdentityOracleError",
        code: 0,
        contractName: "identity-oracle",
        message: "rpc error",
      });
    });
  });

  describe("isDeactivated", () => {
    it("returns true when subject has deactivated identity", async () => {
      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: { value: true },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.isDeactivated(subjectAddress);

      expect(result).toBe(true);
      expect(mockLastContractCall?.method).toBe("is_deactivated");
      expect(mockLastContractCall?.args).toHaveLength(1);
    });

    it("returns false when subject has active identity", async () => {
      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: { value: false },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.isDeactivated(subjectAddress);

      expect(result).toBe(false);
      expect(mockLastContractCall?.method).toBe("is_deactivated");
    });

    it("throws on simulation error", async () => {
      mockSimulateTransaction.mockResolvedValue({ error: "rpc error" });

      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(sdk.isDeactivated(subjectAddress)).rejects.toMatchObject({
        name: "IdentityOracleError",
        code: 0,
        contractName: "identity-oracle",
        message: "rpc error",
      });
    });
  });

  describe("reactivateIdentity", () => {
    it("submits transaction to reactivate identity", async () => {
      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: { value: undefined }, // return value is ()
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.reactivateIdentity(subjectKeypair as never);

      expect(result).toBe("mock-tx-hash");
      expect(mockGetAccount).toHaveBeenCalledWith(subjectAddress);
      expect(mockSendTransaction).toHaveBeenCalled();
      expect(mockContractCalls[mockContractCalls.length - 1]).toMatchObject({
        contractId: mockConfig.identityOracleId,
        method: "reactivate_identity",
      });
    });

    it("throws on simulation error", async () => {
      mockSimulateTransaction.mockResolvedValue({ error: "rpc error" });

      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(
        sdk.reactivateIdentity(subjectKeypair as never),
      ).rejects.toMatchObject({
        name: "IdentityOracleError",
        code: 0,
        contractName: "identity-oracle",
        message: "rpc error",
      });
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

    it("returns scoring weights that sum to 100", async () => {
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

      expect(
        result.vcWeight + result.txWeight + result.repaymentWeight,
      ).toBe(100);
    });

    it("throws on simulation error", async () => {
      mockSimulateTransaction.mockResolvedValue({ error: "rpc error" });

      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(sdk.getWeights()).rejects.toMatchObject({
        name: "CreditOracleError",
        code: 0,
        contractName: "credit-oracle",
        message: "rpc error",
      });
    });
  });

  describe("getPendingWeights", () => {
    it("returns pending scoring weights and their effective ledger", async () => {
      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: {
            value: {
              weights: {
                vc_weight: 45,
                tx_weight: 30,
                repayment_weight: 25,
              },
              effective_ledger: 1_234_567,
            },
          },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.getPendingWeights();

      expect(result).toEqual({
        weights: {
          vcWeight: 45,
          txWeight: 30,
          repaymentWeight: 25,
        },
        effectiveLedger: 1_234_567,
      });
      expect(mockLastContractCall?.method).toBe("get_pending_weights");
    });

    it("returns pending scoring weights that sum to 100", async () => {
      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: {
            value: {
              weights: {
                vc_weight: 45,
                tx_weight: 30,
                repayment_weight: 25,
              },
              effective_ledger: 1_234_567,
            },
          },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.getPendingWeights();

      expect(result).not.toBeNull();
      const { vcWeight, txWeight, repaymentWeight } = result!.weights;
      expect(vcWeight + txWeight + repaymentWeight).toBe(100);
      expect(mockLastContractCall?.method).toBe("get_pending_weights");
    });

    it("returns null when no pending weights exist", async () => {
      mockSimulateTransaction.mockResolvedValue({
        result: { retval: { value: null } },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(sdk.getPendingWeights()).resolves.toBeNull();
      expect(mockLastContractCall?.method).toBe("get_pending_weights");
    });
  });

  describe("getIdentityProtocolStats", () => {
    it("returns identity protocol counters", async () => {
      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: {
            value: {
              total_dids_anchored: 2n,
              total_vcs_anchored: 5n,
              total_vcs_revoked: 1n,
            },
          },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.getIdentityProtocolStats();

      expect(result).toEqual({
        totalDIDsAnchored: 2,
        totalVCsAnchored: 5,
        totalVCsRevoked: 1,
      });
      expect(mockLastContractCall?.method).toBe("get_protocol_stats");
    });

    it("returns zero counters for a fresh contract", async () => {
      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: {
            value: {
              total_dids_anchored: 0n,
              total_vcs_anchored: 0n,
              total_vcs_revoked: 0n,
            },
          },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.getIdentityProtocolStats();

      expect(result).toEqual({
        totalDIDsAnchored: 0,
        totalVCsAnchored: 0,
        totalVCsRevoked: 0,
      });
      expect(mockLastContractCall?.method).toBe("get_protocol_stats");
    });

    it("throws on simulation error", async () => {
      mockSimulateTransaction.mockResolvedValue({ error: "rpc error" });

      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(sdk.getIdentityProtocolStats()).rejects.toMatchObject({
        name: "IdentityOracleError",
        code: 0,
        contractName: "identity-oracle",
        message: "rpc error",
      });
    });
  });

  describe("getCreditProtocolStats", () => {
    it("returns credit protocol counters", async () => {
      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: {
            value: {
              total_subjects_scored: 3n,
              total_repayments_recorded: 7n,
            },
          },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.getCreditProtocolStats();

      expect(result).toEqual({
        totalSubjectsScored: 3,
        totalRepaymentsRecorded: 7,
      });
      expect(mockLastContractCall?.method).toBe("get_protocol_stats");
    });

    it("returns zero counters for a fresh contract", async () => {
      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: {
            value: {
              total_subjects_scored: 0n,
              total_repayments_recorded: 0n,
            },
          },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.getCreditProtocolStats();

      expect(result).toEqual({
        totalSubjectsScored: 0,
        totalRepaymentsRecorded: 0,
      });
      expect(mockLastContractCall?.method).toBe("get_protocol_stats");
    });

    it("throws on simulation error", async () => {
      mockSimulateTransaction.mockResolvedValue({ error: "rpc error" });

      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(sdk.getCreditProtocolStats()).rejects.toMatchObject({
        name: "CreditOracleError",
        code: 0,
        contractName: "credit-oracle",
        message: "rpc error",
      });
    });
  });

  describe("getIssuerTier", () => {
    it("returns tier weight for an issuer", async () => {
      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: { value: 150 },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.getIssuerTier(issuerKeypair.publicKey());

      expect(result).toBe(150);
      expect(mockLastContractCall?.method).toBe("get_issuer_tier");
      expect(mockLastContractCall?.args).toHaveLength(1);
    });

    it("throws on simulation error", async () => {
      mockSimulateTransaction.mockResolvedValue({ error: "rpc error" });

      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(sdk.getIssuerTier(issuerKeypair.publicKey())).rejects.toMatchObject({
        name: "IdentityOracleError",
        code: 0,
        contractName: "identity-oracle",
        message: "rpc error",
      });
    });
  });

  describe("setIssuerTier", () => {
    const adminAddress = "GADMINAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    const adminKeypair = { publicKey: () => adminAddress };

    it("submits transaction to set issuer tier", async () => {
      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: { value: undefined },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.setIssuerTier(
        adminKeypair as never,
        issuerKeypair.publicKey(),
        120,
      );

      expect(result).toBe("mock-tx-hash");
      expect(mockGetAccount).toHaveBeenCalledWith(adminAddress);
      expect(mockSendTransaction).toHaveBeenCalled();
      expect(mockContractCalls[mockContractCalls.length - 1]).toMatchObject({
        contractId: mockConfig.identityOracleId,
        method: "set_issuer_tier",
      });
    });

    it("throws on simulation error", async () => {
      mockSimulateTransaction.mockResolvedValue({ error: "rpc error" });

      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(
        sdk.setIssuerTier(adminKeypair as never, issuerKeypair.publicKey(), 120),
      ).rejects.toMatchObject({
        name: "IdentityOracleError",
        code: 0,
        contractName: "identity-oracle",
        message: "rpc error",
      });
    });
  });

  describe("listIssuers", () => {
    it("returns list of registered issuer addresses", async () => {
      const issuers = ["GISSUER1", "GISSUER2"];
      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: { value: issuers },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.listIssuers();

      expect(result).toEqual(issuers);
      expect(mockLastContractCall?.method).toBe("list_issuers");
    });

    it("returns empty list on fresh contract", async () => {
      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: { value: [] },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.listIssuers();

      expect(result).toEqual([]);
      expect(mockLastContractCall?.method).toBe("list_issuers");
    });

    it("excludes deregistered issuers from returned list", async () => {
      const activeIssuers = ["GISSUER2", "GISSUER3"];
      const deregisteredIssuer = "GISSUER1";

      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: { value: activeIssuers },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.listIssuers();

      expect(result).toEqual(activeIssuers);
      expect(result).not.toContain(deregisteredIssuer);
      expect(mockLastContractCall?.method).toBe("list_issuers");
    });

    it("throws on simulation error", async () => {
      mockSimulateTransaction.mockResolvedValue({ error: "rpc error" });

      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(sdk.listIssuers()).rejects.toMatchObject({
        name: "IdentityOracleError",
        code: 0,
        contractName: "identity-oracle",
        message: "rpc error",
      });
    });

    it("throws when simulation is unsuccessful", async () => {
      mockSimulateTransaction.mockResolvedValue({});

      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(sdk.listIssuers()).rejects.toThrow(
        "Simulation returned unexpected response",
      );
    });

    it("throws when simulation result has no return value", async () => {
      mockSimulateTransaction.mockResolvedValue({
        result: {},
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(sdk.listIssuers()).rejects.toThrow(
        "No return value in simulation result",
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

    it("returns empty list on fresh contract", async () => {
      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: { value: [] },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.getRegisteredIssuers();

      expect(result).toEqual([]);
      expect(mockLastContractCall?.method).toBe("list_issuers");
    });

    it("excludes deregistered issuers from returned list", async () => {
      const activeIssuers = ["GISSUER2", "GISSUER3"];
      const deregisteredIssuer = "GISSUER1";

      mockSimulateTransaction.mockResolvedValue({
        result: {
          retval: { value: activeIssuers },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.getRegisteredIssuers();

      expect(result).toEqual(activeIssuers);
      expect(result).not.toContain(deregisteredIssuer);
      expect(mockLastContractCall?.method).toBe("list_issuers");
    });

    it("throws on simulation error", async () => {
      mockSimulateTransaction.mockResolvedValue({ error: "rpc error" });

      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(sdk.getRegisteredIssuers()).rejects.toMatchObject({
        name: "IdentityOracleError",
        code: 0,
        contractName: "identity-oracle",
        message: "rpc error",
      });
    });

    it("throws when simulation is unsuccessful", async () => {
      mockSimulateTransaction.mockResolvedValue({});

      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(sdk.getRegisteredIssuers()).rejects.toThrow(
        "Simulation returned unexpected response",
      );
    });

    it("throws when simulation result has no return value", async () => {
      mockSimulateTransaction.mockResolvedValue({
        result: {},
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(sdk.getRegisteredIssuers()).rejects.toThrow(
        "No return value in simulation result",
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

      await expect(sdk.getScore(subjectAddress)).rejects.toMatchObject({
        name: "CreditOracleError",
        code: 0,
        contractName: "credit-oracle",
        message: "some other error",
      });
    });

    it("exports ScoreNotComputedError class", () => {
      expect(ScoreNotComputedError).toBeDefined();
      const error = new ScoreNotComputedError(subjectAddress);
      expect(error).toBeInstanceOf(Error);
      expect(error.name).toBe("ScoreNotComputedError");
      expect(error.message).toContain(subjectAddress);
    });
  });

  describe("getTxStats", () => {
    it("returns TxStats on success", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        result: {
          retval: {
            value: {
              volume_30d: 1000n,
              tx_count_30d: 5,
              avg_counterparties: 2,
            },
          },
        },
      });
      const sdk = new StellarDIDCreditSDK(mockConfig);
      const stats = await sdk.getTxStats(subjectAddress);
      expect(stats).toEqual({
        volume30d: 1000n,
        txCount30d: 5,
        avgCounterparties: 2,
      });
      expect(mockLastContractCall).toMatchObject({
        contractId: mockConfig.creditOracleId,
        method: "get_tx_stats",
        args: [{ address: subjectAddress }],
      });
      expect(mockSimulateTransaction).toHaveBeenCalledTimes(1);
    });

    it("returns null if not found", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        result: { retval: { value: null } },
      });
      const sdk = new StellarDIDCreditSDK(mockConfig);
      const stats = await sdk.getTxStats(subjectAddress);
      expect(stats).toBeNull();
      expect(mockLastContractCall).toMatchObject({
        contractId: mockConfig.creditOracleId,
        method: "get_tx_stats",
        args: [{ address: subjectAddress }],
      });
    });
  });

  describe("getDispute", () => {
    it("returns null when simulation result is void", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        result: { retval: xdr.ScVal.scvVoid() },
      });
      const sdk = new StellarDIDCreditSDK(mockConfig);
      const dispute = await sdk.getDispute(subjectAddress, "tx_stats");
      expect(dispute).toBeNull();
      expect(mockLastContractCall).toMatchObject({
        contractId: mockConfig.creditOracleId,
        method: "get_dispute",
      });
    });

    it("returns parsed DisputeRecord on success", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        result: {
          retval: {
            value: {
              subject: subjectAddress,
              input_key: "tx_stats",
              reason: "incorrect value",
              filed_at_ledger: 12345,
              status: { Pending: null },
            },
          },
        },
      });
      const sdk = new StellarDIDCreditSDK(mockConfig);
      const dispute = await sdk.getDispute(subjectAddress, "tx_stats");
      expect(dispute).toEqual({
        subject: subjectAddress,
        inputKey: "tx_stats",
        reason: "incorrect value",
        filedAtLedger: 12345,
        status: "Pending",
      });
    });

    it("throws on simulation error", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        error: "bad dispute",
      });
      const sdk = new StellarDIDCreditSDK(mockConfig);
      await expect(
        sdk.getDispute(subjectAddress, "tx_stats"),
      ).rejects.toMatchObject({
        name: "CreditOracleError",
        contractName: "credit-oracle",
        message: "bad dispute",
      });
    });
  });

  describe("listDisputes", () => {
    it("returns empty array when simulation result is void/null", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        result: { retval: xdr.ScVal.scvVoid() },
      });
      const sdk = new StellarDIDCreditSDK(mockConfig);
      const disputes = await sdk.listDisputes(subjectAddress);
      expect(disputes).toEqual([]);
    });

    it("returns parsed DisputeRecord array on success", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        result: {
          retval: {
            value: [
              {
                subject: subjectAddress,
                input_key: "tx_stats",
                reason: "err",
                filed_at_ledger: 111,
                status: { Resolved: null },
              },
              {
                subject: subjectAddress,
                input_key: "vc_count",
                reason: "wrong",
                filed_at_ledger: 222,
                status: { Pending: null },
              },
            ],
          },
        },
      });
      const sdk = new StellarDIDCreditSDK(mockConfig);
      const disputes = await sdk.listDisputes(subjectAddress);
      expect(disputes).toHaveLength(2);
      expect(disputes[0].inputKey).toBe("tx_stats");
      expect(disputes[0].status).toBe("Resolved");
      expect(disputes[1].inputKey).toBe("vc_count");
      expect(disputes[1].status).toBe("Pending");
    });

    it("throws if result is not an array", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        result: { retval: { value: {} } },
      });
      const sdk = new StellarDIDCreditSDK(mockConfig);
      await expect(sdk.listDisputes(subjectAddress)).rejects.toThrow(
        "list_disputes returned an invalid result",
      );
    });
  });

  describe("getRepaymentRecord", () => {
    it("returns RepaymentRecord on success", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        result: {
          retval: {
            value: {
              on_time_count: 10,
              total_count: 12,
              total_repaid: 500n,
            },
          },
        },
      });
      const sdk = new StellarDIDCreditSDK(mockConfig);
      const record = await sdk.getRepaymentRecord(subjectAddress);
      expect(record).toEqual({
        onTimeCount: 10,
        totalCount: 12,
        totalRepaid: 500n,
      });
    });

    it("defaults the cumulative amount to zero for a legacy V1 record", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        result: {
          retval: {
            value: {
              on_time_count: 8,
              total_count: 10,
            },
          },
        },
      });
      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(sdk.getRepaymentRecord(subjectAddress)).resolves.toEqual({
        onTimeCount: 8,
        totalCount: 10,
        totalRepaid: 0n,
      });
    });

    it("returns null if not found", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        result: { retval: { value: null } },
      });
      const sdk = new StellarDIDCreditSDK(mockConfig);
      const record = await sdk.getRepaymentRecord(subjectAddress);
      expect(record).toBeNull();
    });
  });

  describe("verifyScoreProof", () => {
    it("returns true on successful verification", async () => {
      mockGetAccount.mockResolvedValueOnce({ sequenceNumber: () => "123" });
      mockSimulateTransaction.mockResolvedValueOnce({
        result: { retval: { value: null } },
      });
      mockSendTransaction.mockResolvedValueOnce({
        status: "PENDING",
        hash: "tx-hash-verify",
      });
      mockGetTransaction.mockResolvedValueOnce({ status: "SUCCESS" });

      // Proof payload must be at least 68 bytes (68-byte trailer minimum)
      const validProof = new Uint8Array(384 + 68); // 384-byte proof + 68-byte trailer

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const result = await sdk.verifyScoreProof(
        { publicKey: () => subjectAddress } as unknown as Keypair,
        subjectAddress,
        600,
        validProof,
        "C_VERIFIER"
      );
      expect(result).toBe(true);
    });

    it("throws SDKError if proof payload is too short", async () => {
      const sdk = new StellarDIDCreditSDK(mockConfig);
      await expect(
        sdk.verifyScoreProof(
          { publicKey: () => subjectAddress } as unknown as Keypair,
          subjectAddress,
          600,
          new Uint8Array([1, 2, 3]),
          "C_VERIFIER"
        )
      ).rejects.toThrow("Invalid proof payload: too short");
    });
  });

  describe("generateScoreProof", () => {
    it("throws if score not computed", async () => {
      const sdk = new StellarDIDCreditSDK(mockConfig);
      jest.spyOn(sdk, "getScore").mockResolvedValueOnce(null);

      await expect(sdk.generateScoreProof(subjectAddress, 600)).rejects.toThrow(
        "Score not computed"
      );
    });

    it("uses default stats if tx/repayment records are missing", async () => {
      const sdk = new StellarDIDCreditSDK(mockConfig);
      jest.spyOn(sdk, "getScore").mockResolvedValueOnce({
        score: 600,
        lastUpdated: 0,
        vcCount: 1,
        repaymentRate: 0,
        txVolume30d: 0n,
        previousScore: 0,
        computedAtLedger: 0,
        stale: false,
      });
      jest.spyOn(sdk, "getTxStats").mockResolvedValueOnce(null);
      jest.spyOn(sdk, "getRepaymentRecord").mockResolvedValueOnce(null);
      jest.spyOn(sdk, "getWeights").mockResolvedValueOnce({
        vcWeight: 40,
        txWeight: 30,
        repaymentWeight: 30,
      });

      // We just expect it to attempt generating a proof without throwing on missing stats
      // eslint-disable-next-line @typescript-eslint/no-var-requires
      const zkWasm = require("@stellar-did-credit/zk-wasm");
      jest.spyOn(zkWasm, "generate_score_proof").mockReturnValueOnce(new Uint8Array());
      
      const proof = await sdk.generateScoreProof(subjectAddress, 600);
      expect(proof).toBeInstanceOf(Uint8Array);
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

      const sdk = new StellarDIDCreditSDK({
        ...mockConfig,
        timeoutSeconds: 60,
      });
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
    expect(weights.vcWeight + weights.txWeight + weights.repaymentWeight).toBe(
      100,
    );
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
    const _repayment: RepaymentRecord = {
      onTimeCount: 0,
      totalCount: 0,
      totalRepaid: 0n,
    };
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
      previousScore: null,
      computedAtLedger: 0,
      stale: false,
    };
    const _config: ProtocolConfig = {
      identityOracleId: "",
      creditOracleId: "",
      revocationRegistryId: "",
      networkPassphrase: "",
      rpcUrl: "",
      simAccount: "",
    };
    const _govProp: GovernanceProposal = {
      id: 1n,
      proposer: "G",
      proposedWeights: _weights,
      votesFor: 0n,
      votesAgainst: 0n,
      expiryLedger: 0,
      executionDelayLedgers: 0,
      executed: false,
      cancelled: false,
      quorumRequired: 100n,
    };
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

describe("ContractError hierarchy", () => {
  it("exports ContractError base class with code and contractName", () => {
    const error = new ContractError(5, "test-contract", "something failed");
    expect(error).toBeInstanceOf(Error);
    expect(error).toBeInstanceOf(ContractError);
    expect(error.name).toBe("ContractError");
    expect(error.code).toBe(5);
    expect(error.contractName).toBe("test-contract");
    expect(error.message).toBe("something failed");
  });

  it("exports IdentityOracleError with code and contractName", () => {
    const error = new IdentityOracleError(2, "NotAuthorized (code 2)");
    expect(error).toBeInstanceOf(Error);
    expect(error).toBeInstanceOf(ContractError);
    expect(error).toBeInstanceOf(IdentityOracleError);
    expect(error.name).toBe("IdentityOracleError");
    expect(error.code).toBe(2);
    expect(error.contractName).toBe("identity-oracle");
  });

  it("exports CreditOracleError with code and contractName", () => {
    const error = new CreditOracleError(7, "ComputeCooldownActive (code 7)");
    expect(error).toBeInstanceOf(Error);
    expect(error).toBeInstanceOf(ContractError);
    expect(error).toBeInstanceOf(CreditOracleError);
    expect(error.name).toBe("CreditOracleError");
    expect(error.code).toBe(7);
    expect(error.contractName).toBe("credit-oracle");
  });

  it("exports RevocationRegistryError with code and contractName", () => {
    const error = new RevocationRegistryError(3, "IssuerMismatch (code 3)");
    expect(error).toBeInstanceOf(Error);
    expect(error).toBeInstanceOf(ContractError);
    expect(error).toBeInstanceOf(RevocationRegistryError);
    expect(error.name).toBe("RevocationRegistryError");
    expect(error.code).toBe(3);
    expect(error.contractName).toBe("revocation-registry");
  });

  it("exports GovernanceError with code and contractName", () => {
    const error = new GovernanceError(10, "QuorumNotMet (code 10)");
    expect(error).toBeInstanceOf(Error);
    expect(error).toBeInstanceOf(ContractError);
    expect(error).toBeInstanceOf(GovernanceError);
    expect(error.name).toBe("GovernanceError");
    expect(error.code).toBe(10);
    expect(error.contractName).toBe("governance");
  });

  it("mocks NotAuthorized result and produces IdentityOracleError with code 2", async () => {
    mockSimulateTransaction.mockResolvedValue({
      error: "Error(Contract, #2)",
    });
    const sdk = new StellarDIDCreditSDK(mockConfig);

    await expect(sdk.isVerified(subjectAddress)).rejects.toMatchObject({
      name: "IdentityOracleError",
      code: 2,
      contractName: "identity-oracle",
    });
  });

  it("mocks NotAuthorized result on credit-oracle and produces CreditOracleError with code 2", async () => {
    mockSimulateTransaction.mockResolvedValue({
      error: "Error(Contract, #2)",
    });
    const sdk = new StellarDIDCreditSDK(mockConfig);

    await expect(sdk.getWeights()).rejects.toMatchObject({
      name: "CreditOracleError",
      code: 2,
      contractName: "credit-oracle",
    });
  });
});

describe("parseContractErrorCode", () => {
  it("extracts numeric code from Error(Contract, #N) pattern", () => {
    expect(parseContractErrorCode("Error(Contract, #4)")).toBe(4);
    expect(parseContractErrorCode("Error(Contract, #12)")).toBe(12);
    expect(parseContractErrorCode("something Error(Contract, #1) else")).toBe(
      1,
    );
  });

  it("returns null for non-matching strings", () => {
    expect(parseContractErrorCode("IssuerMismatch")).toBeNull();
    expect(parseContractErrorCode("contract paused")).toBeNull();
    expect(parseContractErrorCode("some random error")).toBeNull();
    expect(parseContractErrorCode("")).toBeNull();
  });

  it("handles case-insensitive patterns", () => {
    expect(parseContractErrorCode("error(contract, #5)")).toBe(5);
    expect(parseContractErrorCode("ERROR(CONTRACT, #3)")).toBe(3);
  });

  it("maps InvalidVotingPeriod governance errors to their variant name", () => {
    expect(() =>
      throwContractError("Error(Contract, #19)", "governance"),
    ).toThrow(new GovernanceError(19, "InvalidVotingPeriod (code 19)"));
  });

  it("maps NoPendingWeights governance errors to their variant name", () => {
    expect(() =>
      throwContractError("Error(Contract, #15)", "governance"),
    ).toThrow(new GovernanceError(15, "NoPendingWeights (code 15)"));
  });

  it("maps ContractPaused governance errors to their variant name", () => {
    expect(() =>
      throwContractError("Error(Contract, #16)", "governance"),
    ).toThrow(new GovernanceError(16, "ContractPaused (code 16)"));
  });

  it("maps VoteTallyOverflow governance errors to their variant name", () => {
    expect(() =>
      throwContractError("Error(Contract, #17)", "governance"),
    ).toThrow(new GovernanceError(17, "VoteTallyOverflow (code 17)"));
  });

  it("maps ProposalRejected governance errors to their variant name", () => {
    expect(() =>
      throwContractError("Error(Contract, #18)", "governance"),
    ).toThrow(new GovernanceError(18, "ProposalRejected (code 18)"));
  });

  it("maps every governance error code 1-18 to its human-readable variant name", () => {
    const expectedNames: Record<number, string> = {
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
      15: "NoPendingWeights",
      16: "ContractPaused",
      17: "VoteTallyOverflow",
      18: "ProposalRejected",
    };
    for (const [code, name] of Object.entries(expectedNames)) {
      expect(() =>
        throwContractError(`Error(Contract, #${code})`, "governance"),
      ).toThrow(new GovernanceError(Number(code), `${name} (code ${code})`));
    }
  });

  it("maps InvalidAmount credit-oracle errors to their variant name", () => {
    expect(() =>
      throwContractError("Error(Contract, #17)", "credit-oracle"),
    ).toThrow(new CreditOracleError(17, "InvalidAmount (code 17)"));
  });

  it("maps InvalidComputeCooldown credit-oracle errors to their variant name", () => {
    expect(() =>
      throwContractError("Error(Contract, #18)", "credit-oracle"),
    ).toThrow(new CreditOracleError(18, "InvalidComputeCooldown (code 18)"));
  });

  it("maps InvalidIssuerTier identity-oracle errors to their variant name", () => {
    expect(() =>
      throwContractError("Error(Contract, #11)", "identity-oracle"),
    ).toThrow(new IdentityOracleError(11, "InvalidIssuerTier (code 11)"));
  });
});


describe("batchRevokeVC", () => {
  beforeEach(() => {
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
    (
      jest.requireMock("@stellar/stellar-sdk").SorobanRpc.Server as jest.Mock
    ).mockClear();
  });

  it("rejects the batch if any hash is not 32 bytes", async () => {
    const sdk = new StellarDIDCreditSDK(mockConfig);

    await expect(
      sdk.batchRevokeVC(issuerKeypair as never, [
        Buffer.alloc(32, 1),
        Buffer.alloc(31, 2),
      ]),
    ).rejects.toMatchObject({
      name: "SDKError",
      code: "INVALID_VC_HASH",
    });
    expect(mockGetAccount).not.toHaveBeenCalled();
    expect(mockSendTransaction).not.toHaveBeenCalled();
  });

  it("revokes a single chunk and returns a confirmed BatchResult", async () => {
    const vcHashes = [Buffer.alloc(32, 1), Buffer.alloc(32, 2)];
    mockSimulateTransaction.mockResolvedValueOnce({
      result: { retval: { value: 50 } },
    });

    const sdk = new StellarDIDCreditSDK(mockConfig);
    const result: BatchResult = await sdk.batchRevokeVC(
      issuerKeypair as never,
      vcHashes,
    );

    expect(result.results).toHaveLength(1);
    expect(result.results[0]?.status).toBe("success");
    expect(result.results[0]?.transactionHash).toBe("mock-tx-hash");
    expect(mockSendTransaction).toHaveBeenCalledTimes(1);
    expect(mockGetTransaction).toHaveBeenCalledWith("mock-tx-hash");
    const batchCalls = mockContractCalls.filter(
      (call) => call.method === "batch_revoke",
    );
    expect(batchCalls).toHaveLength(1);
    const firstArgs = batchCalls[0]?.args[1] as
      | { value?: Buffer[] }
      | Buffer[]
      | undefined;
    const firstChunkHashes = Array.isArray(firstArgs)
      ? firstArgs
      : (firstArgs?.value ?? []);
    expect(firstChunkHashes).toHaveLength(vcHashes.length);
  });

  it("falls back to the default batch size when get_batch_limit fails", async () => {
    const vcHashes = Array.from({ length: 100 }, (_, i) =>
      Buffer.alloc(32, i + 1),
    );
    mockSimulateTransaction
      .mockResolvedValueOnce({ error: "not implemented" })
      .mockResolvedValue({ result: {} });

    const sdk = new StellarDIDCreditSDK(mockConfig);
    const result: BatchResult = await sdk.batchRevokeVC(
      issuerKeypair as never,
      vcHashes,
    );

    expect(result.results).toHaveLength(2);
    expect(
      mockContractCalls.filter((call) => call.method === "batch_revoke"),
    ).toHaveLength(2);
  });

  it("sends all hashes in a single batch_revoke call when get_batch_limit is unavailable", async () => {
    // When the contract's `get_batch_limit` read fails (or the contract has no
    // BatchLimit set), `batchRevokeVC` sends everything in one `batch_revoke`
    // call. Unlike `batchAnchorVCs` — which uses the compile-time
    // BATCH_ANCHOR_MAX_SIZE for its own chunking — `batchRevokeVC` relies
    // exclusively on the contract-configured limit. Without a limit the
    // fallback is a single call; the contract itself enforces `MAX_BATCH_SIZE`
    // (see `contracts/revocation-registry/src/lib.rs`), so an oversized batch
    // fails at the contract layer with `BatchTooLarge`.
    //
    // This test pins that documented behavior. If a future change ever starts
    // chunking the fallback at BATCH_ANCHOR_MAX_SIZE, this assertion will fail
    // and must be updated alongside the implementation.
    const vcHashes = Array.from({ length: 25 }, (_, i) =>
      Buffer.alloc(32, i + 1),
    );

    // First simulate call is the get_batch_limit read -> fail so the SDK
    // exercises the fallback path.
    mockSimulateTransaction
      .mockResolvedValueOnce({ error: "not implemented" })
      .mockResolvedValue({ result: {} });

    const sdk = new StellarDIDCreditSDK(mockConfig);
    const result: BatchResult = await sdk.batchRevokeVC(
      issuerKeypair as never,
      vcHashes,
    );

    // Exactly one batch_revoke call — no fallback chunking.
    expect(result.results).toHaveLength(1);
    const batchCalls = mockContractCalls.filter(
      (call) => call.method === "batch_revoke",
    );
    expect(batchCalls).toHaveLength(1);

    // And that single call carries ALL hashes, not a truncated batch.
    const arg = batchCalls[0]?.args[1] as
      | { value?: Buffer[] }
      | Buffer[]
      | undefined;
    const sentHashes = Array.isArray(arg) ? arg : (arg?.value ?? []);
    expect(sentHashes).toHaveLength(vcHashes.length);

    expect(mockSendTransaction).toHaveBeenCalledTimes(1);
  });
  it("chunks 100 hashes into two sequential transactions of 50 hashes each", async () => {
    const vcHashes = Array.from({ length: 100 }, (_, i) =>
      Buffer.alloc(32, i + 1),
    );
    mockSimulateTransaction
      .mockResolvedValueOnce({ result: { retval: { value: 50 } } })
      .mockResolvedValueOnce({ result: {} })
      .mockResolvedValueOnce({ result: {} });

    const sdk = new StellarDIDCreditSDK(mockConfig);
    const result: BatchResult = await sdk.batchRevokeVC(
      issuerKeypair as never,
      vcHashes,
    );

    expect(result.results).toHaveLength(2);
    const batchCalls = mockContractCalls.filter(
      (call) => call.method === "batch_revoke",
    );
    expect(batchCalls).toHaveLength(2);
    const firstArgs = batchCalls[0]?.args[1] as
      | { value?: Buffer[] }
      | Buffer[]
      | undefined;
    const firstChunkHashes = Array.isArray(firstArgs)
      ? firstArgs
      : (firstArgs?.value ?? []);
    expect(firstChunkHashes).toHaveLength(50);
    const secondArgs = batchCalls[1]?.args[1] as
      | { value?: Buffer[] }
      | Buffer[]
      | undefined;
    const secondChunkHashes = Array.isArray(secondArgs)
      ? secondArgs
      : (secondArgs?.value ?? []);
    expect(secondChunkHashes).toHaveLength(50);
    expect(mockSendTransaction).toHaveBeenCalledTimes(2);
    expect(mockGetTransaction).toHaveBeenCalledTimes(2);
  });

  it("reports partial success when a later chunk fails", async () => {
    const vcHashes = Array.from({ length: 75 }, (_, i) =>
      Buffer.alloc(32, i + 1),
    );
    mockSimulateTransaction
      .mockResolvedValueOnce({ result: { retval: { value: 50 } } })
      .mockResolvedValueOnce({ result: {} })
      .mockResolvedValueOnce({ error: "Error(Contract, #4)" });

    const sdk = new StellarDIDCreditSDK(mockConfig);
    const result: BatchResult = await sdk.batchRevokeVC(
      issuerKeypair as never,
      vcHashes,
    );

    expect(result.results).toHaveLength(2);
    expect(result.results[0]?.status).toBe("success");
    expect(result.results[0]?.transactionHash).toBe("mock-tx-hash");
    expect(result.results[1]?.status).toBe("failed");
    expect(result.results[1]?.error).toBeDefined();
    expect(mockSendTransaction).toHaveBeenCalledTimes(1);
    expect(mockGetTransaction).toHaveBeenCalledTimes(1);
  });

  describe("recency decay config", () => {
    const adminKeypair = { publicKey: () => "GADMIN" };
    const mockConfigData = {
      enabled: true,
      decayBpsPerDay: 5,
      minRecencyBps: 5000,
    };

    it("gets an enabled recency decay config", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        result: {
          retval: {
            value: {
              enabled: true,
              decay_bps_per_day: 5,
              min_recency_bps: 5000,
            },
          },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const config = await sdk.getRecencyDecayConfig();

      expect(config).toEqual(mockConfigData);
      expect(mockContractCalls[0]).toMatchObject({
        contractId: mockConfig.creditOracleId,
        method: "get_recency_decay",
      });
    });

    it("gets a disabled recency decay config", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        result: {
          retval: {
            value: {
              enabled: false,
              decay_bps_per_day: 12,
              min_recency_bps: 7500,
            },
          },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const config = await sdk.getRecencyDecayConfig();

      expect(config).toEqual({
        enabled: false,
        decayBpsPerDay: 12,
        minRecencyBps: 7500,
      });
      expect(mockContractCalls[0]).toMatchObject({
        contractId: mockConfig.creditOracleId,
        method: "get_recency_decay",
      });
    });

    it("returns the documented defaults when no config has been set", async () => {
      // The credit-oracle fills in DEFAULT_DECAY_BPS_PER_DAY (5) and
      // DEFAULT_MIN_RECENCY_BPS (5000) with enabled: false until an admin
      // calls set_recency_decay (see test_get_recency_decay_defaults_to_disabled).
      mockSimulateTransaction.mockResolvedValueOnce({
        result: {
          retval: {
            value: {
              enabled: false,
              decay_bps_per_day: 5,
              min_recency_bps: 5000,
            },
          },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const config = await sdk.getRecencyDecayConfig();

      expect(config).toEqual({
        enabled: false,
        decayBpsPerDay: 5,
        minRecencyBps: 5000,
      });
      expect(mockContractCalls[0]).toMatchObject({
        contractId: mockConfig.creditOracleId,
        method: "get_recency_decay",
      });
    });

    it("throws when the simulation returns no retval", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({ result: {} });

      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(sdk.getRecencyDecayConfig()).rejects.toThrow(
        "No return value in simulation result",
      );
    });

    it("sets the recency decay config", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        result: {
          retval: {
            value: null,
          },
        },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const txHash = await sdk.setRecencyDecayConfig(adminKeypair as never, mockConfigData);

      expect(txHash).toBe("mock-tx-hash");
      expect(mockContractCalls[0]).toMatchObject({
        contractId: mockConfig.creditOracleId,
        method: "set_recency_decay",
      });
      expect(mockSendTransaction).toHaveBeenCalledTimes(1);
    });
  });

  describe("credential type weight", () => {
    const adminKeypair = { publicKey: () => "GADMIN" };

    it("returns the default weight for an unregistered credential type", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        result: { retval: { value: 100 } },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      await expect(sdk.getCredentialTypeWeight("unregistered-type")).resolves.toBe(100);
      expect(mockContractCalls[0]).toMatchObject({
        contractId: mockConfig.creditOracleId,
        method: "get_credential_type_weight",
        args: [{ value: "unregistered-type" }],
      });
    });

    it("sets a credential type weight using the admin signer", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        result: { retval: { value: null } },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      await expect(
        sdk.setCredentialTypeWeight(adminKeypair as never, "employment", 125),
      ).resolves.toBe("mock-tx-hash");
      expect(mockGetAccount).toHaveBeenCalledWith("GADMIN");
      expect(mockContractCalls[0]).toMatchObject({
        contractId: mockConfig.creditOracleId,
        method: "set_credential_type_weight",
      });
      expect(mockContractCalls[0]?.args[0]).toMatchObject({ address: "GADMIN" });
      expect(mockContractCalls[0]?.args[1]).toMatchObject({ value: "employment" });
      expect(mockContractCalls[0]?.args[2]).toMatchObject({ value: 125, type: "u32" });
      expect(mockSendTransaction).toHaveBeenCalledTimes(1);
    });

    it("rejects a setter call that is not authorized by the contract admin", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        error: "Error(Contract, #1)",
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      await expect(
        sdk.setCredentialTypeWeight(adminKeypair as never, "employment", 125),
      ).rejects.toMatchObject({
        name: "CreditOracleError",
        contractName: "credit-oracle",
      });
      expect(mockSendTransaction).not.toHaveBeenCalled();
    });
  });

  describe("compute cooldown", () => {
    const adminKeypair = { publicKey: () => "GADMIN" };

    it("gets the configured cooldown in ledgers", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        result: { retval: { value: 100 } },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      await expect(sdk.getComputeCooldownLedgers()).resolves.toBe(100);
      expect(mockContractCalls[0]).toMatchObject({
        contractId: mockConfig.creditOracleId,
        method: "get_compute_cooldown_ledgers",
        args: [],
      });
    });

    it("sets the configured cooldown with the admin signer", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        result: { retval: { value: null } },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      await expect(
        sdk.setComputeCooldownLedgers(adminKeypair as never, 100),
      ).resolves.toBe("mock-tx-hash");
      expect(mockContractCalls[0]).toMatchObject({
        contractId: mockConfig.creditOracleId,
        method: "set_compute_cooldown_ledgers",
      });
      expect(mockContractCalls[0]?.args[1]).toMatchObject({
        value: 100,
        type: "u32",
      });
      expect(mockSendTransaction).toHaveBeenCalledTimes(1);
    });

    it("returns the default cooldown of 1 ledger when none is stored", async () => {
      // get_compute_cooldown_ledgers falls back to
      // DEFAULT_COMPUTE_COOLDOWN_LEDGERS (1) when nothing is persisted.
      mockSimulateTransaction.mockResolvedValueOnce({
        result: { retval: { value: 1 } },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      await expect(sdk.getComputeCooldownLedgers()).resolves.toBe(1);
      expect(mockContractCalls[0]).toMatchObject({
        contractId: mockConfig.creditOracleId,
        method: "get_compute_cooldown_ledgers",
        args: [],
      });
      expect(mockSendTransaction).not.toHaveBeenCalled();
    });

    it("accepts the lower bound of 1 ledger", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        result: { retval: { value: null } },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      await expect(
        sdk.setComputeCooldownLedgers(adminKeypair as never, 1),
      ).resolves.toBe("mock-tx-hash");
      expect(mockContractCalls[0]).toMatchObject({
        contractId: mockConfig.creditOracleId,
        method: "set_compute_cooldown_ledgers",
      });
      expect(mockContractCalls[0]?.args[0]).toMatchObject({ address: "GADMIN" });
      expect(mockContractCalls[0]?.args[1]).toMatchObject({
        value: 1,
        type: "u32",
      });
      expect(mockSendTransaction).toHaveBeenCalledTimes(1);
    });

    it("accepts the upper bound of 86400 ledgers", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        result: { retval: { value: null } },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      await expect(
        sdk.setComputeCooldownLedgers(adminKeypair as never, 86_400),
      ).resolves.toBe("mock-tx-hash");
      expect(mockContractCalls[0]?.args[1]).toMatchObject({
        value: 86_400,
        type: "u32",
      });
      expect(mockSendTransaction).toHaveBeenCalledTimes(1);
    });

    it("surfaces InvalidComputeCooldown for a cooldown below the lower bound", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        error: "Error(Contract, #18)",
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      await expect(
        sdk.setComputeCooldownLedgers(adminKeypair as never, 0),
      ).rejects.toMatchObject({
        name: "CreditOracleError",
        code: 18,
        contractName: "credit-oracle",
        message: "InvalidComputeCooldown (code 18)",
      });
      expect(mockSendTransaction).not.toHaveBeenCalled();
    });

    it("surfaces InvalidComputeCooldown for a cooldown above the upper bound", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        error: "Error(Contract, #18)",
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      await expect(
        sdk.setComputeCooldownLedgers(adminKeypair as never, 86_401),
      ).rejects.toMatchObject({
        name: "CreditOracleError",
        code: 18,
        contractName: "credit-oracle",
        message: "InvalidComputeCooldown (code 18)",
      });
      expect(mockSendTransaction).not.toHaveBeenCalled();
    });
  });

  describe("pause and upgrade helpers", () => {
    const adminKeypair = { publicKey: () => "GADMINAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA" };
    const validWasmHash = Buffer.alloc(32, 1);
    const invalidWasmHash = Buffer.alloc(16, 1);

    it("pauseIdentityOracle submits a signed transaction to identity-oracle", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        result: { retval: { value: null } },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const txHash = await sdk.pauseIdentityOracle(adminKeypair as never);

      expect(txHash).toBe("mock-tx-hash");
      expect(mockContractCalls[0]).toMatchObject({
        contractId: mockConfig.identityOracleId,
        method: "pause",
        args: [],
      });
      expect(mockSendTransaction).toHaveBeenCalledTimes(1);
    });

    it("unpauseIdentityOracle submits a signed transaction to identity-oracle", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        result: { retval: { value: null } },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const txHash = await sdk.unpauseIdentityOracle(adminKeypair as never);

      expect(txHash).toBe("mock-tx-hash");
      expect(mockContractCalls[0]).toMatchObject({
        contractId: mockConfig.identityOracleId,
        method: "unpause",
        args: [],
      });
      expect(mockSendTransaction).toHaveBeenCalledTimes(1);
    });

    it("pauseCreditOracle submits a signed transaction with admin address to credit-oracle", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        result: { retval: { value: null } },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const txHash = await sdk.pauseCreditOracle(adminKeypair as never);

      expect(txHash).toBe("mock-tx-hash");
      expect(mockContractCalls[0]).toMatchObject({
        contractId: mockConfig.creditOracleId,
        method: "pause",
      });
      expect(mockContractCalls[0].args).toHaveLength(1);
      expect(mockSendTransaction).toHaveBeenCalledTimes(1);
    });

    it("unpauseCreditOracle submits a signed transaction with admin address to credit-oracle", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        result: { retval: { value: null } },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const txHash = await sdk.unpauseCreditOracle(adminKeypair as never);

      expect(txHash).toBe("mock-tx-hash");
      expect(mockContractCalls[0]).toMatchObject({
        contractId: mockConfig.creditOracleId,
        method: "unpause",
      });
      expect(mockContractCalls[0].args).toHaveLength(1);
      expect(mockSendTransaction).toHaveBeenCalledTimes(1);
    });

    it("pauseRevocationRegistry submits a signed transaction to revocation-registry", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        result: { retval: { value: null } },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const txHash = await sdk.pauseRevocationRegistry(adminKeypair as never);

      expect(txHash).toBe("mock-tx-hash");
      expect(mockContractCalls[0]).toMatchObject({
        contractId: mockConfig.revocationRegistryId,
        method: "pause",
        args: [],
      });
      expect(mockSendTransaction).toHaveBeenCalledTimes(1);
    });

    it("unpauseRevocationRegistry submits a signed transaction to revocation-registry", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        result: { retval: { value: null } },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const txHash = await sdk.unpauseRevocationRegistry(adminKeypair as never);

      expect(txHash).toBe("mock-tx-hash");
      expect(mockContractCalls[0]).toMatchObject({
        contractId: mockConfig.revocationRegistryId,
        method: "unpause",
        args: [],
      });
      expect(mockSendTransaction).toHaveBeenCalledTimes(1);
    });

    it("upgradeIdentityOracle submits a signed transaction with 32-byte wasm hash", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        result: { retval: { value: null } },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const txHash = await sdk.upgradeIdentityOracle(
        adminKeypair as never,
        validWasmHash,
      );

      expect(txHash).toBe("mock-tx-hash");
      expect(mockContractCalls[0]).toMatchObject({
        contractId: mockConfig.identityOracleId,
        method: "upgrade",
      });
      // The contract verifies the stored admin through require_admin(), so the
      // only argument is the wasm hash, sent as bytes rather than an address.
      expect(mockContractCalls[0].args).toHaveLength(1);
      expect(mockContractCalls[0].args[0]).toMatchObject({
        value: new Uint8Array(validWasmHash),
        type: "bytes",
      });
      // The admin keypair is the transaction source, which is what authorizes
      // the upgrade on-chain.
      expect(mockGetAccount).toHaveBeenCalledWith(adminKeypair.publicKey());
      expect(mockSendTransaction).toHaveBeenCalledTimes(1);
    });

    it("upgradeIdentityOracle rejects invalid wasm hash", async () => {
      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(
        sdk.upgradeIdentityOracle(adminKeypair as never, invalidWasmHash),
      ).rejects.toThrow("newWasmHash must be a 32-byte Buffer");

      await expect(
        sdk.upgradeIdentityOracle(
          adminKeypair as never,
          "not-a-buffer" as unknown as Buffer,
        ),
      ).rejects.toThrow("newWasmHash must be a 32-byte Buffer");
    });

    it("upgradeRevocationRegistry submits a signed transaction with 32-byte wasm hash", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        result: { retval: { value: null } },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const txHash = await sdk.upgradeRevocationRegistry(
        adminKeypair as never,
        validWasmHash,
      );

      expect(txHash).toBe("mock-tx-hash");
      expect(mockContractCalls[0]).toMatchObject({
        contractId: mockConfig.revocationRegistryId,
        method: "upgrade",
      });
      expect(mockContractCalls[0].args).toHaveLength(1);
      expect(mockContractCalls[0].args[0]).toMatchObject({
        value: new Uint8Array(validWasmHash),
        type: "bytes",
      });
      expect(mockGetAccount).toHaveBeenCalledWith(adminKeypair.publicKey());
      expect(mockSendTransaction).toHaveBeenCalledTimes(1);
    });

    it("upgradeRevocationRegistry rejects invalid wasm hash", async () => {
      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(
        sdk.upgradeRevocationRegistry(adminKeypair as never, invalidWasmHash),
      ).rejects.toThrow("newWasmHash must be a 32-byte Buffer");

      await expect(
        sdk.upgradeRevocationRegistry(
          adminKeypair as never,
          "not-a-buffer" as unknown as Buffer,
        ),
      ).rejects.toThrow("newWasmHash must be a 32-byte Buffer");
    });

    it("upgradeCreditOracle submits a signed transaction with admin address and 32-byte wasm hash", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        result: { retval: { value: null } },
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      const txHash = await sdk.upgradeCreditOracle(
        adminKeypair as never,
        validWasmHash,
      );

      expect(txHash).toBe("mock-tx-hash");
      expect(mockContractCalls[0]).toMatchObject({
        contractId: mockConfig.creditOracleId,
        method: "upgrade",
      });
      // credit-oracle::upgrade takes (admin, new_wasm_hash) explicitly and
      // calls admin.require_auth() itself.
      expect(mockContractCalls[0].args).toHaveLength(2);
      expect(mockContractCalls[0].args[0]).toMatchObject({
        address: adminKeypair.publicKey(),
      });
      expect(mockContractCalls[0].args[1]).toMatchObject({
        value: new Uint8Array(validWasmHash),
        type: "bytes",
      });
      expect(mockGetAccount).toHaveBeenCalledWith(adminKeypair.publicKey());
      expect(mockSendTransaction).toHaveBeenCalledTimes(1);
    });

    it("upgradeCreditOracle rejects invalid wasm hash", async () => {
      const sdk = new StellarDIDCreditSDK(mockConfig);

      await expect(
        sdk.upgradeCreditOracle(adminKeypair as never, invalidWasmHash),
      ).rejects.toThrow("newWasmHash must be a 32-byte Buffer");

      await expect(
        sdk.upgradeCreditOracle(
          adminKeypair as never,
          "not-a-buffer" as unknown as Buffer,
        ),
      ).rejects.toThrow("newWasmHash must be a 32-byte Buffer");
    });

    it("rejects empty and oversized wasm hashes for all three contracts", async () => {
      const sdk = new StellarDIDCreditSDK(mockConfig);
      const emptyHash = Buffer.alloc(0);
      const oversizedHash = Buffer.alloc(33, 1);
      const hexString = "a".repeat(64);

      for (const hash of [emptyHash, oversizedHash, hexString]) {
        await expect(
          sdk.upgradeIdentityOracle(adminKeypair as never, hash as Buffer),
        ).rejects.toThrow("newWasmHash must be a 32-byte Buffer");
        await expect(
          sdk.upgradeRevocationRegistry(adminKeypair as never, hash as Buffer),
        ).rejects.toThrow("newWasmHash must be a 32-byte Buffer");
        await expect(
          sdk.upgradeCreditOracle(adminKeypair as never, hash as Buffer),
        ).rejects.toThrow("newWasmHash must be a 32-byte Buffer");
      }

      // A rejected hash must never reach the network.
      expect(mockSimulateTransaction).not.toHaveBeenCalled();
      expect(mockSendTransaction).not.toHaveBeenCalled();
    });

    it("upgradeIdentityOracle surfaces a non-admin simulation failure as IdentityOracleError", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        error: "HostError: Error(Contract, #2)",
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      await expect(
        sdk.upgradeIdentityOracle(adminKeypair as never, validWasmHash),
      ).rejects.toThrow(IdentityOracleError);
      expect(mockSendTransaction).not.toHaveBeenCalled();
    });

    it("upgradeCreditOracle surfaces a non-admin simulation failure as CreditOracleError", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        error: "HostError: Error(Contract, #2)",
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      await expect(
        sdk.upgradeCreditOracle(adminKeypair as never, validWasmHash),
      ).rejects.toThrow(CreditOracleError);
      expect(mockSendTransaction).not.toHaveBeenCalled();
    });

    it("upgradeRevocationRegistry surfaces a non-admin simulation failure as RevocationRegistryError", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        error: "HostError: Error(Contract, #2)",
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      await expect(
        sdk.upgradeRevocationRegistry(adminKeypair as never, validWasmHash),
      ).rejects.toThrow(RevocationRegistryError);
      expect(mockSendTransaction).not.toHaveBeenCalled();
    });

    it("throws typed ContractError if simulation fails", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({
        error: "HostError: Error(Contract, #2)",
      });

      const sdk = new StellarDIDCreditSDK(mockConfig);
      await expect(
        sdk.pauseIdentityOracle(adminKeypair as never),
      ).rejects.toThrow(IdentityOracleError);
    });

    it("throws unexpected response error if simulation is not successful", async () => {
      mockSimulateTransaction.mockResolvedValueOnce({});

      const sdk = new StellarDIDCreditSDK(mockConfig);
      await expect(
        sdk.pauseCreditOracle(adminKeypair as never),
      ).rejects.toThrow("Simulation returned unexpected response");
    });
  });
});

describe("parseScoreRecord", () => {
  it("test_parseScoreRecord_valid", () => {
    const scoreRecord = {
      score: 750,
      last_updated: 1234567890,
      vc_count: 5,
      repayment_rate: 9500,
      tx_volume_30d: 1000000,
      previous_score: 720,
      computed_at_ledger: 5000,
      stale: false,
    };

    const scVal = { value: scoreRecord } as never;
    const result = parseScoreRecord(scVal);

    expect(result).not.toBeNull();
    expect(result!.score).toBe(750);
    expect(result!.lastUpdated).toBe(1234567890);
    expect(result!.vcCount).toBe(5);
    expect(result!.repaymentRate).toBe(9500);
    expect(result!.txVolume30d).toBe(BigInt(1000000));
    expect(result!.previousScore).toBe(720);
    expect(result!.computedAtLedger).toBe(5000);
    expect(result!.stale).toBe(false);
  });

  it("test_parseScoreRecord_throws_on_missing_field", () => {
    const scoreRecord = {
      score: 750,
      last_updated: 1234567890,
      vc_count: 5,
      repayment_rate: 9500,
      tx_volume_30d: 1000000,
      previous_score: 720,
      computed_at_ledger: 5000,
      // stale intentionally missing
    };

    const scVal = { value: scoreRecord } as never;

    expect(() => parseScoreRecord(scVal)).toThrow(
      "parseScoreRecord: missing field 'stale' in ScoreRecord",
    );
  });
});

describe("getIssuerTier", () => {
  beforeEach(() => {
    mockSimulateTransaction.mockReset();
    mockGetAccount.mockReset();
    mockSendTransaction.mockReset();
    mockGetTransaction.mockReset();
    mockContractCalls.length = 0;
    mockLastContractCall = undefined;
    mockGetAccount.mockResolvedValue({ sequenceNumber: () => "1" });
    mockSendTransaction.mockResolvedValue({
      status: "PENDING",
      hash: "mock-tx-hash",
    });
    mockGetTransaction.mockResolvedValue({ status: "SUCCESS" });
    (
      jest.requireMock("@stellar/stellar-sdk").SorobanRpc.Server as jest.Mock
    ).mockClear();
  });

  const issuerAddress = "GISSUERAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

  it("returns the default tier (100 bps) when no tier has been set", async () => {
    mockSimulateTransaction.mockResolvedValueOnce({
      result: { retval: { value: 100 } },
    });

    const sdk = new StellarDIDCreditSDK(mockConfig);
    const tier = await sdk.getIssuerTier(issuerAddress);

    expect(tier).toBe(100);
    expect(mockContractCalls[0]).toMatchObject({
      contractId: mockConfig.identityOracleId,
      method: "get_issuer_tier",
    });
  });

  it("returns a custom tier value when one has been set", async () => {
    mockSimulateTransaction.mockResolvedValueOnce({
      result: { retval: { value: 200 } },
    });

    const sdk = new StellarDIDCreditSDK(mockConfig);
    const tier = await sdk.getIssuerTier(issuerAddress);

    expect(tier).toBe(200);
  });

  it("throws when simulation returns an error", async () => {
    mockSimulateTransaction.mockResolvedValueOnce({
      error: "Error(Contract, #11)",
    });

    const sdk = new StellarDIDCreditSDK(mockConfig);
    await expect(sdk.getIssuerTier(issuerAddress)).rejects.toThrow(
      IdentityOracleError,
    );
  });

  it("throws when simulation returns an unexpected response", async () => {
    mockSimulateTransaction.mockResolvedValueOnce({});

    const sdk = new StellarDIDCreditSDK(mockConfig);
    await expect(sdk.getIssuerTier(issuerAddress)).rejects.toThrow(
      "Simulation returned unexpected response",
    );
  });
});

describe("setIssuerTier", () => {
  beforeEach(() => {
    mockSimulateTransaction.mockReset();
    mockGetAccount.mockReset();
    mockSendTransaction.mockReset();
    mockGetTransaction.mockReset();
    mockContractCalls.length = 0;
    mockLastContractCall = undefined;
    mockGetAccount.mockResolvedValue({ sequenceNumber: () => "1" });
    mockSendTransaction.mockResolvedValue({
      status: "PENDING",
      hash: "mock-tx-hash",
    });
    mockGetTransaction.mockResolvedValue({ status: "SUCCESS" });
    (
      jest.requireMock("@stellar/stellar-sdk").SorobanRpc.Server as jest.Mock
    ).mockClear();
  });

  const adminKeypair = {
    publicKey: () => "GADMINAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
  };
  const issuerAddress = "GISSUERAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

  it("sets the tier and returns a transaction hash", async () => {
    mockSimulateTransaction.mockResolvedValueOnce({
      result: { retval: { value: null } },
    });

    const sdk = new StellarDIDCreditSDK(mockConfig);
    const txHash = await sdk.setIssuerTier(adminKeypair as never, issuerAddress, 150);

    expect(txHash).toBe("mock-tx-hash");
    expect(mockContractCalls[0]).toMatchObject({
      contractId: mockConfig.identityOracleId,
      method: "set_issuer_tier",
    });
    expect(mockSendTransaction).toHaveBeenCalledTimes(1);
  });

  it("passes the weight_bps as a u32 ScVal", async () => {
    mockSimulateTransaction.mockResolvedValueOnce({
      result: { retval: { value: null } },
    });

    const sdk = new StellarDIDCreditSDK(mockConfig);
    await sdk.setIssuerTier(adminKeypair as never, issuerAddress, 150);

    const args = mockContractCalls[0]?.args;
    // args[2] is the weightBps encoded via nativeToScVal
    expect(args?.[2]).toMatchObject({ value: 150, type: "u32" });
  });

  it("sets the minimum valid tier (1 bps)", async () => {
    mockSimulateTransaction.mockResolvedValueOnce({
      result: { retval: { value: null } },
    });

    const sdk = new StellarDIDCreditSDK(mockConfig);
    const txHash = await sdk.setIssuerTier(adminKeypair as never, issuerAddress, 1);

    expect(txHash).toBe("mock-tx-hash");
    expect(mockContractCalls[0]?.args[2]).toMatchObject({ value: 1, type: "u32" });
  });

  it("sets the maximum valid tier (300 bps)", async () => {
    mockSimulateTransaction.mockResolvedValueOnce({
      result: { retval: { value: null } },
    });

    const sdk = new StellarDIDCreditSDK(mockConfig);
    const txHash = await sdk.setIssuerTier(adminKeypair as never, issuerAddress, 300);

    expect(txHash).toBe("mock-tx-hash");
    expect(mockContractCalls[0]?.args[2]).toMatchObject({ value: 300, type: "u32" });
  });

  it("throws when simulation returns a contract error (e.g. InvalidIssuerTier)", async () => {
    mockSimulateTransaction.mockResolvedValueOnce({
      error: "Error(Contract, #11)",
    });

    const sdk = new StellarDIDCreditSDK(mockConfig);
    await expect(
      sdk.setIssuerTier(adminKeypair as never, issuerAddress, 0),
    ).rejects.toThrow(IdentityOracleError);
  });

  it("throws NotAuthorized when a non-admin caller is rejected by the contract", async () => {
    mockSimulateTransaction.mockResolvedValueOnce({
      error: "Error(Contract, #2)",
    });

    const nonAdminKeypair = {
      publicKey: () => "GNONADMINAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    };
    const sdk = new StellarDIDCreditSDK(mockConfig);
    await expect(
      sdk.setIssuerTier(nonAdminKeypair as never, issuerAddress, 150),
    ).rejects.toThrow(IdentityOracleError);
  });

  it("throws when simulation returns an unexpected response", async () => {
    mockSimulateTransaction.mockResolvedValueOnce({});

    const sdk = new StellarDIDCreditSDK(mockConfig);
    await expect(
      sdk.setIssuerTier(adminKeypair as never, issuerAddress, 150),
    ).rejects.toThrow("Simulation returned unexpected response");
  });
});

describe("subscribeToScoreEvents", () => {
  beforeEach(() => {
    jest.useFakeTimers();
    mockGetLatestLedger.mockReset();
    mockGetEvents.mockReset();
    mockGetLatestLedger.mockResolvedValue({ sequence: 200 });
    (
      jest.requireMock("@stellar/stellar-sdk").SorobanRpc.Server as jest.Mock
    ).mockClear();
  });

  afterEach(() => {
    jest.useRealTimers();
  });

  it("delivers a typed ScoreEvent to the callback", async () => {
    mockGetEvents.mockResolvedValueOnce({
      latestLedger: 200,
      events: [
        {
          ledger: 200,
          ledgerClosedAt: "2026-10-15T12:00:00Z",
          value: { value: [subjectAddress, 720] },
        },
      ],
    });

    const sdk = new StellarDIDCreditSDK({ ...mockConfig, pollIntervalMs: 10 });
    const received: ScoreEvent[] = [];
    const unsubscribe = sdk.subscribeToScoreEvents((event) => {
      received.push(event);
    });

    await Promise.resolve();
    await Promise.resolve();

    expect(received).toHaveLength(1);
    expect(received[0]).toEqual({
      subject: subjectAddress,
      score: 720,
      ledger: 200,
      timestamp: Math.floor(new Date("2026-10-15T12:00:00Z").getTime() / 1000),
    });

    unsubscribe();
  });

  it("filters events on the creditOracleId contract with Score topic", async () => {
    mockGetEvents.mockResolvedValueOnce({
      latestLedger: 200,
      events: [],
    });

    const sdk = new StellarDIDCreditSDK({ ...mockConfig, pollIntervalMs: 10 });
    const unsubscribe = sdk.subscribeToScoreEvents(jest.fn());

    await Promise.resolve();
    await Promise.resolve();

    expect(mockGetEvents).toHaveBeenCalledWith(
      expect.objectContaining({
        filters: [
          {
            type: "contract",
            contractIds: [mockConfig.creditOracleId],
            topics: [["symbol:Score"]],
          },
        ],
      }),
    );

    unsubscribe();
  });

  it("stops polling after unsubscribe is called", async () => {
    mockGetEvents.mockResolvedValue({ latestLedger: 200, events: [] });

    const sdk = new StellarDIDCreditSDK({ ...mockConfig, pollIntervalMs: 10 });
    const unsubscribe = sdk.subscribeToScoreEvents(jest.fn());

    await Promise.resolve();
    await Promise.resolve();
    unsubscribe();

    const callsAfterUnsubscribe = mockGetEvents.mock.calls.length;
    await jest.advanceTimersByTimeAsync(50);
    expect(mockGetEvents.mock.calls.length).toBe(callsAfterUnsubscribe);
  });

  it("delivers multiple events across polls", async () => {
    mockGetEvents
      .mockResolvedValueOnce({
        latestLedger: 200,
        events: [
          {
            ledger: 200,
            ledgerClosedAt: "2026-10-15T12:00:00Z",
            value: { value: [subjectAddress, 650] },
          },
        ],
      })
      .mockResolvedValueOnce({
        latestLedger: 201,
        events: [
          {
            ledger: 201,
            ledgerClosedAt: "2026-10-15T12:00:05Z",
            value: { value: [subjectAddress, 680] },
          },
        ],
      });

    const sdk = new StellarDIDCreditSDK({ ...mockConfig, pollIntervalMs: 10 });
    const received: ScoreEvent[] = [];
    const unsubscribe = sdk.subscribeToScoreEvents((event) => {
      received.push(event);
    });

    await Promise.resolve();
    await Promise.resolve();
    expect(received).toHaveLength(1);
    expect(received[0]!.score).toBe(650);

    await jest.advanceTimersByTimeAsync(10);
    expect(received).toHaveLength(2);
    expect(received[1]!.score).toBe(680);

    unsubscribe();
  });
});

describe("event subscription convenience methods", () => {
  const customContractId =
    "CCUSTOMCONTRACTAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
  const issuer = "GISSUERAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
  const vcHash = Buffer.alloc(32, 7);

  beforeEach(() => {
    jest.useFakeTimers();
    mockGetLatestLedger.mockReset();
    mockGetEvents.mockReset();
    mockGetLatestLedger.mockResolvedValue({ sequence: 100 });
    (
      jest.requireMock("@stellar/stellar-sdk").SorobanRpc.Server as jest.Mock
    ).mockClear();
  });

  afterEach(() => {
    jest.useRealTimers();
  });

  describe("onVCAnchored", () => {
    it("polls the given contract for the VCAnch topic", async () => {
      mockGetEvents.mockResolvedValueOnce({ latestLedger: 100, events: [] });
      const sdk = new StellarDIDCreditSDK({ ...mockConfig, pollIntervalMs: 10 });
      const unsubscribe = sdk.onVCAnchored(customContractId, jest.fn());
      await Promise.resolve();
      await Promise.resolve();
      expect(mockGetEvents).toHaveBeenCalledWith(
        expect.objectContaining({
          filters: [
            {
              type: "contract",
              contractIds: [customContractId],
              topics: [["symbol:VCAnch"]],
            },
          ],
        }),
      );
      unsubscribe();
    });

    it("delivers typed (issuer, subject, vcHash) data to the callback", async () => {
      mockGetEvents.mockResolvedValueOnce({
        latestLedger: 100,
        events: [
          {
            ledger: 100,
            ledgerClosedAt: "2026-10-15T12:00:00Z",
            value: { value: [issuer, subjectAddress, vcHash] },
          },
        ],
      });
      const sdk = new StellarDIDCreditSDK({ ...mockConfig, pollIntervalMs: 10 });
      const callback = jest.fn();
      const unsubscribe = sdk.onVCAnchored(customContractId, callback);
      await Promise.resolve();
      await Promise.resolve();
      expect(callback).toHaveBeenCalledTimes(1);
      const [gotIssuer, gotSubject, gotHash] = callback.mock.calls[0];
      expect(gotIssuer).toBe(issuer);
      expect(gotSubject).toBe(subjectAddress);
      expect(Buffer.isBuffer(gotHash)).toBe(true);
      expect((gotHash as Buffer).equals(vcHash)).toBe(true);
      unsubscribe();
    });
  });

  describe("onScoreComputed", () => {
    it("polls the given contract for the Score topic", async () => {
      mockGetEvents.mockResolvedValueOnce({ latestLedger: 100, events: [] });
      const sdk = new StellarDIDCreditSDK({ ...mockConfig, pollIntervalMs: 10 });
      const unsubscribe = sdk.onScoreComputed(customContractId, jest.fn());
      await Promise.resolve();
      await Promise.resolve();
      expect(mockGetEvents).toHaveBeenCalledWith(
        expect.objectContaining({
          filters: [
            {
              type: "contract",
              contractIds: [customContractId],
              topics: [["symbol:Score"]],
            },
          ],
        }),
      );
      unsubscribe();
    });

    it("delivers typed (subject, score) data to the callback", async () => {
      mockGetEvents.mockResolvedValueOnce({
        latestLedger: 100,
        events: [
          {
            ledger: 100,
            ledgerClosedAt: "2026-10-15T12:00:00Z",
            value: { value: [subjectAddress, 742] },
          },
        ],
      });
      const sdk = new StellarDIDCreditSDK({ ...mockConfig, pollIntervalMs: 10 });
      const callback = jest.fn();
      const unsubscribe = sdk.onScoreComputed(customContractId, callback);
      await Promise.resolve();
      await Promise.resolve();
      expect(callback).toHaveBeenCalledTimes(1);
      const [gotSubject, gotScore] = callback.mock.calls[0];
      expect(gotSubject).toBe(subjectAddress);
      expect(typeof gotScore).toBe("number");
      expect(gotScore).toBe(742);
      unsubscribe();
    });
  });

  describe("onVCRevoked", () => {
    it("polls the given contract for the Revoked topic", async () => {
      mockGetEvents.mockResolvedValueOnce({ latestLedger: 100, events: [] });
      const sdk = new StellarDIDCreditSDK({ ...mockConfig, pollIntervalMs: 10 });
      const unsubscribe = sdk.onVCRevoked(customContractId, jest.fn());
      await Promise.resolve();
      await Promise.resolve();
      expect(mockGetEvents).toHaveBeenCalledWith(
        expect.objectContaining({
          filters: [
            {
              type: "contract",
              contractIds: [customContractId],
              topics: [["symbol:Revoked"]],
            },
          ],
        }),
      );
      unsubscribe();
    });

    it("delivers typed (issuer, vcHash) data to the callback", async () => {
      mockGetEvents.mockResolvedValueOnce({
        latestLedger: 100,
        events: [
          {
            ledger: 100,
            ledgerClosedAt: "2026-10-15T12:00:00Z",
            value: { value: [issuer, vcHash] },
          },
        ],
      });
      const sdk = new StellarDIDCreditSDK({ ...mockConfig, pollIntervalMs: 10 });
      const callback = jest.fn();
      const unsubscribe = sdk.onVCRevoked(customContractId, callback);
      await Promise.resolve();
      await Promise.resolve();
      expect(callback).toHaveBeenCalledTimes(1);
      const [gotIssuer, gotHash] = callback.mock.calls[0];
      expect(gotIssuer).toBe(issuer);
      expect(Buffer.isBuffer(gotHash)).toBe(true);
      expect((gotHash as Buffer).equals(vcHash)).toBe(true);
      unsubscribe();
    });
  });
});
