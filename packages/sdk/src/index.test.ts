import { StellarDIDCreditSDK, ScoreNotComputedError } from "./index";

const mockSimulateTransaction = jest.fn();
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
  Account: jest.fn().mockImplementation((accountId: string, sequence: string) => ({
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
      simulateTransaction: mockSimulateTransaction,
    })),
    Api: {
      isSimulationError: (sim: { error?: string }) => Boolean(sim.error),
      isSimulationSuccess: (sim: { result?: unknown }) => Boolean(sim.result),
      assembleTransaction: jest.fn().mockReturnValue({
        build: jest.fn().mockReturnValue({
          sign: jest.fn(),
        }),
      }),
    },
  },
}));

const mockConfig = {
  identityOracleId: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
  creditOracleId: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
  revocationRegistryId: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
  networkPassphrase: "Test SDF Network ; September 2015",
  rpcUrl: "http://localhost:8000",
  simAccount: "GBUQWP3BOUZX34ULNQG23RQ6F4YUSXHTQSXE7XDZT4A65XJLQRGEZSM",
};

const subjectAddress =
  "GBUQWP3BOUZX34ULNQG23RQ6F4YUSXHTQSXE7XDZT4A65XJLQRGEZSM";

describe("StellarDIDCreditSDK", () => {
  beforeEach(() => {
    mockSimulateTransaction.mockReset();
    mockLastContractCall = undefined;
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

      await expect(sdk.verifyVC(subjectAddress, Buffer.alloc(31))).rejects.toThrow(
        "vcHash must be exactly 32 bytes",
      );
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
