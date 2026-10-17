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

describe("CLI tx-stats command", () => {
  let originalConsoleLog: typeof console.log;
  let originalConsoleError: typeof console.error;
  let originalProcessExit: typeof process.exit;
  let mockConsoleLog: jest.Mock;
  let mockConsoleError: jest.Mock;
  let mockProcessExit: jest.Mock;
  let mockGetTxStats: jest.Mock;

  const validSubject =
    "GCZRUROFMVTL55BZJJBGIB3S366QKQMADFJG6SXCSJRTJNUQMCCR32IO";

  const mockStats = {
    volume30d: 1_000_000_000n,
    txCount30d: 42,
    avgCounterparties: 7,
  };

  const expectedJson = JSON.stringify(
    mockStats,
    (key, value) => (typeof value === "bigint" ? value.toString() : value),
    2,
  );

  /** Returns every printed table line (borders and rows) for alignment checks. */
  function printedTableLines(): string[] {
    return mockConsoleLog.mock.calls
      .map((call) => String(call[0] ?? ""))
      .filter((line) => /^[│┌├└]/.test(line));
  }

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

    mockGetTxStats = jest.fn();
    (StellarDIDCreditSDK as unknown as jest.Mock).mockImplementation(() => ({
      getTxStats: mockGetTxStats,
    }));

    const txStatsCmd = program.commands.find((c) => c.name() === "tx-stats");
    if (txStatsCmd) {
      txStatsCmd.setOptionValue("json", undefined);
    }
  });

  afterEach(() => {
    console.log = originalConsoleLog;
    console.error = originalConsoleError;
    process.exit = originalProcessExit;
    jest.clearAllMocks();
  });

  it("prints a human-readable table with volume, count and counterparties", async () => {
    mockGetTxStats.mockResolvedValue(mockStats);

    await program.parseAsync(["node", "stellar-did", "tx-stats", validSubject]);

    expect(mockGetTxStats).toHaveBeenCalledWith(validSubject);
    expect(mockConsoleLog).toHaveBeenCalledWith(
      expect.stringContaining(`Fetching transaction statistics for ${validSubject}`),
    );
    expect(mockConsoleLog).toHaveBeenCalledWith(
      expect.stringContaining("Transaction Statistics"),
    );
    expect(mockConsoleLog).toHaveBeenCalledWith(
      expect.stringContaining("Volume (30d):"),
    );
    expect(mockConsoleLog).toHaveBeenCalledWith(
      expect.stringContaining("100.0000000 XLM"),
    );
    expect(mockConsoleLog).toHaveBeenCalledWith(
      expect.stringContaining("TX Count (30d):"),
    );
    expect(mockConsoleLog).toHaveBeenCalledWith(
      expect.stringContaining("Avg Counterparties:"),
    );
    expect(mockConsoleError).not.toHaveBeenCalled();
    expect(mockProcessExit).not.toHaveBeenCalled();
  });

  it("keeps every line of the table the same width", async () => {
    mockGetTxStats.mockResolvedValue(mockStats);

    await program.parseAsync(["node", "stellar-did", "tx-stats", validSubject]);

    const lines = printedTableLines();
    expect(lines.length).toBeGreaterThanOrEqual(6);
    expect(new Set(lines.map((line) => [...line].length)).size).toBe(1);
  });

  it("outputs JSON containing volume30d, txCount30d and avgCounterparties with --json", async () => {
    mockGetTxStats.mockResolvedValue(mockStats);

    await program.parseAsync([
      "node",
      "stellar-did",
      "tx-stats",
      "--json",
      validSubject,
    ]);

    expect(mockGetTxStats).toHaveBeenCalledWith(validSubject);
    expect(mockConsoleLog).not.toHaveBeenCalledWith(
      expect.stringContaining(`Fetching transaction statistics for ${validSubject}`),
    );
    expect(mockConsoleLog).toHaveBeenCalledWith(expectedJson);
    expect(mockProcessExit).toHaveBeenCalledWith(0);

    const parsed = JSON.parse(expectedJson);
    expect(Object.keys(parsed).sort()).toEqual(
      ["avgCounterparties", "txCount30d", "volume30d"],
    );
    expect(mockConsoleError).not.toHaveBeenCalled();
  });

  it("prints a helpful message and exits 0 when no stats are stored", async () => {
    mockGetTxStats.mockResolvedValue(null);

    await program.parseAsync(["node", "stellar-did", "tx-stats", validSubject]);

    expect(mockConsoleLog).toHaveBeenCalledWith(
      "No transaction statistics have been submitted for this subject yet.",
    );
    expect(mockProcessExit).toHaveBeenCalledWith(0);
    expect(mockConsoleError).not.toHaveBeenCalled();
  });

  it("outputs JSON with txStats null and exits 0 when --json is passed and stats are missing", async () => {
    mockGetTxStats.mockResolvedValue(null);

    await program.parseAsync([
      "node",
      "stellar-did",
      "tx-stats",
      "--json",
      validSubject,
    ]);

    expect(mockConsoleLog).toHaveBeenCalledWith(JSON.stringify({ txStats: null }));
    expect(mockProcessExit).toHaveBeenCalledWith(0);
    expect(mockConsoleError).not.toHaveBeenCalled();
  });

  it("handles errors from sdk.getTxStats gracefully and exits 1", async () => {
    mockGetTxStats.mockRejectedValue(new Error("RPC network timeout"));

    await program.parseAsync(["node", "stellar-did", "tx-stats", validSubject]);

    expect(mockConsoleError).toHaveBeenCalledWith("Failed:", "RPC network timeout");
    expect(mockProcessExit).toHaveBeenCalledWith(1);
  });
});
