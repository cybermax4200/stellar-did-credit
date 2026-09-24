import { program } from "./index";
import { StellarDIDCreditSDK } from "@stellar-did-credit/sdk";

jest.mock("@stellar-did-credit/sdk");
jest.mock("./config", () => ({
  loadConfig: jest.fn(() => ({
    rpcUrl: "https://soroban-testnet.stellar.org",
    networkPassphrase: "Test SDF Network ; September 2015",
    creditOracleId: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM",
    simAccount: "GCZRUROFMVTL55BZJJBGIB3S366QKQMADFJG6SXCSJRTJNUQMCCR32IO",
  })),
  validateConfig: jest.fn(),
}));

describe("CLI get-score command", () => {
  let originalConsoleLog: typeof console.log;
  let originalConsoleError: typeof console.error;
  let originalProcessExit: typeof process.exit;
  let mockConsoleLog: jest.Mock;
  let mockConsoleError: jest.Mock;
  let mockProcessExit: jest.Mock;
  let mockGetScore: jest.Mock;

  const validSubject = "GCZRUROFMVTL55BZJJBGIB3S366QKQMADFJG6SXCSJRTJNUQMCCR32IO";

  beforeEach(() => {
    originalConsoleLog = console.log;
    originalConsoleError = console.error;
    originalProcessExit = process.exit;

    mockConsoleLog = jest.fn();
    mockConsoleError = jest.fn();
    mockProcessExit = jest.fn() as unknown as jest.Mock;

    console.log = mockConsoleLog;
    console.error = mockConsoleError;
    process.exit = mockProcessExit as unknown as typeof process.exit;

    mockGetScore = jest.fn();
    (StellarDIDCreditSDK as unknown as jest.Mock).mockImplementation(() => ({
      getScore: mockGetScore,
    }));

    const getScoreCmd = program.commands.find((c) => c.name() === "get-score");
    if (getScoreCmd) {
      getScoreCmd.setOptionValue("json", undefined);
    }
  });

  afterEach(() => {
    console.log = originalConsoleLog;
    console.error = originalConsoleError;
    process.exit = originalProcessExit;
    jest.clearAllMocks();
  });

  it("prints helpful message and exits 0 when score is null", async () => {
    mockGetScore.mockResolvedValue(null);

    await program.parseAsync(["node", "stellar-did", "get-score", validSubject]);

    expect(mockGetScore).toHaveBeenCalledWith(validSubject);
    expect(mockConsoleLog).toHaveBeenCalledWith(
      expect.stringContaining(`Fetching credit score for ${validSubject}`),
    );
    expect(mockConsoleLog).toHaveBeenCalledWith(
      "No credit score has been computed for this subject yet. Run `compute-score` first.",
    );
    expect(mockProcessExit).toHaveBeenCalledWith(0);
    expect(mockConsoleError).not.toHaveBeenCalled();
  });

  it("outputs JSON with score null and exits 0 when --json is passed and score is null", async () => {
    mockGetScore.mockResolvedValue(null);

    await program.parseAsync(["node", "stellar-did", "get-score", "--json", validSubject]);

    expect(mockGetScore).toHaveBeenCalledWith(validSubject);
    // Should NOT have printed the human-readable "Fetching..." banner in JSON mode
    expect(mockConsoleLog).not.toHaveBeenCalledWith(
      expect.stringContaining(`Fetching credit score for ${validSubject}`),
    );
    expect(mockConsoleLog).toHaveBeenCalledWith(
      JSON.stringify({ score: null }),
    );
    expect(mockProcessExit).toHaveBeenCalledWith(0);
    expect(mockConsoleError).not.toHaveBeenCalled();
  });

  it("outputs JSON with score record and exits 0 when --json is passed and score exists", async () => {
    const mockScoreRecord = {
      score: 750,
      lastUpdated: 1700000000,
      vcCount: 3,
      repaymentRate: 8500,
      txVolume30d: 1000000000n,
      previousScore: 720,
      computedAtLedger: 123456,
      stale: false,
    };
    mockGetScore.mockResolvedValue(mockScoreRecord);

    await program.parseAsync(["node", "stellar-did", "get-score", "--json", validSubject]);

    expect(mockGetScore).toHaveBeenCalledWith(validSubject);
    expect(mockConsoleLog).not.toHaveBeenCalledWith(
      expect.stringContaining(`Fetching credit score for ${validSubject}`),
    );
    expect(mockConsoleLog).toHaveBeenCalledWith(
      JSON.stringify(
        mockScoreRecord,
        (key, value) => (typeof value === "bigint" ? value.toString() : value),
        2,
      ),
    );
    expect(mockProcessExit).toHaveBeenCalledWith(0);
    expect(mockConsoleError).not.toHaveBeenCalled();
  });

  it("prints score table when score exists in human-readable mode", async () => {
    const mockScoreRecord = {
      score: 750,
      lastUpdated: 1700000000,
      vcCount: 3,
      repaymentRate: 8500,
      txVolume30d: 1000000000n,
      previousScore: 720,
      computedAtLedger: 123456,
      stale: false,
    };
    mockGetScore.mockResolvedValue(mockScoreRecord);

    await program.parseAsync(["node", "stellar-did", "get-score", validSubject]);

    expect(mockGetScore).toHaveBeenCalledWith(validSubject);
    expect(mockConsoleLog).toHaveBeenCalledWith(
      expect.stringContaining(`Fetching credit score for ${validSubject}`),
    );
    expect(mockConsoleLog).toHaveBeenCalledWith(
      expect.stringContaining("Credit Score: 750"),
    );
    expect(mockConsoleError).not.toHaveBeenCalled();
  });

  it("handles errors from sdk.getScore gracefully and exits 1", async () => {
    mockGetScore.mockRejectedValue(new Error("RPC network timeout"));

    await program.parseAsync(["node", "stellar-did", "get-score", validSubject]);

    expect(mockGetScore).toHaveBeenCalledWith(validSubject);
    expect(mockConsoleError).toHaveBeenCalledWith("Failed:", "RPC network timeout");
    expect(mockProcessExit).toHaveBeenCalledWith(1);
  });
});
