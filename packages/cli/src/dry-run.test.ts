import { program } from "./index";
import { StellarDIDCreditSDK } from "@stellar-did-credit/sdk";
import { Keypair, nativeToScVal } from "@stellar/stellar-sdk";

const mockSimulateTransaction = jest.fn();
const mockGetAccount = jest.fn();

jest.mock("@stellar/stellar-sdk", () => {
  const actual = jest.requireActual("@stellar/stellar-sdk");
  return {
    ...actual,
    SorobanRpc: {
      ...actual.SorobanRpc,
      Server: jest.fn().mockImplementation(() => ({
        getAccount: mockGetAccount,
        simulateTransaction: mockSimulateTransaction,
      })),
    },
  };
});

jest.mock("@stellar-did-credit/sdk", () => {
  const actual = jest.requireActual("@stellar-did-credit/sdk");
  return {
    ...actual,
    StellarDIDCreditSDK: jest.fn(),
  };
});

jest.mock("./config", () => ({
  loadConfig: jest.fn(() => ({
    rpcUrl: "https://soroban-testnet.stellar.org",
    networkPassphrase: "Test SDF Network ; September 2015",
    identityOracleId: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM",
    creditOracleId: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM",
    governanceId: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM",
    simAccount: "GCZRUROFMVTL55BZJJBGIB3S366QKQMADFJG6SXCSJRTJNUQMCCR32IO",
  })),
  validateConfig: jest.fn(),
}));

