import { program } from "./index";
import { StellarDIDCreditSDK } from "@stellar-did-credit/sdk";
import { validateConfig, loadConfig } from "./config";

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

describe("CLI recency-decay command", () => {
  let originalConsoleLog: typeof console.log;
  let originalConsoleError: typeof console.error;
  let originalProcessExit: typeof process.exit;
  let mockConsoleLog: jest.Mock;
  let mockConsoleError: jest.Mock;
  let mockProcessExit: jest.Mock;
  let mockGetRecencyDecayConfig: jest.Mock;

  const mockDecayConfig = {
    enabled: true,
    decayBpsPerDay: 5,
    minRecencyBps: 5000,
  };

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

    mockGetRecencyDecayConfig = jest.fn();
    (StellarDIDCreditSDK as unknown as jest.Mock).mockImplementation(() => ({
      getRecencyDecayConfig: mockGetRecencyDecayConfig,
    }));

    const recencyDecayCmd = program.commands.find((c) => c.name() === "recency-decay");
    if (recencyDecayCmd) {
      recencyDecayCmd.setOptionValue("json", undefined);
    }
  });

  afterEach(() => {
    console.log = originalConsoleLog;
    console.error = originalConsoleError;
    process.exit = originalProcessExit;
    jest.clearAllMocks();
  });

  it("fetches and displays recency decay configuration in human-readable table", async () => {
    mockGetRecencyDecayConfig.mockResolvedValue(mockDecayConfig);

    await program.parseAsync(["node", "stellar-did", "recency-decay"]);

    expect(loadConfig).toHaveBeenCalledWith("testnet");
    expect(validateConfig).toHaveBeenCalledWith(
      expect.objectContaining({
        creditOracleId: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM",
      }),
      ["creditOracleId"],
      true,
    );
    expect(mockGetRecencyDecayConfig).toHaveBeenCalled();
    expect(mockConsoleLog).toHaveBeenCalledWith(
      expect.stringContaining("Fetching recency decay configuration on testnet..."),
    );
    expect(mockConsoleLog).toHaveBeenCalledWith(
      expect.stringContaining("Enabled:                      true"),
    );
    expect(mockConsoleLog).toHaveBeenCalledWith(
      expect.stringContaining("decay_bps_per_day:               5"),
    );
    expect(mockConsoleLog).toHaveBeenCalledWith(
      expect.stringContaining("min_recency_bps:              5000"),
    );
    expect(mockConsoleError).not.toHaveBeenCalled();
  });

  it("outputs JSON with all 3 configuration fields and exits 0 when --json is provided", async () => {
    mockGetRecencyDecayConfig.mockResolvedValue(mockDecayConfig);

    await program.parseAsync(["node", "stellar-did", "recency-decay", "--json"]);

    expect(mockGetRecencyDecayConfig).toHaveBeenCalled();
    expect(mockConsoleLog).not.toHaveBeenCalledWith(
      expect.stringContaining("Fetching recency decay configuration"),
    );
    expect(mockConsoleLog).toHaveBeenCalledWith(
      JSON.stringify(
        {
          enabled: true,
          decay_bps_per_day: 5,
          min_recency_bps: 5000,
        },
        null,
        2,
      ),
    );
    expect(mockProcessExit).toHaveBeenCalledWith(0);
    expect(mockConsoleError).not.toHaveBeenCalled();
  });

  it("handles errors from sdk.getRecencyDecayConfig gracefully and exits 1", async () => {
    mockGetRecencyDecayConfig.mockRejectedValue(new Error("Simulation failed"));

    await program.parseAsync(["node", "stellar-did", "recency-decay"]);

    expect(mockGetRecencyDecayConfig).toHaveBeenCalled();
    expect(mockConsoleError).toHaveBeenCalledWith("Failed:", "Simulation failed");
    expect(mockProcessExit).toHaveBeenCalledWith(1);
  });
});
