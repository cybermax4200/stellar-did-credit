import { program } from "./index";
import { StellarDIDCreditSDK } from "@stellar-did-credit/sdk";

// The WASM prover is irrelevant to these tests and may not be built locally.
jest.mock("@stellar-did-credit/zk-wasm", () => ({}), { virtual: true });

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
  })),
  validateConfig: jest.fn(),
}));

describe("governance list CLI command", () => {
  const mockListProposals = jest.fn();

  let logSpy: jest.SpyInstance;
  let errorSpy: jest.SpyInstance;
  let exitSpy: jest.SpyInstance;

  beforeEach(() => {
    logSpy = jest.spyOn(console, "log").mockImplementation(() => undefined);
    errorSpy = jest.spyOn(console, "error").mockImplementation(() => undefined);
    exitSpy = jest
      .spyOn(process, "exit")
      .mockImplementation(((code?: number) => {
        throw new Error(`process.exit(${code})`);
      }) as never);

    (StellarDIDCreditSDK as unknown as jest.Mock).mockImplementation(() => ({
      governance: {
        listProposals: mockListProposals,
      },
    }));
  });

  afterEach(() => {
    jest.restoreAllMocks();
  });

  it("lists all proposals when called without options", async () => {
    mockListProposals.mockResolvedValueOnce([
      {
        id: 1n,
        proposer: "GBUQWP3BOUZX34ULNQG23RQ6F4YUSXHTQSXE7XDZT4A65XJLQRGEZSM",
        proposedWeights: {
          vcWeight: 40,
          txWeight: 30,
          repaymentWeight: 30,
        },
        votesFor: 100n,
        votesAgainst: 10n,
        expiryLedger: 1000,
        executionDelayLedgers: 100,
        executed: false,
        cancelled: false,
        quorumRequired: 50n,
      },
      {
        id: 2n,
        proposer: "GBUQWP3BOUZX34ULNQG23RQ6F4YUSXHTQSXE7XDZT4A65XJLQRGEZSM",
        proposedWeights: {
          vcWeight: 50,
          txWeight: 25,
          repaymentWeight: 25,
        },
        votesFor: 200n,
        votesAgainst: 0n,
        expiryLedger: 2000,
        executionDelayLedgers: 50,
        executed: true,
        cancelled: false,
        quorumRequired: 50n,
      },
    ]);

    await program.parseAsync(["node", "cli.js", "governance", "list"]);

    expect(mockListProposals).toHaveBeenCalledWith(undefined, undefined);
    expect(logSpy).toHaveBeenCalledWith(expect.stringContaining("Proposal #1 [ACTIVE]"));
    expect(logSpy).toHaveBeenCalledWith(expect.stringContaining("Proposal #2 [EXECUTED]"));
  });

  it("passes --from and --limit options to sdk.governance.listProposals", async () => {
    mockListProposals.mockResolvedValueOnce([]);

    await program.parseAsync([
      "node",
      "cli.js",
      "governance",
      "list",
      "--from",
      "2",
      "--limit",
      "5",
    ]);

    expect(mockListProposals).toHaveBeenCalledWith(2n, 5);
    expect(logSpy).toHaveBeenCalledWith("No proposals found.");
  });

  it("handles errors gracefully and exits with code 1", async () => {
    mockListProposals.mockRejectedValueOnce(new Error("RPC timeout"));

    await expect(
      program.parseAsync(["node", "cli.js", "governance", "list"]),
    ).rejects.toThrow("process.exit(1)");

    expect(errorSpy).toHaveBeenCalledWith("Failed:", "RPC timeout");
    expect(exitSpy).toHaveBeenCalledWith(1);
  });
});
