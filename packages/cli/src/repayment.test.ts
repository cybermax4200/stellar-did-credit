import { program } from "./index";
import { StellarDIDCreditSDK } from "@stellar-did-credit/sdk";

jest.mock("@stellar-did-credit/zk-wasm", () => ({}), { virtual: true });
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

describe("CLI repayment command", () => {
  let originalConsoleLog: typeof console.log;
  let originalConsoleError: typeof console.error;
  let originalProcessExit: typeof process.exit;
  let mockConsoleLog: jest.Mock;
  let mockConsoleError: jest.Mock;
  let mockProcessExit: jest.Mock;
  let mockGetRepaymentRecord: jest.Mock;

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

    mockGetRepaymentRecord = jest.fn();
    (StellarDIDCreditSDK as unknown as jest.Mock).mockImplementation(() => ({
      getRepaymentRecord: mockGetRepaymentRecord,
    }));

    const repaymentCmd = program.commands.find((c) => c.name() === "repayment");
    if (repaymentCmd) {
      repaymentCmd.setOptionValue("json", undefined);
    }
  });

  afterEach(() => {
    console.log = originalConsoleLog;
    console.error = originalConsoleError;
    process.exit = originalProcessExit;
    jest.clearAllMocks();
  });

  it("prints helpful message and exits 0 when repayment record is null", async () => {
    mockGetRepaymentRecord.mockResolvedValue(null);

    await program.parseAsync(["node", "stellar-did", "repayment", validSubject]);

    expect(mockGetRepaymentRecord).toHaveBeenCalledWith(validSubject);
    expect(mockConsoleLog).toHaveBeenCalledWith(
      expect.stringContaining(`Fetching repayment history for ${validSubject}`),
    );
    expect(mockConsoleLog).toHaveBeenCalledWith(
      "No repayment history found for this subject.",
    );
    expect(mockProcessExit).toHaveBeenCalledWith(0);
    expect(mockConsoleError).not.toHaveBeenCalled();
  });

  it("outputs JSON with repayment null and exits 0 when --json and record is null", async () => {
    mockGetRepaymentRecord.mockResolvedValue(null);

    await program.parseAsync(["node", "stellar-did", "repayment", "--json", validSubject]);

    expect(mockGetRepaymentRecord).toHaveBeenCalledWith(validSubject);
    expect(mockConsoleLog).not.toHaveBeenCalledWith(
      expect.stringContaining(`Fetching repayment history for ${validSubject}`),
    );
    expect(mockConsoleLog).toHaveBeenCalledWith(
      JSON.stringify({ repayment: null }),
    );
    expect(mockProcessExit).toHaveBeenCalledWith(0);
    expect(mockConsoleError).not.toHaveBeenCalled();
  });

  it("outputs JSON with repayment record including repaymentRateBps when --json", async () => {
    const mockRecord = {
      onTimeCount: 8,
      totalCount: 10,
      totalRepaid: 500000000n,
    };
    mockGetRepaymentRecord.mockResolvedValue(mockRecord);

    await program.parseAsync(["node", "stellar-did", "repayment", "--json", validSubject]);

    expect(mockGetRepaymentRecord).toHaveBeenCalledWith(validSubject);
    expect(mockConsoleLog).not.toHaveBeenCalledWith(
      expect.stringContaining(`Fetching repayment history for ${validSubject}`),
    );
    const loggedOutput = mockConsoleLog.mock.calls.find(
      ([arg]) => typeof arg === "string" && arg.includes("onTimeCount"),
    );
    expect(loggedOutput).toBeDefined();
    const parsed = JSON.parse(loggedOutput![0] as string);
    expect(parsed.onTimeCount).toBe(8);
    expect(parsed.totalCount).toBe(10);
    expect(parsed.totalRepaid).toBe("500000000");
    expect(parsed.repaymentRateBps).toBe(8000); // 8/10 * 10000
    expect(mockProcessExit).toHaveBeenCalledWith(0);
    expect(mockConsoleError).not.toHaveBeenCalled();
  });

  it("prints repayment table in human-readable mode when record exists", async () => {
    const mockRecord = {
      onTimeCount: 9,
      totalCount: 10,
      totalRepaid: 1000000000n,
    };
    mockGetRepaymentRecord.mockResolvedValue(mockRecord);

    await program.parseAsync(["node", "stellar-did", "repayment", validSubject]);

    expect(mockGetRepaymentRecord).toHaveBeenCalledWith(validSubject);
    expect(mockConsoleLog).toHaveBeenCalledWith(
      expect.stringContaining(`Fetching repayment history for ${validSubject}`),
    );
    expect(mockConsoleLog).toHaveBeenCalledWith(
      expect.stringContaining("Repayment History"),
    );
    expect(mockConsoleLog).toHaveBeenCalledWith(
      expect.stringContaining("9000"), // repaymentRate bps: 9/10 * 10000
    );
    expect(mockConsoleError).not.toHaveBeenCalled();
  });

  it("calculates repaymentRateBps as 0 when totalCount is 0", async () => {
    const mockRecord = {
      onTimeCount: 0,
      totalCount: 0,
      totalRepaid: 0n,
    };
    mockGetRepaymentRecord.mockResolvedValue(mockRecord);

    await program.parseAsync(["node", "stellar-did", "repayment", "--json", validSubject]);

    const loggedOutput = mockConsoleLog.mock.calls.find(
      ([arg]) => typeof arg === "string" && arg.includes("repaymentRateBps"),
    );
    expect(loggedOutput).toBeDefined();
    const parsed = JSON.parse(loggedOutput![0] as string);
    expect(parsed.repaymentRateBps).toBe(0);
    expect(mockProcessExit).toHaveBeenCalledWith(0);
  });

  it("handles errors from sdk.getRepaymentRecord gracefully and exits 1", async () => {
    mockGetRepaymentRecord.mockRejectedValue(new Error("RPC network timeout"));

    await program.parseAsync(["node", "stellar-did", "repayment", validSubject]);

    expect(mockGetRepaymentRecord).toHaveBeenCalledWith(validSubject);
    expect(mockConsoleError).toHaveBeenCalledWith("Failed:", "RPC network timeout");
    expect(mockProcessExit).toHaveBeenCalledWith(1);
  });
});
