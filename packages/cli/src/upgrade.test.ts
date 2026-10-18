import { program } from "./index";
import { StellarDIDCreditSDK } from "@stellar-did-credit/sdk";
import { Keypair } from "@stellar/stellar-sdk";
import { validateConfig } from "./config";

const mockSimulateTransaction = jest.fn();
const mockGetAccount = jest.fn();

jest.mock("@stellar-did-credit/zk-wasm", () => ({}), { virtual: true });

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
    identityOracleId: "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
    creditOracleId: "CABAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAFNSZ",
    revocationRegistryId: "CABQGAYDAMBQGAYDAMBQGAYDAMBQGAYDAMBQGAYDAMBQGAYDAMBQGCK3",
    simAccount: "GCZRUROFMVTL55BZJJBGIB3S366QKQMADFJG6SXCSJRTJNUQMCCR32IO",
  })),
  validateConfig: jest.fn(),
}));

describe("CLI upgrade command group", () => {
  let originalConsoleLog: typeof console.log;
  let originalConsoleError: typeof console.error;
  let originalProcessExit: typeof process.exit;
  let mockConsoleLog: jest.Mock;
  let mockConsoleError: jest.Mock;
  let mockProcessExit: jest.Mock;

  let mockUpgradeIdentityOracle: jest.Mock;
  let mockUpgradeCreditOracle: jest.Mock;
  let mockUpgradeRevocationRegistry: jest.Mock;

  const adminKeypair = Keypair.random();
  const adminSecret = adminKeypair.secret();
  const adminPublic = adminKeypair.publicKey();

  const wasmHashHex = "ab".repeat(32);
  const wasmHashBuffer = Buffer.from(wasmHashHex, "hex");

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

    mockUpgradeIdentityOracle = jest.fn().mockResolvedValue("tx-identity");
    mockUpgradeCreditOracle = jest.fn().mockResolvedValue("tx-credit");
    mockUpgradeRevocationRegistry = jest
      .fn()
      .mockResolvedValue("tx-revocation");

    (StellarDIDCreditSDK as unknown as jest.Mock).mockImplementation(() => ({
      upgradeIdentityOracle: mockUpgradeIdentityOracle,
      upgradeCreditOracle: mockUpgradeCreditOracle,
      upgradeRevocationRegistry: mockUpgradeRevocationRegistry,
    }));

    // jest.clearAllMocks() in afterEach wipes the constructor implementation
    // installed by the module factory, so re-install it per test.
    (
      jest.requireMock("@stellar/stellar-sdk").SorobanRpc.Server as jest.Mock
    ).mockImplementation(() => ({
      getAccount: mockGetAccount,
      simulateTransaction: mockSimulateTransaction,
    }));

    mockGetAccount.mockResolvedValue({ sequenceNumber: () => "100" });
    mockSimulateTransaction.mockResolvedValue({
      transactionData: {},
      minResourceFee: "2500",
      cost: { cpuInsns: "12345", memBytes: "6789" },
      result: { retval: { value: null } },
    });

    // `program` is a module-level singleton, so option values set by one
    // parseAsync() call leak into the next test unless they are reset.
    const upgradeCmd = program.commands.find((c) => c.name() === "upgrade");
    if (upgradeCmd) {
      for (const sub of upgradeCmd.commands) {
        sub.setOptionValue("dryRun", undefined);
      }
    }
  });

  afterEach(() => {
    console.log = originalConsoleLog;
    console.error = originalConsoleError;
    process.exit = originalProcessExit;
    jest.clearAllMocks();
  });

  it("upgrade identity submits the wasm hash through the SDK", async () => {
    await program.parseAsync([
      "node",
      "stellar-did",
      "upgrade",
      "identity",
      adminSecret,
      wasmHashHex,
    ]);

    expect(validateConfig).toHaveBeenCalledWith(expect.anything(), [
      "identityOracleId",
    ]);
    expect(mockUpgradeIdentityOracle).toHaveBeenCalledTimes(1);
    const [identitySigner, identityHash] =
      mockUpgradeIdentityOracle.mock.calls[0];
    expect(identitySigner.publicKey()).toBe(adminPublic);
    expect(Buffer.isBuffer(identityHash)).toBe(true);
    expect(identityHash.equals(wasmHashBuffer)).toBe(true);
    expect(mockConsoleLog).toHaveBeenCalledWith(
      expect.stringContaining("Upgrading identity-oracle on testnet..."),
    );
    expect(mockConsoleLog).toHaveBeenCalledWith("  Transaction: tx-identity");
    expect(mockConsoleError).not.toHaveBeenCalled();
  });

  it("upgrade credit submits the wasm hash through the SDK", async () => {
    await program.parseAsync([
      "node",
      "stellar-did",
      "upgrade",
      "credit",
      adminSecret,
      wasmHashHex,
    ]);

    expect(validateConfig).toHaveBeenCalledWith(expect.anything(), [
      "creditOracleId",
    ]);
    expect(mockUpgradeCreditOracle).toHaveBeenCalledTimes(1);
    const [creditSigner, creditHash] = mockUpgradeCreditOracle.mock.calls[0];
    expect(creditSigner.publicKey()).toBe(adminPublic);
    expect(Buffer.isBuffer(creditHash)).toBe(true);
    expect(creditHash.equals(wasmHashBuffer)).toBe(true);
    expect(mockConsoleLog).toHaveBeenCalledWith("  Transaction: tx-credit");
    expect(mockConsoleError).not.toHaveBeenCalled();
  });

  it("upgrade revocation submits the wasm hash through the SDK", async () => {
    await program.parseAsync([
      "node",
      "stellar-did",
      "upgrade",
      "revocation",
      adminSecret,
      wasmHashHex,
    ]);

    expect(validateConfig).toHaveBeenCalledWith(expect.anything(), [
      "revocationRegistryId",
    ]);
    expect(mockUpgradeRevocationRegistry).toHaveBeenCalledTimes(1);
    const [revocationSigner, revocationHash] =
      mockUpgradeRevocationRegistry.mock.calls[0];
    expect(revocationSigner.publicKey()).toBe(adminPublic);
    expect(Buffer.isBuffer(revocationHash)).toBe(true);
    expect(revocationHash.equals(wasmHashBuffer)).toBe(true);
    expect(mockConsoleLog).toHaveBeenCalledWith(
      "  Transaction: tx-revocation",
    );
    expect(mockConsoleError).not.toHaveBeenCalled();
  });

  it("rejects a wasm hash that is not 64 hex characters", async () => {
    await program.parseAsync([
      "node",
      "stellar-did",
      "upgrade",
      "identity",
      adminSecret,
      "abcd",
    ]);

    expect(mockConsoleError).toHaveBeenCalledWith(
      "Error: wasm-hash must be a 64-character hex string (32 bytes).",
    );
    expect(mockProcessExit).toHaveBeenCalledWith(1);
    expect(mockUpgradeIdentityOracle).not.toHaveBeenCalled();
  });

  it("rejects a 64-character wasm hash containing non-hex characters", async () => {
    await program.parseAsync([
      "node",
      "stellar-did",
      "upgrade",
      "credit",
      adminSecret,
      "z".repeat(64),
    ]);

    expect(mockConsoleError).toHaveBeenCalledWith(
      "Error: wasm-hash must be a 64-character hex string (32 bytes).",
    );
    expect(mockProcessExit).toHaveBeenCalledWith(1);
    expect(mockUpgradeCreditOracle).not.toHaveBeenCalled();
  });

  it("upgrade identity --dry-run simulates without broadcasting", async () => {
    await program.parseAsync([
      "node",
      "stellar-did",
      "upgrade",
      "identity",
      adminSecret,
      wasmHashHex,
      "--dry-run",
    ]);

    expect(mockSimulateTransaction).toHaveBeenCalledTimes(1);
    expect(mockUpgradeIdentityOracle).not.toHaveBeenCalled();
    expect(mockProcessExit).toHaveBeenCalledWith(0);

    const logOutput = mockConsoleLog.mock.calls
      .map((c) => c.join(" "))
      .join("\n");
    expect(logOutput).toContain("Simulating upgrade identity-oracle (--dry-run)");
    expect(logOutput).toContain(
      "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
    );
  });

  it("upgrade credit --dry-run simulates against the credit-oracle contract", async () => {
    await program.parseAsync([
      "node",
      "stellar-did",
      "upgrade",
      "credit",
      adminSecret,
      wasmHashHex,
      "--dry-run",
    ]);

    expect(mockSimulateTransaction).toHaveBeenCalledTimes(1);
    expect(mockUpgradeCreditOracle).not.toHaveBeenCalled();
    expect(mockProcessExit).toHaveBeenCalledWith(0);

    const logOutput = mockConsoleLog.mock.calls
      .map((c) => c.join(" "))
      .join("\n");
    expect(logOutput).toContain("Simulating upgrade credit-oracle (--dry-run)");
    expect(logOutput).toContain(
      "CABAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAFNSZ",
    );
  });

  it("upgrade revocation --dry-run simulates against the revocation-registry contract", async () => {
    await program.parseAsync([
      "node",
      "stellar-did",
      "upgrade",
      "revocation",
      adminSecret,
      wasmHashHex,
      "--dry-run",
    ]);

    expect(mockSimulateTransaction).toHaveBeenCalledTimes(1);
    expect(mockUpgradeRevocationRegistry).not.toHaveBeenCalled();
    expect(mockProcessExit).toHaveBeenCalledWith(0);

    const logOutput = mockConsoleLog.mock.calls
      .map((c) => c.join(" "))
      .join("\n");
    expect(logOutput).toContain(
      "Simulating upgrade revocation-registry (--dry-run)",
    );
    expect(logOutput).toContain(
      "CABQGAYDAMBQGAYDAMBQGAYDAMBQGAYDAMBQGAYDAMBQGAYDAMBQGCK3",
    );
  });

  it("exits 1 when the upgrade transaction fails", async () => {
    mockUpgradeIdentityOracle.mockRejectedValue(
      new Error("NotAuthorized (code 2)"),
    );

    await program.parseAsync([
      "node",
      "stellar-did",
      "upgrade",
      "identity",
      adminSecret,
      wasmHashHex,
    ]);

    expect(mockConsoleError).toHaveBeenCalledWith(
      "Failed:",
      "NotAuthorized (code 2)",
    );
    expect(mockProcessExit).toHaveBeenCalledWith(1);
  });

  it("exits 1 when the dry-run simulation fails", async () => {
    mockSimulateTransaction.mockResolvedValue({
      error: "HostError: Error(Contract, #2)",
    });

    await program.parseAsync([
      "node",
      "stellar-did",
      "upgrade",
      "identity",
      adminSecret,
      wasmHashHex,
      "--dry-run",
    ]);

    expect(mockProcessExit).toHaveBeenCalledWith(1);
    const errOutput = mockConsoleError.mock.calls
      .map((c) => c.join(" "))
      .join("\n");
    expect(errOutput).toContain("Simulation Failed");
  });
});
