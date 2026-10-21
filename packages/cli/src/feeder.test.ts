import { program } from "./index";
import { Keypair } from "@stellar/stellar-sdk";

// The WASM prover is irrelevant to these tests and may not be built locally.
jest.mock("@stellar-did-credit/zk-wasm", () => ({}), { virtual: true });

jest.mock("@stellar-did-credit/feeder", () => {
  const start = jest.fn(() => jest.fn());
  const runCycle = jest.fn().mockResolvedValue(undefined);
  const Feeder = jest.fn().mockImplementation(() => ({ start, runCycle }));
  return {
    __esModule: true,
    Feeder,
    NETWORK_CONFIGS: {
      testnet: {
        networkPassphrase: "Test SDF Network ; September 2015",
        rpcUrl: "https://soroban-testnet.stellar.org",
        horizonUrl: "https://horizon-testnet.stellar.org",
        simAccount: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
      },
      mainnet: {
        networkPassphrase: "Public Global Stellar Network ; September 2015",
        rpcUrl: "https://soroban-rpc.mainnet.stellarchain.io",
        horizonUrl: "https://horizon.stellar.org",
        simAccount: "",
      },
      futurenet: {
        networkPassphrase: "Test SDF Future Network ; October 2022",
        rpcUrl: "https://rpc-futurenet.stellar.org",
        horizonUrl: "https://horizon-futurenet.stellar.org",
        simAccount: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
      },
    },
    __mocks: { start, runCycle },
  };
});

jest.mock("./config", () => ({
  loadConfig: jest.fn(),
  validateConfig: jest.fn(),
}));

type SignalListener = (signal: NodeJS.Signals) => void;

const feederMock = jest.requireMock("@stellar-did-credit/feeder") as {
  Feeder: jest.Mock;
  __mocks: { start: jest.Mock; runCycle: jest.Mock };
};
const configMock = jest.requireMock("./config") as {
  loadConfig: jest.Mock;
  validateConfig: jest.Mock;
};

const FeederCtor = feederMock.Feeder;
const { start: mockStart, runCycle: mockRunCycle } = feederMock.__mocks;