describe("CLI --dry-run option", () => {
  let originalConsoleLog: typeof console.log;
  let originalConsoleError: typeof console.error;
  let originalProcessExit: typeof process.exit;
  let mockConsoleLog: jest.Mock;
  let mockConsoleError: jest.Mock;
  let mockProcessExit: jest.Mock;

  const mockAnchorDID = jest.fn();
  const mockComputeScore = jest.fn();
  const mockIssueVC = jest.fn();
  const mockCreateProposal = jest.fn();
  const mockExecute = jest.fn();
  const mockApplyWeights = jest.fn();

  const secret = Keypair.random().secret();
  const subject = Keypair.random().publicKey();
  const vcHash = "a".repeat(64);

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

    mockGetAccount.mockResolvedValue({
      sequenceNumber: () => "100",
    });

    mockSimulateTransaction.mockResolvedValue({
      transactionData: {},
      minResourceFee: "2500",
      cost: { cpuInsns: "12345", memBytes: "6789" },
      result: { retval: nativeToScVal(720) },
    });

    (StellarDIDCreditSDK as unknown as jest.Mock).mockImplementation(() => ({
      anchorDID: mockAnchorDID,
      computeScore: mockComputeScore,
      issueVC: mockIssueVC,
      governance: {
        createProposal: mockCreateProposal,
        execute: mockExecute,
        applyWeights: mockApplyWeights,
      },
    }));
  });

  afterEach(() => {
    console.log = originalConsoleLog;
    console.error = originalConsoleError;
    process.exit = originalProcessExit;
    jest.clearAllMocks();
  });

  it("anchor-did --dry-run simulates and prints gas cost without broadcasting", async () => {
    await program.parseAsync([
      "node",
      "stellar-did",
      "anchor-did",
      secret,
      "QmTestCid123",
      "--dry-run",
    ]);

    expect(mockSimulateTransaction).toHaveBeenCalledTimes(1);
    expect(mockAnchorDID).not.toHaveBeenCalled();
    expect(mockProcessExit).toHaveBeenCalledWith(0);

    const logOutput = mockConsoleLog.mock.calls.map((c) => c.join(" ")).join("\n");
    expect(logOutput).toContain("Simulating anchor-did (--dry-run)");
    expect(logOutput).toContain("2500 stroops");
  });

  it("anchor-vc --dry-run simulates and prints gas cost without broadcasting", async () => {
    await program.parseAsync([
      "node",
      "stellar-did",
      "anchor-vc",
      secret,
      subject,
      vcHash,
      "--dry-run",
    ]);

    expect(mockSimulateTransaction).toHaveBeenCalledTimes(1);
    expect(mockIssueVC).not.toHaveBeenCalled();
    expect(mockProcessExit).toHaveBeenCalledWith(0);

    const logOutput = mockConsoleLog.mock.calls.map((c) => c.join(" ")).join("\n");
    expect(logOutput).toContain("Simulating anchor-vc (--dry-run)");
    expect(logOutput).toContain("2500 stroops");
  });

  it("compute-score --dry-run simulates and prints gas cost without broadcasting", async () => {
    await program.parseAsync([
      "node",
      "stellar-did",
      "compute-score",
      "--dry-run",
      secret,
      subject,
    ]);

    expect(mockSimulateTransaction).toHaveBeenCalledTimes(1);
    expect(mockComputeScore).not.toHaveBeenCalled();
    expect(mockProcessExit).toHaveBeenCalledWith(0);

    const logOutput = mockConsoleLog.mock.calls.map((c) => c.join(" ")).join("\n");
    expect(logOutput).toContain("Simulating compute-score (--dry-run)");
    expect(logOutput).toContain("2500 stroops");
  });

  it("governance create-proposal --dry-run simulates without broadcasting", async () => {
    await program.parseAsync([
      "node",
      "stellar-did",
      "governance",
      "create-proposal",
      secret,
      "40",
      "30",
      "30",
      "--dry-run",
    ]);

    expect(mockSimulateTransaction).toHaveBeenCalledTimes(1);
    expect(mockCreateProposal).not.toHaveBeenCalled();
    expect(mockProcessExit).toHaveBeenCalledWith(0);

    const logOutput = mockConsoleLog.mock.calls.map((c) => c.join(" ")).join("\n");
    expect(logOutput).toContain("Simulating governance create-proposal (--dry-run)");
    expect(logOutput).toContain("2500 stroops");
  });

  it("governance execute --dry-run simulates without broadcasting", async () => {
    await program.parseAsync([
      "node",
      "stellar-did",
      "governance",
      "execute",
      secret,
      "1",
      "--dry-run",
    ]);

    expect(mockSimulateTransaction).toHaveBeenCalledTimes(1);
    expect(mockExecute).not.toHaveBeenCalled();
    expect(mockProcessExit).toHaveBeenCalledWith(0);

    const logOutput = mockConsoleLog.mock.calls.map((c) => c.join(" ")).join("\n");
    expect(logOutput).toContain("Simulating governance execute (--dry-run)");
    expect(logOutput).toContain("2500 stroops");
  });

  it("governance apply-weights --dry-run simulates without broadcasting", async () => {
    await program.parseAsync([
      "node",
      "stellar-did",
      "governance",
      "apply-weights",
      secret,
      "--dry-run",
    ]);

    expect(mockSimulateTransaction).toHaveBeenCalledTimes(1);
    expect(mockApplyWeights).not.toHaveBeenCalled();
    expect(mockProcessExit).toHaveBeenCalledWith(0);

    const logOutput = mockConsoleLog.mock.calls.map((c) => c.join(" ")).join("\n");
    expect(logOutput).toContain("Simulating governance apply-weights (--dry-run)");
    expect(logOutput).toContain("2500 stroops");
  });

  it("exits 1 on simulation error", async () => {
    mockSimulateTransaction.mockResolvedValue({
      error: "HostError: Error(Contract, #1)",
    });

    await program.parseAsync([
      "node",
      "stellar-did",
      "compute-score",
      "--dry-run",
      secret,
      subject,
    ]);

    expect(mockProcessExit).toHaveBeenCalledWith(1);
    const errOutput = mockConsoleError.mock.calls.map((c) => c.join(" ")).join("\n");
    expect(errOutput).toContain("Simulation Failed");
    expect(errOutput).toContain("Error(Contract, #1)");
  });
});
