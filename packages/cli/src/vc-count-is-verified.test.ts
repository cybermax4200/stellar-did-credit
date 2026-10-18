import { program } from "./index";
import { StellarDIDCreditSDK } from "@stellar-did-credit/sdk";

jest.mock("@stellar-did-credit/sdk");
jest.mock("./config", () => ({
  loadConfig: jest.fn(() => ({
    rpcUrl: "https://soroban-testnet.stellar.org",
    networkPassphrase: "Test SDF Network ; September 2015",
    identityOracleId: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM",
    creditOracleId: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM",
    simAccount: "GCZRUROFMVTL55BZJJBGIB3S366QKQMADFJG6SXCSJRTJNUQMCCR32IO",
  })),
  validateConfig: jest.fn(),
}));

describe.each([
  {
    command: "vc-count",
    sdkMethod: "getVCCount",
    jsonKey: "vcCount",
    result: 3,
    banner: "Fetching active VC count for",
    humanOutput: "Active VC count: 3",
  },
  {
    command: "is-verified",
    sdkMethod: "isVerified",
    jsonKey: "isVerified",
    result: true,
    banner: "Checking verification status for",
    humanOutput: "✅ Subject is VERIFIED.",
  },
])("$command command", ({ command, sdkMethod, jsonKey, result, banner, humanOutput }) => {
  let originalConsoleLog: typeof console.log;
  let originalConsoleError: typeof console.error;
  let originalProcessExit: typeof process.exit;
  let mockConsoleLog: jest.Mock;
  let mockConsoleError: jest.Mock;
  let mockProcessExit: jest.Mock;
  let mockSdkMethod: jest.Mock;

  const subjectAddress =
    "GCZRUROFMVTL55BZJJBGIB3S366QKQMADFJG6SXCSJRTJNUQMCCR32IO";

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

    mockSdkMethod = jest.fn().mockResolvedValue(result);
    (StellarDIDCreditSDK as unknown as jest.Mock).mockImplementation(() => ({
      [sdkMethod]: mockSdkMethod,
    }));

    const commandInstance = program.commands.find(
      (registeredCommand) => registeredCommand.name() === command,
    );
    commandInstance?.setOptionValue("json", undefined);
  });

  afterEach(() => {
    console.log = originalConsoleLog;
    console.error = originalConsoleError;
    process.exit = originalProcessExit;
    jest.clearAllMocks();
  });

  it("outputs the expected JSON and exits successfully with --json", async () => {
    await program.parseAsync([
      "node",
      "stellar-did",
      command,
      "--json",
      subjectAddress,
    ]);

    expect(mockSdkMethod).toHaveBeenCalledWith(subjectAddress);
    expect(mockConsoleLog).toHaveBeenCalledTimes(1);
    expect(mockConsoleLog).toHaveBeenCalledWith(
      JSON.stringify({ [jsonKey]: result }),
    );
    expect(mockConsoleLog).not.toHaveBeenCalledWith(
      expect.stringContaining(banner),
    );
    expect(mockProcessExit).toHaveBeenCalledWith(0);
    expect(mockConsoleError).not.toHaveBeenCalled();
  });

  it("preserves human-readable output without --json", async () => {
    await program.parseAsync(["node", "stellar-did", command, subjectAddress]);

    expect(mockSdkMethod).toHaveBeenCalledWith(subjectAddress);
    expect(mockConsoleLog).toHaveBeenCalledWith(
      expect.stringContaining(banner),
    );
    expect(mockConsoleLog).toHaveBeenCalledWith(humanOutput);
    expect(mockConsoleLog).not.toHaveBeenCalledWith(
      JSON.stringify({ [jsonKey]: result }),
    );
    expect(mockProcessExit).not.toHaveBeenCalled();
    expect(mockConsoleError).not.toHaveBeenCalled();
  });
});