describe("feeder commands", () => {
  const subjectA = Keypair.random().publicKey();
  const subjectB = Keypair.random().publicKey();
  const secret = Keypair.random().secret();

  const baseConfig = {
    rpcUrl: "https://soroban-testnet.stellar.org",
    networkPassphrase: "Test SDF Network ; September 2015",
    identityOracleId: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM",
    creditOracleId: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM",
    revocationRegistryId: "",
    simAccount: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
    maxRetries: 3,
    subjects: [subjectA],
  };

  let originalFeederSecret: string | undefined;
  let sigintBefore: SignalListener[];
  let sigtermBefore: SignalListener[];

  beforeEach(() => {
    originalFeederSecret = process.env["FEEDER_SECRET"];
    process.env["FEEDER_SECRET"] = secret;
    delete process.env["HORIZON_URL"];
    delete process.env["POLL_INTERVAL_MS"];

    sigintBefore = process.listeners("SIGINT") as unknown as SignalListener[];
    sigtermBefore = process.listeners("SIGTERM") as unknown as SignalListener[];

    configMock.loadConfig.mockReturnValue({ ...baseConfig });
    FeederCtor.mockClear();
    mockStart.mockClear();
    mockRunCycle.mockClear();
    // Commander keeps option values between parseAsync calls on the same
    // Command instance, so reset --subjects before each test.
    for (const name of ["start", "run-cycle"]) {
      program.commands
        .find((c) => c.name() === "feeder")
        ?.commands.find((c) => c.name() === name)
        ?.setOptionValue("subjects", undefined);
    }
    jest.spyOn(console, "log").mockImplementation(() => undefined);
    jest.spyOn(console, "error").mockImplementation(() => undefined);
    jest
      .spyOn(process, "exit")
      .mockImplementation(((code?: number) => {
        throw new Error(`process.exit(${code})`);
      }) as never);
  });

  afterEach(() => {
    for (const listener of process.listeners(
      "SIGINT",
    ) as unknown as SignalListener[]) {
      if (!sigintBefore.includes(listener)) {
        process.removeListener("SIGINT", listener);
      }
    }
    for (const listener of process.listeners(
      "SIGTERM",
    ) as unknown as SignalListener[]) {
      if (!sigtermBefore.includes(listener)) {
        process.removeListener("SIGTERM", listener);
      }
    }

    if (originalFeederSecret === undefined) {
      delete process.env["FEEDER_SECRET"];
    } else {
      process.env["FEEDER_SECRET"] = originalFeederSecret;
    }

    jest.restoreAllMocks();
    jest.clearAllMocks();
  });

  const feederCommand = () =>
    program.commands.find((c) => c.name() === "feeder")!;

  it("registers start and run-cycle with --subjects", () => {
    const help = feederCommand().helpInformation();
    expect(help).toContain("start");
    expect(help).toContain("run-cycle");

    for (const name of ["start", "run-cycle"]) {
      const cmd = feederCommand().commands.find((c) => c.name() === name)!;
      expect(cmd.helpInformation()).toContain("--subjects");
    }
  });

  it("run-cycle builds the feeder from config and runs one cycle", async () => {
    await program.parseAsync(["node", "stellar-did", "feeder", "run-cycle"]);

    expect(FeederCtor).toHaveBeenCalledTimes(1);
    const [feederConfig] = FeederCtor.mock.calls[0];
    expect(feederConfig).toMatchObject({
      rpcUrl: baseConfig.rpcUrl,
      horizonUrl: "https://horizon-testnet.stellar.org",
      creditOracleId: baseConfig.creditOracleId,
      identityOracleId: baseConfig.identityOracleId,
      simAccount: baseConfig.simAccount,
      subjects: [subjectA],
      pollIntervalMs: 3_600_000,
      maxRetries: 3,
    });
    expect(mockRunCycle).toHaveBeenCalledTimes(1);
    expect(mockStart).not.toHaveBeenCalled();
  });

  it("--subjects overrides the subjects from the config file", async () => {
    await program.parseAsync([
      "node",
      "stellar-did",
      "feeder",
      "run-cycle",
      "--subjects",
      ` ${subjectB} , ${subjectA} `,
    ]);

    const [feederConfig] = FeederCtor.mock.calls[0];
    expect(feederConfig.subjects).toEqual([subjectB, subjectA]);
  });

  it("honours HORIZON_URL and POLL_INTERVAL_MS overrides", async () => {
    process.env["HORIZON_URL"] = "https://horizon.example.org";
    process.env["POLL_INTERVAL_MS"] = "60000";

    await program.parseAsync(["node", "stellar-did", "feeder", "run-cycle"]);

    const [feederConfig] = FeederCtor.mock.calls[0];
    expect(feederConfig.horizonUrl).toBe("https://horizon.example.org");
    expect(feederConfig.pollIntervalMs).toBe(60_000);
  });

  it("start begins the polling loop", async () => {
    await program.parseAsync(["node", "stellar-did", "feeder", "start"]);

    expect(FeederCtor).toHaveBeenCalledTimes(1);
    expect(mockStart).toHaveBeenCalledTimes(1);
    expect(mockRunCycle).not.toHaveBeenCalled();
  });

  it("fails when no subjects are configured anywhere", async () => {
    configMock.loadConfig.mockReturnValue({ ...baseConfig, subjects: undefined });

    await expect(
      program.parseAsync(["node", "stellar-did", "feeder", "run-cycle"]),
    ).rejects.toThrow("process.exit(1)");
    expect(FeederCtor).not.toHaveBeenCalled();
  });

  it("fails when FEEDER_SECRET is missing", async () => {
    delete process.env["FEEDER_SECRET"];

    await expect(
      program.parseAsync(["node", "stellar-did", "feeder", "run-cycle"]),
    ).rejects.toThrow("process.exit(1)");
    expect(FeederCtor).not.toHaveBeenCalled();
  });

  it("rejects an invalid subject address", async () => {
    await expect(
      program.parseAsync([
        "node",
        "stellar-did",
        "feeder",
        "run-cycle",
        "--subjects",
        "not-an-address",
      ]),
    ).rejects.toThrow("process.exit(1)");
    expect(FeederCtor).not.toHaveBeenCalled();
  });

  it("rejects an invalid POLL_INTERVAL_MS", async () => {
    process.env["POLL_INTERVAL_MS"] = "abc";

    await expect(
      program.parseAsync(["node", "stellar-did", "feeder", "run-cycle"]),
    ).rejects.toThrow("process.exit(1)");
    expect(FeederCtor).not.toHaveBeenCalled();
  });
});
