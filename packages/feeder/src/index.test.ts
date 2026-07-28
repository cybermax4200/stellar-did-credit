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


describe("Address validation and error handling", () => {
  let consoleLogSpy: jest.SpyInstance;
  let consoleErrorSpy: jest.SpyInstance;

  beforeEach(() => {
    consoleLogSpy = jest.spyOn(console, "log").mockImplementation(() => {});
    consoleErrorSpy = jest
      .spyOn(console, "error")
      .mockImplementation(() => {});
  });

  afterEach(() => {
    consoleLogSpy.mockRestore();
    consoleErrorSpy.mockRestore();
  });

  it("invalid address is detected early and skipped with log message", async () => {
    const { fetchHorizonStats } = await import("./index");
    const stats = await fetchHorizonStats(
      "https://horizon.example",
      "INVALID_ADDRESS",
    );

    expect(stats.volume30d).toBe(BigInt(0));
    expect(stats.txCount30d).toBe(0);
    expect(stats.avgCounterparties).toBe(0);
    expect(consoleLogSpy).toHaveBeenCalledWith(
      "[feeder] Skipping invalid Stellar address: INVALID_ADDRESS",
    );
  });

  it("account not found (404) is handled gracefully, not thrown", async () => {
    const { fetchHorizonStats } = await import("./index");

    mockHorizonPaymentsCall.mockImplementationOnce(() => {
      const err: any = new Error("Not found");
      err.response = { status: 404 };
      throw err;
    });

    const stats = await fetchHorizonStats(
      "https://horizon.example",
      "GACCOUNT",
    );

    expect(stats.volume30d).toBe(BigInt(0));
    expect(stats.txCount30d).toBe(0);
    expect(stats.avgCounterparties).toBe(0);
    expect(consoleLogSpy).toHaveBeenCalledWith(
      "[feeder] Account not found for GACCOUNT, skipping",
    );
  });

  it("transient error is re-thrown for retry by withExponentialBackoff", async () => {
    const { fetchHorizonStats } = await import("./index");

    mockHorizonPaymentsCall.mockImplementationOnce(() => {
      const err: any = new Error("Network timeout");
      err.code = "ETIMEDOUT";
      throw err;
    });

    // Should throw the transient error, not swallow it
    await expect(
      fetchHorizonStats("https://horizon.example", "GADDR"),
    ).rejects.toThrow();
  });

  it("empty payments response (zero records) handled correctly", async () => {
    const { fetchHorizonStats } = await import("./index");

    mockHorizonPaymentsCall.mockResolvedValueOnce({
      records: [],
      next: jest.fn().mockResolvedValue({ records: [] }),
    });

    const stats = await fetchHorizonStats(
      "https://horizon.example",
      "GADDR",
    );

    expect(stats.volume30d).toBe(BigInt(0));
    expect(stats.txCount30d).toBe(0);
    expect(stats.avgCounterparties).toBe(0);
  });

  it("getActiveVcCount returns 0 for unknown subject without throwing", async () => {
    const { getActiveVcCount } = await import("./index");

    sdk.scValToNative.mockReturnValueOnce(0);
    mockServerInstance.simulateTransaction.mockResolvedValueOnce({
      result: { retval: {} },
    });

    const count = await getActiveVcCount(mockServerInstance as any, config, "GSUBJECT");

    expect(count).toBe(0);
  });

  it("getActiveVcCount returns 0 on simulation error", async () => {
    const { getActiveVcCount } = await import("./index");

    mockServerInstance.simulateTransaction.mockResolvedValueOnce({
      error: "Simulation failed",
    });

    const count = await getActiveVcCount(mockServerInstance as any, config, "GSUBJECT");

    expect(count).toBe(0);
    expect(consoleErrorSpy).toHaveBeenCalledWith(
      expect.stringContaining("get_active_vc_count simulation failed"),
      expect.any(String),
    );
  });

  it("invalid VC count address is validated and returns 0", async () => {
    const { getActiveVcCount } = await import("./index");

    const count = await getActiveVcCount(
      mockServerInstance as any,
      config,
      "INVALID",
    );

    expect(count).toBe(0);
    expect(consoleLogSpy).toHaveBeenCalledWith(
      expect.stringContaining("Skipping invalid Stellar address"),
    );
  });
});

