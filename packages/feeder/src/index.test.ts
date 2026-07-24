import { Feeder, getScore } from "./index";
import type { FeederConfig } from "./index";
import type { Keypair } from "@stellar/stellar-sdk";
import { SorobanRpc, scValToNative } from "@stellar/stellar-sdk";

jest.mock("@stellar/stellar-sdk", () => ({
  SorobanRpc: {
    Server: jest.fn().mockImplementation(() => ({})),
    Api: {
      isSimulationError: jest.fn(),
      isSimulationSuccess: jest.fn(),
    },
  },
  Contract: jest.fn().mockImplementation(() => ({ call: jest.fn() })),
  TransactionBuilder: jest.fn().mockImplementation(() => ({
    addOperation: jest.fn().mockReturnThis(),
    setTimeout: jest.fn().mockReturnThis(),
    build: jest.fn(),
  })),
  BASE_FEE: "100",
  Account: jest.fn(),
  scValToNative: jest.fn(),
  nativeToScVal: jest.fn(),
  Address: jest.fn().mockImplementation(() => ({ toScVal: jest.fn() })),
  xdr: {
    ScValType: { scvVoid: jest.fn(() => "scvVoid") },
  },
  Keypair: {},
  Horizon: { Server: jest.fn() },
}));

jest.mock("@stellar/stellar-sdk/rpc", () => ({
  assembleTransaction: jest.fn(),
}));

function deferred<T>(): {
  promise: Promise<T>;
  resolve: (value: T) => void;
} {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

const config: FeederConfig = {
  rpcUrl: "https://rpc.example",
  horizonUrl: "https://horizon.example",
  networkPassphrase: "Test SDF Network ; September 2015",
  creditOracleId: "CCREDIT",
  identityOracleId: "CIDENTITY",
  simAccount: "GSIM",
  subjects: ["GSUBJECT1", "GSUBJECT2"],
  pollIntervalMs: 999_999,
};

describe("Feeder graceful shutdown", () => {
  it("stop() mid-cycle lets the in-progress subject finish but starts no new subject or cycle", async () => {
    const feeder = new Feeder(config, {} as Keypair);

    const firstSubjectSync = deferred<void>();
    const feedSubjectSpy = jest
      .spyOn(feeder, "feedSubject")
      .mockImplementationOnce(() => firstSubjectSync.promise)
      .mockImplementation(() => Promise.resolve());

    const setTimeoutSpy = jest.spyOn(global, "setTimeout");

    const stop = feeder.start();

    // Flush microtasks so runCycle() reaches feedSubject() for the first subject.
    await Promise.resolve();
    await Promise.resolve();

    expect(feedSubjectSpy).toHaveBeenCalledTimes(1);
    expect(feedSubjectSpy.mock.calls[0][0]).toBe("GSUBJECT1");
    const signalPassedToSubject = feedSubjectSpy.mock.calls[0][1] as
      | AbortSignal
      | undefined;
    expect(signalPassedToSubject?.aborted).toBe(false);

    // Call stop() while the first subject's sync is still in flight.
    stop();
    expect(signalPassedToSubject?.aborted).toBe(true);

    // The in-progress operation is allowed to complete...
    firstSubjectSync.resolve();
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();

    // ...but no new subject was started, and no new cycle was scheduled.
    expect(feedSubjectSpy).toHaveBeenCalledTimes(1);
    expect(setTimeoutSpy).not.toHaveBeenCalled();

    setTimeoutSpy.mockRestore();
  });
});

describe("getScore", () => {
  const scoreConfig = {
    creditOracleId: "CCREDIT",
    networkPassphrase: "Test SDF Network ; September 2015",
    simAccount: "GSIM",
  };

  function fakeServer(simResult: unknown) {
    return {
      simulateTransaction: jest.fn().mockResolvedValue(simResult),
    } as unknown as SorobanRpc.Server;
  }

  afterEach(() => {
    jest.clearAllMocks();
  });

  it("returns null when the contract has no score computed yet (void return)", async () => {
    (SorobanRpc.Api.isSimulationError as unknown as jest.Mock).mockReturnValue(false);
    (SorobanRpc.Api.isSimulationSuccess as unknown as jest.Mock).mockReturnValue(true);
    const voidScVal = { switch: () => "scvVoid" };
    const server = fakeServer({ result: { retval: voidScVal } });

    const result = await getScore(server, scoreConfig, "GSUBJECT1");

    expect(result).toBeNull();
  });

  it("returns the parsed ScoreRecord when the contract returns Some(record)", async () => {
    (SorobanRpc.Api.isSimulationError as unknown as jest.Mock).mockReturnValue(false);
    (SorobanRpc.Api.isSimulationSuccess as unknown as jest.Mock).mockReturnValue(true);
    const recordScVal = { switch: () => "scvMap" };
    (scValToNative as jest.Mock).mockReturnValue({
      score: 720,
      last_updated: 1_700_000_000,
      vc_count: 4,
      repayment_rate: 9500,
      tx_volume_30d: 123456n,
    });
    const server = fakeServer({ result: { retval: recordScVal } });

    const result = await getScore(server, scoreConfig, "GSUBJECT1");

    expect(result).toEqual({
      score: 720,
      lastUpdated: 1_700_000_000,
      vcCount: 4,
      repaymentRate: 9500,
      txVolume30d: 123456n,
    });
  });

  it("throws when the simulation itself fails", async () => {
    (SorobanRpc.Api.isSimulationError as unknown as jest.Mock).mockReturnValue(true);
    const server = fakeServer({ error: "host unreachable" });

    await expect(getScore(server, scoreConfig, "GSUBJECT1")).rejects.toThrow(
      "get_score simulation failed: host unreachable",
    );
  });
});
