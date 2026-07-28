/* eslint-disable @typescript-eslint/no-explicit-any */
import { Feeder } from "./index";
import type { FeederConfig } from "./index";
import type { Keypair } from "@stellar/stellar-sdk";
import * as sdk from "@stellar/stellar-sdk";

// ---------------------------------------------------------------------------
// Shared mock instances so individual tests can reconfigure behaviour.
// ---------------------------------------------------------------------------

const mockServerInstance = {
  getAccount: jest.fn().mockResolvedValue({ sequence: "1" }),
  simulateTransaction: jest.fn().mockResolvedValue({
    result: { retval: { _value: 0 } },
  }),
  sendTransaction: jest.fn().mockResolvedValue({
    status: "PENDING",
    hash: "mock-tx-hash",
  }),
  getTransaction: jest.fn().mockResolvedValue({
    status: "SUCCESS",
  }),
};

const mockHorizonPaymentsCall = jest
  .fn()
  .mockResolvedValue({ records: [] });

const mockHorizonInstance = {
  payments: () => ({
    forAccount: () => ({
      order: () => ({
        limit: () => ({
          call: mockHorizonPaymentsCall,
        }),
      }),
    }),
  }),
};

jest.mock("@stellar/stellar-sdk", () => ({
  SorobanRpc: {
    Server: jest.fn().mockImplementation(() => mockServerInstance),
    Api: {
      isSimulationError: jest.fn(
        (sim: { error?: unknown }) => sim?.error !== undefined,
      ),
      isSimulationSuccess: jest.fn(
        (sim: { result?: unknown }) => sim?.result !== undefined,
      ),
    },
  },
  Contract: jest.fn().mockImplementation(() => ({
    call: jest.fn().mockReturnValue({}),
  })),
  TransactionBuilder: jest.fn().mockImplementation(() => ({
    addOperation: jest.fn().mockReturnThis(),
    setTimeout: jest.fn().mockReturnThis(),
    build: jest.fn().mockReturnValue({}),
  })),
  BASE_FEE: "100",
  Account: jest.fn(),
  scValToNative: jest.fn().mockReturnValue(3),
  nativeToScVal: jest.fn().mockReturnValue({}),
  Address: jest.fn().mockImplementation(() => ({
    toScVal: jest.fn().mockReturnValue({}),
  })),
  xdr: {
    ScVal: {
      scvMap: jest.fn().mockReturnValue({}),
      scvSymbol: jest.fn().mockReturnValue({}),
    },
    ScMapEntry: jest.fn(),
    Operation: {},
  },
  Keypair: {},
  Horizon: {
    Server: jest.fn().mockImplementation(() => mockHorizonInstance),
  },
}));

jest.mock("@stellar/stellar-sdk/rpc", () => ({
  assembleTransaction: jest.fn().mockImplementation(() => ({
    build: jest.fn().mockReturnValue({
      sign: jest.fn(),
    }),
  })),
}));

// Reset shared mock state before each test.
beforeEach(() => {
  jest.clearAllMocks();
  mockServerInstance.simulateTransaction.mockResolvedValue({
    result: { retval: { _value: 0 } },
  });
  mockServerInstance.sendTransaction.mockResolvedValue({
    status: "PENDING",
    hash: "mock-tx-hash",
  });
  mockServerInstance.getTransaction.mockResolvedValue({
    status: "SUCCESS",
  });
  mockHorizonPaymentsCall.mockResolvedValue({ records: [] });
});

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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