describe("runCycle error handling and summary", () => {
  let consoleLogSpy: jest.SpyInstance;
  let consoleErrorSpy: jest.SpyInstance;
  let consoleWarnSpy: jest.SpyInstance;

  beforeEach(() => {
    consoleLogSpy = jest.spyOn(console, "log").mockImplementation(() => {});
    consoleErrorSpy = jest
      .spyOn(console, "error")
      .mockImplementation(() => {});
    consoleWarnSpy = jest
      .spyOn(console, "warn")
      .mockImplementation(() => {});
  });

  afterEach(() => {
    consoleLogSpy.mockRestore();
    consoleErrorSpy.mockRestore();
    consoleWarnSpy.mockRestore();
  });

  it("runCycle completes when some subjects fail", async () => {
    const feeder = new Feeder(config, { publicKey: () => "GFEEDER" } as any);

    const feedSubjectSpy = jest
      .spyOn(feeder, "feedSubject")
      .mockImplementationOnce(async () => {
        // First subject succeeds
      })
      .mockImplementationOnce(async () => {
        // Second subject fails
        throw new Error("Transient error");
      });

    await expect(feeder.runCycle()).resolves.toBeUndefined();
    expect(feedSubjectSpy).toHaveBeenCalledTimes(2);

    feedSubjectSpy.mockRestore();
  });

  it("runCycle completes when ALL subjects fail", async () => {
    const feeder = new Feeder(config, { publicKey: () => "GFEEDER" } as any);

    const feedSubjectSpy = jest
      .spyOn(feeder, "feedSubject")
      .mockRejectedValue(new Error("All fail"));

    await expect(feeder.runCycle()).resolves.toBeUndefined();
    expect(feedSubjectSpy).toHaveBeenCalledTimes(2);

    feedSubjectSpy.mockRestore();
  });

  it("summary log shows correct succeeded/skipped/failed counts", async () => {
    const feeder = new Feeder(config, { publicKey: () => "GFEEDER" } as any);

    // Mock feedSubject: first succeeds, second fails with permanent error
    const feedSubjectSpy = jest
      .spyOn(feeder, "feedSubject")
      .mockImplementationOnce(async () => {
        // First subject succeeds
      })
      .mockImplementationOnce(async () => {
        // Second subject fails with 404 (permanent)
        const err: any = new Error("Account not found");
        err.response = { status: 404 };
        throw err;
      });

    await feeder.runCycle();

    // Should log summary with correct counts
    expect(consoleLogSpy).toHaveBeenCalledWith(
      "[feeder] Cycle complete: 1 succeeded, 1 skipped, 0 failed",
    );

    feedSubjectSpy.mockRestore();
  });

  it("validates all addresses at the start of cycle", async () => {
    const configWithInvalid: FeederConfig = {
      ...config,
      subjects: ["GVALID123456789012345678901234567890123456789012345", "INVALID"],
    };
    const feeder = new Feeder(configWithInvalid, {
      publicKey: () => "GFEEDER",
    } as any);

    const feedSubjectSpy = jest
      .spyOn(feeder, "feedSubject")
      .mockResolvedValue(undefined);

    await feeder.runCycle();

    // Should warn about invalid address
    expect(consoleWarnSpy).toHaveBeenCalledWith(
      expect.stringContaining("Found 1 invalid"),
    );

    // Should only call feedSubject for valid address
    expect(feedSubjectSpy).toHaveBeenCalledTimes(1);

    feedSubjectSpy.mockRestore();
  });

  it("includes subject address and error type in error logs", async () => {
    const feeder = new Feeder(config, { publicKey: () => "GFEEDER" } as any);

    const feedSubjectSpy = jest
      .spyOn(feeder, "feedSubject")
      .mockImplementationOnce(async () => {
        // Permanent error
        const err: any = new Error("Account not found");
        err.response = { status: 404 };
        throw err;
      });

    await feeder.runCycle();

    // Should include subject, error type, and action
    expect(consoleErrorSpy).toHaveBeenCalledWith(
      expect.stringMatching(/GSUBJECT1.*permanent.*skipped/),
    );

    feedSubjectSpy.mockRestore();
  });
});

describe("Error classification helpers", () => {
  it("isAccountNotFoundError correctly identifies 404", async () => {
    const { fetchHorizonStats } = await import("./index");

    // Mock Horizon to return 404
    mockHorizonPaymentsCall.mockImplementationOnce(() => {
      const err: any = new Error("Not found");
      err.response = { status: 404 };
      throw err;
    });

    const consoleLogSpy = jest
      .spyOn(console, "log")
      .mockImplementation(() => {});

    const stats = await fetchHorizonStats(
      "https://horizon.example",
      "GADDR",
    );

    expect(stats.volume30d).toBe(BigInt(0));
    expect(consoleLogSpy).toHaveBeenCalledWith(
      "[feeder] Account not found for GADDR, skipping",
    );

    consoleLogSpy.mockRestore();
  });

  it("isTransientError correctly identifies network errors", async () => {
    const { fetchHorizonStats } = await import("./index");

    // Mock Horizon to return network timeout
    mockHorizonPaymentsCall.mockImplementationOnce(() => {
      const err: any = new Error("Network timeout");
      err.code = "ETIMEDOUT";
      throw err;
    });

    // Should throw (not catch) transient errors
    await expect(
      fetchHorizonStats("https://horizon.example", "GADDR"),
    ).rejects.toThrow("Network timeout");
  });
});
