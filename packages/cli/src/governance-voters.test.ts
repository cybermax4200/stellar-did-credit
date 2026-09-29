import { program } from "./index";
import { StellarDIDCreditSDK } from "@stellar-did-credit/sdk";
import { Keypair } from "@stellar/stellar-sdk";

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

describe("governance voter admin commands", () => {
  const mockRegisterVoter = jest.fn();
  const mockUpdateVoterWeight = jest.fn();
  const secret = Keypair.random().secret();
  const voter = Keypair.random().publicKey();
  const txHash = "b".repeat(64);

  let logSpy: jest.SpyInstance;
  let errorSpy: jest.SpyInstance;
  let exitSpy: jest.SpyInstance;
  let stderrSpy: jest.SpyInstance;

  beforeEach(() => {
    logSpy = jest.spyOn(console, "log").mockImplementation(() => undefined);
    errorSpy = jest.spyOn(console, "error").mockImplementation(() => undefined);
    stderrSpy = jest.spyOn(process.stderr, "write").mockImplementation(() => true);
    exitSpy = jest
      .spyOn(process, "exit")
      .mockImplementation(((code?: number) => {
        throw new Error(`process.exit(${code})`);
      }) as never);

    mockRegisterVoter.mockResolvedValue(txHash);
    mockUpdateVoterWeight.mockResolvedValue(txHash);
    (StellarDIDCreditSDK as unknown as jest.Mock).mockImplementation(() => ({
      governance: {
        registerVoter: mockRegisterVoter,
        updateVoterWeight: mockUpdateVoterWeight,
      },
    }));
  });

  afterEach(() => {
    jest.restoreAllMocks();
    jest.clearAllMocks();
  });

  const governanceCommand = () =>
    program.commands.find((c) => c.name() === "governance")!;

  it("lists both commands under governance --help", () => {
    const help = governanceCommand().helpInformation();
    expect(help).toContain("register-voter");
    expect(help).toContain("update-voter-weight");
  });

  it("documents arguments in register-voter --help", () => {
    const cmd = governanceCommand().commands.find((c) => c.name() === "register-voter")!;
    const help = cmd.helpInformation();
    expect(help).toContain("<admin-secret>");
    expect(help).toContain("<voter-address>");
    expect(help).toContain("<weight>");
    expect(help).toContain("--dry-run");
  });

  it("documents weight = 0 deregistration in update-voter-weight --help", () => {
    const cmd = governanceCommand().commands.find((c) => c.name() === "update-voter-weight")!;
    expect(cmd.helpInformation()).toContain("0");
    expect(cmd.description()).toContain("deregister");
  });

  it("register-voter submits and prints tx hash and explorer link", async () => {
    await program.parseAsync([
      "node", "stellar-did", "governance", "register-voter", secret, voter, "5",
    ]);

    expect(mockRegisterVoter).toHaveBeenCalledWith(expect.anything(), voter, 5n);
    const out = logSpy.mock.calls.map((c) => c.join(" ")).join("\n");
    expect(out).toContain(txHash);
    expect(out).toContain(`https://stellar.expert/explorer/testnet/tx/${txHash}`);
  });

  it("update-voter-weight accepts weight 0 to deregister", async () => {
    await program.parseAsync([
      "node", "stellar-did", "governance", "update-voter-weight", secret, voter, "0",
    ]);

    expect(mockUpdateVoterWeight).toHaveBeenCalledWith(expect.anything(), voter, 0n);
    const out = logSpy.mock.calls.map((c) => c.join(" ")).join("\n");
    expect(out).toContain(txHash);
    expect(out).toContain("Explorer:");
  });

  it.each(["abc", "-1", "1.5", ""])(
    "register-voter rejects invalid weight %p",
    async (weight) => {
      await expect(
        program.parseAsync([
          "node", "stellar-did", "governance", "register-voter", secret, voter, weight,
        ]),
      ).rejects.toThrow();
      expect(mockRegisterVoter).not.toHaveBeenCalled();
    },
  );

  it("register-voter rejects weight 0", async () => {
    await expect(
      program.parseAsync([
        "node", "stellar-did", "governance", "register-voter", secret, voter, "0",
      ]),
    ).rejects.toThrow();
    expect(mockRegisterVoter).not.toHaveBeenCalled();
  });

  it("update-voter-weight rejects negative weight", async () => {
    await expect(
      program.parseAsync([
        "node", "stellar-did", "governance", "update-voter-weight", secret, voter, "-3",
      ]),
    ).rejects.toThrow();
    expect(mockUpdateVoterWeight).not.toHaveBeenCalled();
  });

  it("rejects an invalid voter address", async () => {
    await expect(
      program.parseAsync([
        "node", "stellar-did", "governance", "register-voter", secret, "not-an-address", "5",
      ]),
    ).rejects.toThrow("process.exit(1)");
    expect(mockRegisterVoter).not.toHaveBeenCalled();
    expect(errorSpy).toHaveBeenCalled();
    expect(exitSpy).toHaveBeenCalledWith(1);
    expect(stderrSpy).not.toBeUndefined();
  });
});