describe("Feeder state tracking", () => {
  let consoleLogSpy: jest.SpyInstance;

  beforeEach(() => {
    consoleLogSpy = jest.spyOn(console, "log").mockImplementation(() => {});
  });

  afterEach(() => {
    consoleLogSpy.mockRestore();
  });

  it("runCycle calls feedSubject for every configured subject", async () => {
    const feeder = new Feeder(config, { publicKey: () => "GFEEDER" } as any);
    const feedSubjectSpy = jest
      .spyOn(feeder, "feedSubject")
      .mockResolvedValue(undefined);

    await feeder.runCycle();

    expect(feedSubjectSpy).toHaveBeenCalledTimes(2);
    expect(feedSubjectSpy).toHaveBeenCalledWith("GSUBJECT1", undefined);
    expect(feedSubjectSpy).toHaveBeenCalledWith("GSUBJECT2", undefined);

    feedSubjectSpy.mockRestore();
  });

  it("syncState map is initially empty so first cycle syncs every subject", () => {
    const feeder = new Feeder(config, { publicKey: () => "GFEEDER" } as any);
    expect((feeder as any).syncState.size).toBe(0);
  });

  it("skips unchanged subject on subsequent cycles with 'unchanged, skipping' log", async () => {
    const feeder = new Feeder(config, { publicKey: () => "GFEEDER" } as unknown as Keypair);

    // Pre-populate sync state with values that match what the mocks return:
    // - getActiveVcCount returns 3 (via scValToNative)
    // - fetchHorizonStats with empty records returns volume=0, txCount=0, avg=0
    (feeder as any).syncState.set("GSUBJECT1", {
      vcCount: 3,
      volume30d: BigInt(0),
      txCount30d: 0,
      avgCounterparties: 0,
    });

    sdk.scValToNative.mockReturnValue(3);
    mockHorizonPaymentsCall.mockResolvedValue({ records: [] });

    // The feeder should detect no change and return early.
    await feeder.feedSubject("GSUBJECT1");

    expect(consoleLogSpy).toHaveBeenCalledWith(
      "[feeder] GSUBJECT1 — unchanged, skipping",
    );

    const syncingCalls = consoleLogSpy.mock.calls.filter(
      (call: string[]) => call[0] === "[feeder] syncing GSUBJECT1",
    );
    expect(syncingCalls).toHaveLength(0);
  });

  it("re-syncs subject whose VC count has changed since last cycle", async () => {
    const feeder = new Feeder(config, { publicKey: () => "GFEEDER" } as unknown as Keypair);

    // Pre-populate sync state with old values.
    (feeder as any).syncState.set("GSUBJECT1", {
      vcCount: 3,
      volume30d: BigInt(0),
      txCount30d: 0,
      avgCounterparties: 0,
    });

    // Return a different vcCount (5) so the comparison detects a change.
    sdk.scValToNative.mockReturnValue(5);

    mockHorizonPaymentsCall.mockResolvedValue({ records: [] });

    // feeder detects the change and proceeds (no rejection needed —
    // the mock pipeline is complete enough to succeed).
    await feeder.feedSubject("GSUBJECT1");

    // Should have logged "syncing" (only appears when data changed).
    expect(consoleLogSpy).toHaveBeenCalledWith(
      "[feeder] syncing GSUBJECT1",
    );

    // Should NOT have logged the skip message.
    const skipCalls = consoleLogSpy.mock.calls.filter(
      (call: string[]) =>
        call[0] === "[feeder] GSUBJECT1 — unchanged, skipping",
    );
    expect(skipCalls).toHaveLength(0);
  });

  // Use extended timeout: waitForConfirmation sleeps 3 s per tx.
  it("syncState is updated after a successful sync", async () => {
    const feeder = new Feeder(config, { publicKey: () => "GFEEDER" } as unknown as Keypair);

    // The syncState should be empty before the cycle.
    expect((feeder as any).syncState.size).toBe(0);

    // Mock: getActiveVcCount returns 3, getHasIdentityOracle returns null.
    sdk.scValToNative
      .mockReturnValueOnce(3)
      .mockReturnValueOnce(null);

    mockHorizonPaymentsCall.mockResolvedValue({ records: [] });

    // Properly mock the full submission pipeline.
    mockServerInstance.simulateTransaction.mockResolvedValue({
      result: { retval: {} },
    });
    mockServerInstance.sendTransaction.mockResolvedValue({
      status: "PENDING",
      hash: "tx-hash",
    });
    mockServerInstance.getTransaction.mockResolvedValue({
      status: "SUCCESS",
    });
    mockServerInstance.getAccount.mockResolvedValue({ sequence: "99" });

    // feedSubject proceeds through the full sync path.
    await feeder.feedSubject("GSUBJECT1");

    // After successful sync, state should be populated.
    expect((feeder as any).syncState.size).toBe(1);
    const state = (feeder as any).syncState.get("GSUBJECT1");
    expect(state.vcCount).toBe(3);
    expect(state.volume30d).toBe(BigInt(0));
    expect(state.txCount30d).toBe(0);
    expect(state.avgCounterparties).toBe(0);
  }, 15_000);
});

describe("Horizon rate limiting handling", () => {
  it("retries on 429 and respects Retry-After header", async () => {
    const consoleWarn = jest
      .spyOn(console, "warn")
      .mockImplementation(() => {});

    // Override the per-test mock for Horizon call to simulate 429 then success.
    mockHorizonPaymentsCall
      .mockImplementationOnce(() => {
        const err: any = new Error("429 Too Many Requests");
        err.response = {
          status: 429,
          headers: new Map([["retry-after", "1"]]),
        };
        throw err;
      })
      .mockImplementationOnce(() => Promise.resolve({ records: [] }));

    // Dynamic import so the mock is picked up.
    const stats = await (
      await import("./index")
    ).fetchHorizonStats("https://horizon.example", "GADDR");
    expect(stats.txCount30d).toBe(0);
    expect(consoleWarn).toHaveBeenCalled();

    consoleWarn.mockRestore();
  });
});
