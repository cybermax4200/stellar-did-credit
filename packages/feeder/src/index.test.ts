/* eslint-disable @typescript-eslint/no-explicit-any */
import {
  Feeder,
  parsePollIntervalMs,
  MIN_POLL_INTERVAL_MS,
  isValidSorobanContractId,
} from "./index";
import type { FeederConfig } from "./index";
import type { Keypair } from "@stellar/stellar-sdk";
import * as sdk from "@stellar/stellar-sdk";
import * as http from "http";
import { HealthTracker, createHealthServer } from "./health";
import type { HealthSnapshot } from "./health";

// ---------------------------------------------------------------------------
// Shared mock instances so individual tests can reconfigure behaviour.
// ---------------------------------------------------------------------------

const mockServerInstance = {
  getAccount: jest.fn().mockResolvedValue({ sequenceNumber: () => "1" }),
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
  getLatestLedger: jest.fn().mockResolvedValue({ sequence: 100 }),
  getEvents: jest.fn().mockResolvedValue({ events: [], latestLedger: 100 }),
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
    assembleTransaction: jest.fn().mockImplementation(() => ({
      build: jest.fn().mockReturnValue({
        sign: jest.fn(),
      }),
    })),
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
  Address: Object.assign(
    jest.fn().mockImplementation(() => ({
      toScVal: jest.fn().mockReturnValue({}),
    })),
    {
      fromString: jest.fn((address: string) => {
        if (
          (address.startsWith("C") || address.startsWith("G")) &&
          address.length === 56
        ) {
          return {};
        }
        throw new Error("Invalid address");
      }),
    },
  ),
  xdr: {
    ScVal: {
      scvMap: jest.fn().mockReturnValue({}),
      scvSymbol: jest.fn().mockReturnValue({}),
    },
    ScMapEntry: jest.fn(),
    Operation: {},
  },
  Keypair: {
    fromPublicKey: jest.fn().mockImplementation((publicKey: string) => {
      if (publicKey.startsWith("G") && publicKey.length === 56) {
        return {
          publicKey: () => publicKey,
        };
      }
      throw new Error("Invalid address");
    }),
  },
  Horizon: {
    Server: jest.fn().mockImplementation(() => mockHorizonInstance),
  },
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
  mockServerInstance.getLatestLedger.mockResolvedValue({ sequence: 100 });
  mockServerInstance.getEvents.mockResolvedValue({ events: [], latestLedger: 100 });
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
  subjects: ["GBAD5234567234567234567234567234567234567234567234567231", "GBAD5234567234567234567234567234567234567234567234567232"],
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
    expect(feedSubjectSpy.mock.calls[0][0]).toBe("GBAD5234567234567234567234567234567234567234567234567231");
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
    expect(feedSubjectSpy).toHaveBeenCalledWith("GBAD5234567234567234567234567234567234567234567234567231", undefined);
    expect(feedSubjectSpy).toHaveBeenCalledWith("GBAD5234567234567234567234567234567234567234567234567232", undefined);

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
    (feeder as any).syncState.set("GBAD5234567234567234567234567234567234567234567234567231", {
      vcCount: 3,
      volume30d: BigInt(0),
      txCount30d: 0,
      avgCounterparties: 0,
    });

    jest.mocked(sdk.scValToNative).mockReturnValue(3);
    mockHorizonPaymentsCall.mockResolvedValue({ records: [] });

    // The feeder should detect no change and return early.
    await feeder.feedSubject("GBAD5234567234567234567234567234567234567234567234567231");

    expect(consoleLogSpy).toHaveBeenCalledWith(
      "[feeder] GBAD5234567234567234567234567234567234567234567234567231 — unchanged, skipping",
    );

    const syncingCalls = consoleLogSpy.mock.calls.filter(
      (call: string[]) => call[0] === "[feeder] syncing GBAD5234567234567234567234567234567234567234567234567231",
    );
    expect(syncingCalls).toHaveLength(0);
  });

  it("re-syncs subject whose VC count has changed since last cycle", async () => {
    const feeder = new Feeder(config, { publicKey: () => "GFEEDER" } as unknown as Keypair);

    // Pre-populate sync state with old values.
    (feeder as any).syncState.set("GBAD5234567234567234567234567234567234567234567234567231", {
      vcCount: 3,
      volume30d: BigInt(0),
      txCount30d: 0,
      avgCounterparties: 0,
    });

    // Return a different vcCount (5) so the comparison detects a change.
    jest.mocked(sdk.scValToNative).mockReturnValue(5);

    mockHorizonPaymentsCall.mockResolvedValue({ records: [] });

    // feeder detects the change and proceeds (no rejection needed —
    // the mock pipeline is complete enough to succeed).
    await feeder.feedSubject("GBAD5234567234567234567234567234567234567234567234567231");

    // Should have logged "syncing" (only appears when data changed).
    expect(consoleLogSpy).toHaveBeenCalledWith(
      "[feeder] syncing GBAD5234567234567234567234567234567234567234567234567231",
    );

    // Should NOT have logged the skip message.
    const skipCalls = consoleLogSpy.mock.calls.filter(
      (call: string[]) =>
        call[0] === "[feeder] GBAD5234567234567234567234567234567234567234567234567231 — unchanged, skipping",
    );
    expect(skipCalls).toHaveLength(0);
  });

  // Use extended timeout: waitForConfirmation sleeps 3 s per tx.
  it("syncState is updated after a successful sync", async () => {
    const feeder = new Feeder(config, { publicKey: () => "GFEEDER" } as unknown as Keypair);

    // The syncState should be empty before the cycle.
    expect((feeder as any).syncState.size).toBe(0);

    // Mock: getActiveVcCount returns 3, getHasIdentityOracle returns null.
    jest.mocked(sdk.scValToNative)
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
    mockServerInstance.getAccount.mockResolvedValue({
      sequenceNumber: () => "99",
    });

    // feedSubject proceeds through the full sync path.
    await feeder.feedSubject("GBAD5234567234567234567234567234567234567234567234567231");

    // After successful sync, state should be populated.
    expect((feeder as any).syncState.size).toBe(1);
    const state = (feeder as any).syncState.get("GBAD5234567234567234567234567234567234567234567234567231");
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
    ).fetchHorizonStats("https://horizon.example", "GBAD5234567234567234567234567234567234567234567234567233");
    expect(stats.txCount30d).toBe(0);
    expect(consoleWarn).toHaveBeenCalled();

    consoleWarn.mockRestore();
  });

  describe("edge cases", () => {
    const subject = "GBAD5234567234567234567234567234567234567234567234567233";
    let consoleWarn: jest.SpyInstance;
    let setTimeoutSpy: jest.SpyInstance;
    let sleepDelays: number[];

    // Run sleep() callbacks immediately and record the requested delays.
    beforeEach(() => {
      consoleWarn = jest.spyOn(console, "warn").mockImplementation(() => {});
      sleepDelays = [];
      setTimeoutSpy = jest.spyOn(global, "setTimeout").mockImplementation(((
        cb: () => void,
        ms?: number,
      ) => {
        sleepDelays.push(ms ?? 0);
        cb();
        return 0 as unknown as NodeJS.Timeout;
      }) as unknown as typeof setTimeout);
    });

    afterEach(() => {
      consoleWarn.mockRestore();
      setTimeoutSpy.mockRestore();
    });

    function rateLimitError(headers: Map<string, string>): any {
      const err: any = new Error("429 Too Many Requests");
      err.response = { status: 429, headers };
      return err;
    }

    it("429 without Retry-After header uses exponential backoff", async () => {
      // Pin jitter factor (0.5 + random) to exactly 1.
      const randomSpy = jest.spyOn(Math, "random").mockReturnValue(0.5);

      mockHorizonPaymentsCall
        .mockImplementationOnce(() => Promise.reject(rateLimitError(new Map())))
        .mockImplementationOnce(() => Promise.reject(rateLimitError(new Map())))
        .mockImplementationOnce(() => Promise.resolve({ records: [] }));

      const { fetchHorizonStats } = await import("./index");
      const stats = await fetchHorizonStats("https://horizon.example", subject, {
        maxRetries: 3,
        retryBaseDelayMs: 10,
      });

      expect(stats.txCount30d).toBe(0);
      expect(mockHorizonPaymentsCall).toHaveBeenCalledTimes(3);
      expect(sleepDelays).toEqual([10, 20]);
      expect(consoleWarn).toHaveBeenCalledTimes(2);
      randomSpy.mockRestore();
    });

    it("429 with Retry-After=0 waits the 500ms floor and then succeeds", async () => {
      mockHorizonPaymentsCall
        .mockImplementationOnce(() =>
          Promise.reject(rateLimitError(new Map([["retry-after", "0"]]))),
        )
        .mockImplementationOnce(() => Promise.resolve({ records: [] }));

      const { fetchHorizonStats } = await import("./index");
      const stats = await fetchHorizonStats("https://horizon.example", subject, {
        maxRetries: 3,
        retryBaseDelayMs: 10,
      });

      expect(stats.txCount30d).toBe(0);
      expect(mockHorizonPaymentsCall).toHaveBeenCalledTimes(2);
      expect(sleepDelays).toEqual([500]);
    });

    it("throws HorizonRateLimitError once max retries are exhausted", async () => {
      mockHorizonPaymentsCall.mockImplementation(() =>
        Promise.reject(rateLimitError(new Map([["retry-after", "1"]]))),
      );

      const { fetchHorizonStats } = await import("./index");
      await expect(
        fetchHorizonStats("https://horizon.example", subject, {
          maxRetries: 2,
          retryBaseDelayMs: 10,
        }),
      ).rejects.toMatchObject({
        name: "HorizonRateLimitError",
        message: "Horizon rate limit persisted after 2 retries",
      });

      // Initial attempt + 2 retries, each retry honouring Retry-After=1s.
      expect(mockHorizonPaymentsCall).toHaveBeenCalledTimes(3);
      expect(sleepDelays).toEqual([1000, 1000]);
    });
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
      "GBAD5234567234567234567234567234567234567234567234567234",
    );

    expect(stats.volume30d).toBe(BigInt(0));
    expect(stats.txCount30d).toBe(0);
    expect(stats.avgCounterparties).toBe(0);
    expect(consoleLogSpy).toHaveBeenCalledWith(
      "[feeder] Account not found for GBAD5234567234567234567234567234567234567234567234567234, skipping",
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
      fetchHorizonStats("https://horizon.example", "GBAD5234567234567234567234567234567234567234567234567233"),
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
      "GBAD5234567234567234567234567234567234567234567234567233",
    );

    expect(stats.volume30d).toBe(BigInt(0));
    expect(stats.txCount30d).toBe(0);
    expect(stats.avgCounterparties).toBe(0);
  });

  it("getActiveVcCount returns 0 for unknown subject without throwing", async () => {
    const { getActiveVcCount } = await import("./index");

    jest.mocked(sdk.scValToNative).mockReturnValueOnce(0);
    mockServerInstance.simulateTransaction.mockResolvedValueOnce({
      result: { retval: {} },
    });

    const count = await getActiveVcCount(mockServerInstance as any, config, "GBAD5234567234567234567234567234567234567234567234567235");

    expect(count).toBe(0);
  });

  it("getActiveVcCount throws on simulation error", async () => {
    const { getActiveVcCount } = await import("./index");
    const subjectAddress =
      "GBAD5234567234567234567234567234567234567234567234567235";

    mockServerInstance.simulateTransaction.mockResolvedValueOnce({
      error: "HostError: Missing contract",
    });

    await expect(
      getActiveVcCount(mockServerInstance as any, config, subjectAddress),
    ).rejects.toThrow(
      `get_active_vc_count contract error for ${subjectAddress}: HostError: Missing contract`,
    );

    expect(consoleErrorSpy).toHaveBeenCalledWith(
      expect.stringContaining("get_active_vc_count contract error"),
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
      subjects: ["GVALID12345678901234567890123456789012345678901234567890", "INVALID"],
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
      expect.stringMatching(/GBAD5234567234567234567234567234567234567234567234567231.*permanent.*skipped/),
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
      "GBAD5234567234567234567234567234567234567234567234567233",
    );

    expect(stats.volume30d).toBe(BigInt(0));
    expect(consoleLogSpy).toHaveBeenCalledWith(
      "[feeder] Account not found for GBAD5234567234567234567234567234567234567234567234567233, skipping",
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
      fetchHorizonStats("https://horizon.example", "GBAD5234567234567234567234567234567234567234567234567233"),
    ).rejects.toThrow("Network timeout");
  });
});

describe("isValidSorobanContractId", () => {
  it("accepts a well-formed Soroban contract address", () => {
    expect(isValidSorobanContractId("C" + "A".repeat(55))).toBe(true);
  });

  it("rejects addresses that do not start with C", () => {
    expect(isValidSorobanContractId("G" + "A".repeat(55))).toBe(false);
    expect(isValidSorobanContractId("c" + "A".repeat(55))).toBe(false);
  });

  it("rejects addresses with the wrong length", () => {
    expect(isValidSorobanContractId("C" + "A".repeat(10))).toBe(false);
    expect(isValidSorobanContractId("C" + "A".repeat(54))).toBe(false);
    expect(isValidSorobanContractId("C" + "A".repeat(56))).toBe(false);
  });

  it("rejects empty, whitespace, and non-string values", () => {
    expect(isValidSorobanContractId("")).toBe(false);
    expect(isValidSorobanContractId("   ")).toBe(false);
    expect(isValidSorobanContractId("not-an-address")).toBe(false);
  });
});

describe("parsePollIntervalMs", () => {
  let exitSpy: jest.SpyInstance;
  let errorSpy: jest.SpyInstance;
  let warnSpy: jest.SpyInstance;

  beforeEach(() => {
    // Make process.exit throw so we can assert on it without killing the process.
    exitSpy = jest
      .spyOn(process, "exit")
      .mockImplementation((code?: string | number | null | undefined) => {
        throw new Error(`process.exit(${code})`);
      });
    errorSpy = jest.spyOn(console, "error").mockImplementation(() => {});
    warnSpy = jest.spyOn(console, "warn").mockImplementation(() => {});
  });

  afterEach(() => {
    exitSpy.mockRestore();
    errorSpy.mockRestore();
    warnSpy.mockRestore();
  });

  it("returns the value when given a valid interval at the minimum boundary", () => {
    expect(parsePollIntervalMs(String(MIN_POLL_INTERVAL_MS))).toBe(
      MIN_POLL_INTERVAL_MS,
    );
    expect(exitSpy).not.toHaveBeenCalled();
  });

  it("returns the value when given a valid interval well above the minimum", () => {
    expect(parsePollIntervalMs("3600000")).toBe(3_600_000);
    expect(exitSpy).not.toHaveBeenCalled();
  });

  it("exits with error for a non-numeric string", () => {
    expect(() => parsePollIntervalMs("abc")).toThrow("process.exit(1)");
    expect(errorSpy).toHaveBeenCalledWith(
      expect.stringContaining("Non-numeric values are not accepted"),
    );
  });

  it("exits with error for an empty string", () => {
    expect(() => parsePollIntervalMs("")).toThrow("process.exit(1)");
    expect(errorSpy).toHaveBeenCalledWith(
      expect.stringContaining("Non-numeric values are not accepted"),
    );
  });

  it("exits with error for a float string", () => {
    expect(() => parsePollIntervalMs("3600000.5")).toThrow("process.exit(1)");
    expect(errorSpy).toHaveBeenCalledWith(
      expect.stringContaining("Non-numeric values are not accepted"),
    );
  });

  it("exits with error for a mixed alphanumeric string", () => {
    expect(() => parsePollIntervalMs("100abc")).toThrow("process.exit(1)");
    expect(errorSpy).toHaveBeenCalledWith(
      expect.stringContaining("Non-numeric values are not accepted"),
    );
  });

  it("exits with error for a negative value", () => {
    expect(() => parsePollIntervalMs("-1000")).toThrow("process.exit(1)");
    expect(errorSpy).toHaveBeenCalledWith(
      expect.stringContaining("must be a positive integer greater than zero"),
    );
  });

  it("exits with error for zero", () => {
    expect(() => parsePollIntervalMs("0")).toThrow("process.exit(1)");
    expect(errorSpy).toHaveBeenCalledWith(
      expect.stringContaining("must be a positive integer greater than zero"),
    );
  });

  it("logs a warning and exits with error for a value below the minimum", () => {
    expect(() => parsePollIntervalMs("1000")).toThrow("process.exit(1)");
    expect(warnSpy).toHaveBeenCalledWith(
      expect.stringContaining("below the recommended minimum"),
    );
    expect(errorSpy).toHaveBeenCalledWith(
      expect.stringContaining(`must be at least ${MIN_POLL_INTERVAL_MS}ms`),
    );
  });

  it("handles whitespace-padded valid values", () => {
    expect(parsePollIntervalMs("  3600000  ")).toBe(3_600_000);
    expect(exitSpy).not.toHaveBeenCalled();
  });
});

describe("Dead-letter queue", () => {
  let consoleLogSpy: jest.SpyInstance;
  let consoleErrorSpy: jest.SpyInstance;
  let consoleWarnSpy: jest.SpyInstance;

  beforeEach(() => {
    consoleLogSpy = jest.spyOn(console, "log").mockImplementation(() => {});
    consoleErrorSpy = jest.spyOn(console, "error").mockImplementation(() => {});
    consoleWarnSpy = jest.spyOn(console, "warn").mockImplementation(() => {});
  });

  afterEach(() => {
    consoleLogSpy.mockRestore();
    consoleErrorSpy.mockRestore();
    consoleWarnSpy.mockRestore();
  });

  it("publishes cycle failures and partial recovery through GET /health", async () => {
    const healthTracker = new HealthTracker();
    const feeder = new Feeder(
      { ...config, maxConsecutiveFailures: 2, healthTracker },
      { publicKey: () => "GFEEDER" } as Keypair,
    );
    const feedSubjectSpy = jest
      .spyOn(feeder, "feedSubject")
      .mockRejectedValue(new Error("transient failure"));
    const server = createHealthServer(0, healthTracker);

    try {
      await new Promise<void>((resolve, reject) => {
        server.once("listening", resolve);
        server.once("error", reject);
      });
      const address = server.address();
      if (!address || typeof address === "string") {
        throw new Error("Expected health server to listen on a TCP port");
      }
      const healthUrl = `http://127.0.0.1:${address.port}/health`;
      const readHealth = (): Promise<HealthSnapshot> =>
        new Promise((resolve, reject) => {
          http.get(healthUrl, (res) => {
            let body = "";
            res.on("data", (chunk) => {
              body += chunk;
            });
            res.on("error", reject);
            res.on("end", () => {
              try {
                expect(res.statusCode).toBe(200);
                expect(res.headers["content-type"]).toBe("application/json");
                resolve(JSON.parse(body) as HealthSnapshot);
              } catch (err) {
                reject(err);
              }
            });
          }).on("error", reject);
        });

      await feeder.runCycle();
      expect(await readHealth()).toMatchObject({
        deadLetterCount: 0,
        deadLetterSubjects: [],
      });

      await feeder.runCycle();
      expect(await readHealth()).toMatchObject({
        deadLetterCount: 2,
        deadLetterSubjects: config.subjects,
      });

      // Further failures must not count a subject more than once.
      await feeder.runCycle();
      expect(await readHealth()).toMatchObject({
        deadLetterCount: 2,
        deadLetterSubjects: config.subjects,
      });

      feedSubjectSpy.mockImplementation(async (subject) => {
        if (subject === config.subjects[1]) {
          throw new Error("still failing");
        }
      });
      await feeder.runCycle();
      expect(await readHealth()).toMatchObject({
        failureCount: 7,
        deadLetterCount: 1,
        deadLetterSubjects: [config.subjects[1]],
      });

      feedSubjectSpy.mockResolvedValue(undefined);
      await feeder.runCycle();
      expect(await readHealth()).toMatchObject({
        failureCount: 7,
        deadLetterCount: 0,
        deadLetterSubjects: [],
      });
    } finally {
      await new Promise<void>((resolve, reject) => {
        server.close((err) => {
          if (err) reject(err);
          else resolve();
        });
      });
    }
  });

  it("subject appears in dead-letter after MAX_CONSECUTIVE_FAILURES consecutive failures", async () => {
    const healthTracker = new HealthTracker();
    const configWithThreshold: FeederConfig = {
      ...config,
      maxConsecutiveFailures: 5,
      healthTracker,
    };
    const feeder = new Feeder(configWithThreshold, { publicKey: () => "GFEEDER" } as any);

    // feedSubject throws every time
    jest
      .spyOn(feeder, "feedSubject")
      .mockRejectedValue(new Error("transient failure"));

    for (let i = 0; i < 5; i++) {
      await feeder.runCycle();
    }

    // Subject should be in dead-letter queue
    const deadLetters = feeder.getDeadLetterSubjects();
    expect(deadLetters).toContain("GBAD5234567234567234567234567234567234567234567234567231");
    expect(deadLetters).toContain("GBAD5234567234567234567234567234567234567234567234567232");
    expect(healthTracker.getSnapshot()).toMatchObject({
      deadLetterCount: 2,
      deadLetterSubjects: config.subjects,
    });

    // Should have logged ERROR with [dead-letter] prefix
    const deadLetterErrors = consoleErrorSpy.mock.calls.filter(
      (call: string[]) => typeof call[0] === "string" && call[0].includes("[dead-letter]") && call[0].includes("has failed"),
    );
    expect(deadLetterErrors.length).toBeGreaterThan(0);
  });

  it("subject is cleared from dead-letter after a successful feed", async () => {
    const healthTracker = new HealthTracker();
    const configWithThreshold: FeederConfig = {
      ...config,
      maxConsecutiveFailures: 5,
      healthTracker,
    };
    const feeder = new Feeder(configWithThreshold, { publicKey: () => "GFEEDER" } as any);

    const feedSubjectSpy = jest
      .spyOn(feeder, "feedSubject")
      .mockRejectedValue(new Error("transient failure"));

    // Fail 5 times to enter dead-letter
    for (let i = 0; i < 5; i++) {
      await feeder.runCycle();
    }
    expect(feeder.getDeadLetterSubjects()).toContain("GBAD5234567234567234567234567234567234567234567234567231");

    // Now succeed
    feedSubjectSpy.mockResolvedValue(undefined);
    await feeder.runCycle();

    // Dead-letter should be cleared
    expect(feeder.getDeadLetterSubjects()).not.toContain("GBAD5234567234567234567234567234567234567234567234567231");
    expect(healthTracker.getSnapshot()).toMatchObject({
      deadLetterCount: 0,
      deadLetterSubjects: [],
    });

    // Should have logged recovery message
    const recoveryLogs = consoleLogSpy.mock.calls.filter(
      (call: string[]) => typeof call[0] === "string" && call[0].includes("recovered"),
    );
    expect(recoveryLogs.length).toBeGreaterThan(0);
  });

  it("logs sub-threshold failures at warn level with failure count", async () => {
    const healthTracker = new HealthTracker();
    const configWithThreshold: FeederConfig = {
      ...config,
      maxConsecutiveFailures: 5,
      healthTracker,
    };
    const feeder = new Feeder(configWithThreshold, { publicKey: () => "GFEEDER" } as any);

    jest
      .spyOn(feeder, "feedSubject")
      .mockRejectedValue(new Error("transient failure"));

    // Fail only 3 times (below threshold of 5)
    for (let i = 0; i < 3; i++) {
      await feeder.runCycle();
    }

    // Subject should NOT be in dead-letter yet
    const deadLetters = feeder.getDeadLetterSubjects();
    expect(deadLetters).not.toContain("GBAD5234567234567234567234567234567234567234567234567231");
    expect(healthTracker.getSnapshot()).toMatchObject({
      deadLetterCount: 0,
      deadLetterSubjects: [],
    });

    // Should have logged warn with failure count
    const warnLogs = consoleWarnSpy.mock.calls.filter(
      (call: string[]) => typeof call[0] === "string" && call[0].includes("[dead-letter]") && call[0].includes("will retry"),
    );
    expect(warnLogs.length).toBeGreaterThan(0);
  });

  it("still retries dead-letter subjects each cycle", async () => {
    const configWithThreshold: FeederConfig = {
      ...config,
      maxConsecutiveFailures: 5,
    };
    const feeder = new Feeder(configWithThreshold, { publicKey: () => "GFEEDER" } as any);

    const feedSubjectSpy = jest
      .spyOn(feeder, "feedSubject")
      .mockRejectedValue(new Error("transient failure"));

    // Fail 5 times to enter dead-letter
    for (let i = 0; i < 5; i++) {
      await feeder.runCycle();
    }

    // Fail one more cycle — feedSubject should still be called for both subjects
    feedSubjectSpy.mockClear();
    feedSubjectSpy.mockRejectedValue(new Error("still failing"));
    await feeder.runCycle();

    // feedSubject was still called for both subjects (dead-letter does not drop)
    expect(feedSubjectSpy).toHaveBeenCalledTimes(2);
  });
});

describe("Feeder oracle configuration handling", () => {
  let consoleLogSpy: jest.SpyInstance;

  beforeEach(() => {
    consoleLogSpy = jest.spyOn(console, "log").mockImplementation(() => {});
  });

  afterEach(() => {
    consoleLogSpy.mockRestore();
  });

  it("skips set_vc_count when identity oracle is configured", async () => {
    const feeder = new Feeder(config, { publicKey: () => "GFEEDER" } as unknown as Keypair);

    // Mock getHasIdentityOracle to return true (oracle is configured)
    const getHasIdentityOracleSpy = jest
      .spyOn(feeder, "getHasIdentityOracle")
      .mockResolvedValue(true);

    // Mock the other necessary functions
    jest.mocked(sdk.scValToNative).mockReturnValue(3);
    mockHorizonPaymentsCall.mockResolvedValue({ records: [] });

    // Mock successful transaction submission for update_tx_stats only
    mockServerInstance.simulateTransaction.mockResolvedValue({
      result: { retval: {} },
    });
    mockServerInstance.sendTransaction.mockResolvedValue({
      status: "PENDING",
      hash: "update-tx-stats-hash",
    });
    mockServerInstance.getTransaction.mockResolvedValue({
      status: "SUCCESS",
    });
    mockServerInstance.getAccount.mockResolvedValue({
      sequenceNumber: () => "99",
    });

    await feeder.feedSubject("GBAD5234567234567234567234567234567234567234567234567231");

    // Should have called getHasIdentityOracle to check configuration
    expect(getHasIdentityOracleSpy).toHaveBeenCalled();

    // Should log that set_vc_count is being skipped due to cross-contract lookup
    expect(consoleLogSpy).toHaveBeenCalledWith(
      "  skipping set_vc_count (cross-contract lookup configured)"
    );

    // Should have only submitted one transaction (update_tx_stats)
    expect(mockServerInstance.sendTransaction).toHaveBeenCalledTimes(1);

    getHasIdentityOracleSpy.mockRestore();
  });

  it("skips set_vc_count when skipLegacyVcCount is explicitly enabled", async () => {
    const configWithSkip: FeederConfig = {
      ...config,
      skipLegacyVcCount: true,
    };
    const feeder = new Feeder(configWithSkip, { publicKey: () => "GFEEDER" } as unknown as Keypair);

    // Mock getHasIdentityOracle to return false (oracle not configured)
    const getHasIdentityOracleSpy = jest
      .spyOn(feeder, "getHasIdentityOracle")
      .mockResolvedValue(false);

    // Mock the other necessary functions
    jest.mocked(sdk.scValToNative).mockReturnValue(3);
    mockHorizonPaymentsCall.mockResolvedValue({ records: [] });

    // Mock successful transaction submission for update_tx_stats only
    mockServerInstance.simulateTransaction.mockResolvedValue({
      result: { retval: {} },
    });
    mockServerInstance.sendTransaction.mockResolvedValue({
      status: "PENDING",
      hash: "update-tx-stats-hash",
    });
    mockServerInstance.getTransaction.mockResolvedValue({
      status: "SUCCESS",
    });
    mockServerInstance.getAccount.mockResolvedValue({
      sequenceNumber: () => "99",
    });

    await feeder.feedSubject("GBAD5234567234567234567234567234567234567234567234567231");

    // Should have called getHasIdentityOracle to check configuration
    expect(getHasIdentityOracleSpy).toHaveBeenCalled();

    // Should log that set_vc_count is being skipped due to explicit configuration
    expect(consoleLogSpy).toHaveBeenCalledWith(
      "  skipping set_vc_count (skipLegacyVcCount enabled)"
    );

    // Should have only submitted one transaction (update_tx_stats)
    expect(mockServerInstance.sendTransaction).toHaveBeenCalledTimes(1);

    getHasIdentityOracleSpy.mockRestore();
  });

  it("calls set_vc_count when neither oracle nor skipLegacyVcCount are configured", async () => {
    const feeder = new Feeder(config, { publicKey: () => "GFEEDER" } as unknown as Keypair);

    // Mock getHasIdentityOracle to return false (oracle not configured)
    const getHasIdentityOracleSpy = jest
      .spyOn(feeder, "getHasIdentityOracle")
      .mockResolvedValue(false);

    // Mock the other necessary functions
    jest.mocked(sdk.scValToNative).mockReturnValue(3);
    mockHorizonPaymentsCall.mockResolvedValue({ records: [] });

    // Mock successful transaction submission for both set_vc_count and update_tx_stats
    mockServerInstance.simulateTransaction.mockResolvedValue({
      result: { retval: {} },
    });
    mockServerInstance.sendTransaction
      .mockResolvedValueOnce({
        status: "PENDING",
        hash: "set-vc-count-hash",
      })
      .mockResolvedValueOnce({
        status: "PENDING",
        hash: "update-tx-stats-hash",
      });
    mockServerInstance.getTransaction.mockResolvedValue({
      status: "SUCCESS",
    });
    mockServerInstance.getAccount.mockResolvedValue({
      sequenceNumber: () => "99",
    });

    await feeder.feedSubject("GBAD5234567234567234567234567234567234567234567234567231");

    // Should have called getHasIdentityOracle to check configuration
    expect(getHasIdentityOracleSpy).toHaveBeenCalled();

    // Should NOT have logged the skip message
    const skipLogs = consoleLogSpy.mock.calls.filter(
      (call: string[]) => call[0]?.includes("skipping set_vc_count")
    );
    expect(skipLogs).toHaveLength(0);

    // Should have submitted two transactions (set_vc_count + update_tx_stats)
    expect(mockServerInstance.sendTransaction).toHaveBeenCalledTimes(2);

    // Should log the transaction hashes
    expect(consoleLogSpy).toHaveBeenCalledWith("  set_vc_count tx   = set-vc-count-hash");
    expect(consoleLogSpy).toHaveBeenCalledWith("  update_tx_stats tx = update-tx-stats-hash");

    getHasIdentityOracleSpy.mockRestore();
  }, 15_000);
});

// ---------------------------------------------------------------------------
// fetchHorizonStats — mixed operation types (issue #503)
// ---------------------------------------------------------------------------

describe("fetchHorizonStats — mixed operation types", () => {
  const SUBJECT = "GBAD5234567234567234567234567234567234567234567234567231";
  const OTHER   = "GBAD5234567234567234567234567234567234567234567234567232";
  const NOW_ISO = new Date(Date.now() - 60_000).toISOString(); // 1 minute ago

  it("counts path_payment_strict_send XLM source leg in volume and tx_count", async () => {
    const { fetchHorizonStats } = await import("./index");

    mockHorizonPaymentsCall.mockResolvedValueOnce({
      records: [
        {
          type: "path_payment_strict_send",
          transaction_hash: "txhash-pps",
          created_at: NOW_ISO,
          from: SUBJECT,
          to: OTHER,
          // Source leg: 5 XLM native
          source_asset_type: "native",
          source_amount: "5.0000000",
          // Destination leg: non-native asset
          asset_type: "credit_alphanum4",
          amount: "100.0000000",
        },
      ],
      next: jest.fn().mockResolvedValue({ records: [] }),
    });

    const stats = await fetchHorizonStats("https://horizon.example", SUBJECT);

    // 5 XLM = 50_000_000 stroops
    expect(stats.volume30d).toBe(BigInt(50_000_000));
    expect(stats.txCount30d).toBe(1);
  });

  it("counts path_payment_strict_receive XLM destination leg in volume and tx_count", async () => {
    const { fetchHorizonStats } = await import("./index");

    mockHorizonPaymentsCall.mockResolvedValueOnce({
      records: [
        {
          type: "path_payment_strict_receive",
          transaction_hash: "txhash-ppr",
          created_at: NOW_ISO,
          from: OTHER,
          to: SUBJECT,
          // Source leg: non-native
          source_asset_type: "credit_alphanum4",
          source_amount: "50.0000000",
          // Destination leg: 10 XLM native received by SUBJECT
          asset_type: "native",
          amount: "10.0000000",
        },
      ],
      next: jest.fn().mockResolvedValue({ records: [] }),
    });

    const stats = await fetchHorizonStats("https://horizon.example", SUBJECT);

    // 10 XLM = 100_000_000 stroops
    expect(stats.volume30d).toBe(BigInt(100_000_000));
    expect(stats.txCount30d).toBe(1);
  });

  it("does NOT count non-native path payment legs in volume", async () => {
    const { fetchHorizonStats } = await import("./index");

    mockHorizonPaymentsCall.mockResolvedValueOnce({
      records: [
        {
          type: "path_payment_strict_send",
          transaction_hash: "txhash-nonxlm",
          created_at: NOW_ISO,
          from: SUBJECT,
          to: OTHER,
          source_asset_type: "credit_alphanum4",
          source_amount: "999.0000000",
          asset_type: "credit_alphanum4",
          amount: "888.0000000",
        },
      ],
      next: jest.fn().mockResolvedValue({ records: [] }),
    });

    const stats = await fetchHorizonStats("https://horizon.example", SUBJECT);

    // No XLM legs — volume stays 0 but tx is counted
    expect(stats.volume30d).toBe(BigInt(0));
    expect(stats.txCount30d).toBe(1);
  });

  it("counts create_account in tx_count but not volume", async () => {
    const { fetchHorizonStats } = await import("./index");

    mockHorizonPaymentsCall.mockResolvedValueOnce({
      records: [
        {
          type: "create_account",
          transaction_hash: "txhash-ca",
          created_at: NOW_ISO,
          funder: SUBJECT,
          account: OTHER,
          starting_balance: "1.0000000",
        },
      ],
      next: jest.fn().mockResolvedValue({ records: [] }),
    });

    const stats = await fetchHorizonStats("https://horizon.example", SUBJECT);

    expect(stats.volume30d).toBe(BigInt(0));
    expect(stats.txCount30d).toBe(1);
    // OTHER is tracked as a counterparty
    expect(stats.avgCounterparties).toBe(1);
  });

  it("counts claim_claimable_balance in tx_count but not volume", async () => {
    const { fetchHorizonStats } = await import("./index");

    mockHorizonPaymentsCall.mockResolvedValueOnce({
      records: [
        {
          type: "claim_claimable_balance",
          transaction_hash: "txhash-ccb",
          created_at: NOW_ISO,
          claimant: SUBJECT,
        },
      ],
      next: jest.fn().mockResolvedValue({ records: [] }),
    });

    const stats = await fetchHorizonStats("https://horizon.example", SUBJECT);

    expect(stats.volume30d).toBe(BigInt(0));
    expect(stats.txCount30d).toBe(1);
  });

  it("aggregates mixed operation types correctly across one page", async () => {
    const { fetchHorizonStats } = await import("./index");

    mockHorizonPaymentsCall.mockResolvedValueOnce({
      records: [
        // 1) plain XLM payment — 3 XLM
        {
          type: "payment",
          transaction_hash: "tx1",
          created_at: NOW_ISO,
          from: OTHER,
          to: SUBJECT,
          asset_type: "native",
          amount: "3.0000000",
        },
        // 2) path_payment_strict_send — SUBJECT sends 5 XLM, receives non-native
        {
          type: "path_payment_strict_send",
          transaction_hash: "tx2",
          created_at: NOW_ISO,
          from: SUBJECT,
          to: OTHER,
          source_asset_type: "native",
          source_amount: "5.0000000",
          asset_type: "credit_alphanum4",
          amount: "200.0000000",
        },
        // 3) create_account — SUBJECT funded a new account (no volume)
        {
          type: "create_account",
          transaction_hash: "tx3",
          created_at: NOW_ISO,
          funder: SUBJECT,
          account: OTHER,
          starting_balance: "1.0000000",
        },
        // 4) claim_claimable_balance — no volume
        {
          type: "claim_claimable_balance",
          transaction_hash: "tx4",
          created_at: NOW_ISO,
          claimant: SUBJECT,
        },
        // 5) non-XLM payment — should not add to volume
        {
          type: "payment",
          transaction_hash: "tx5",
          created_at: NOW_ISO,
          from: OTHER,
          to: SUBJECT,
          asset_type: "credit_alphanum4",
          amount: "999.0000000",
        },
      ],
      next: jest.fn().mockResolvedValue({ records: [] }),
    });

    const stats = await fetchHorizonStats("https://horizon.example", SUBJECT);

    // volume = 3 XLM (payment) + 5 XLM (path_payment source) = 8 XLM = 80_000_000 stroops
    expect(stats.volume30d).toBe(BigInt(80_000_000));
    // all 5 ops are in distinct transactions
    expect(stats.txCount30d).toBe(5);
  });
});


// ---------------------------------------------------------------------------
// fetchHorizonStats — per-operation-type coverage (issue #988)
//
// Each of the five operation types fetchHorizonStats understands is exercised
// in isolation so volume, tx_count and counterparties are pinned down per type
// rather than only in aggregate.
// ---------------------------------------------------------------------------

describe("fetchHorizonStats — operation type coverage (#988)", () => {
  const SUBJECT = "GSUBJAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
  const OTHER_A = "GOTHAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
  const OTHER_B = "GOTHBAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
  const NEW_ACCOUNT = "GNEWAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
  const NOW_ISO = new Date(Date.now() - 60_000).toISOString();

  /** Wraps records in a single Horizon page whose next() is an empty page. */
  function pageOf(records: any[]): any {
    return { records, next: jest.fn().mockResolvedValue({ records: [] }) };
  }

  // -- payment ------------------------------------------------------------

  it("payment: counts native XLM volume in both directions and tx_count", async () => {
    const { fetchHorizonStats } = await import("./index");

    mockHorizonPaymentsCall.mockResolvedValueOnce(
      pageOf([
        {
          type: "payment",
          transaction_hash: "tx-pay-in",
          created_at: NOW_ISO,
          from: OTHER_A,
          to: SUBJECT,
          asset_type: "native",
          amount: "2.5000000",
        },
        {
          type: "payment",
          transaction_hash: "tx-pay-out",
          created_at: NOW_ISO,
          from: SUBJECT,
          to: OTHER_B,
          asset_type: "native",
          amount: "1.5000000",
        },
      ]),
    );

    const stats = await fetchHorizonStats("https://horizon.example", SUBJECT);

    // 2.5 XLM received + 1.5 XLM sent = 4 XLM = 40_000_000 stroops
    expect(stats.volume30d).toBe(BigInt(40_000_000));
    expect(stats.txCount30d).toBe(2);
    // One counterparty per transaction.
    expect(stats.avgCounterparties).toBe(1);
  });

  it("payment: excludes non-native amounts from volume but still counts the tx", async () => {
    const { fetchHorizonStats } = await import("./index");

    mockHorizonPaymentsCall.mockResolvedValueOnce(
      pageOf([
        {
          type: "payment",
          transaction_hash: "tx-pay-usdc",
          created_at: NOW_ISO,
          from: OTHER_A,
          to: SUBJECT,
          asset_type: "credit_alphanum4",
          asset_code: "USDC",
          amount: "999.0000000",
        },
      ]),
    );

    const stats = await fetchHorizonStats("https://horizon.example", SUBJECT);

    expect(stats.volume30d).toBe(BigInt(0));
    expect(stats.txCount30d).toBe(1);
    // The other party is still tracked even though the asset is not XLM.
    expect(stats.avgCounterparties).toBe(1);
  });

  it("payment: dedupes distinct counterparties within a single transaction", async () => {
    const { fetchHorizonStats } = await import("./index");

    mockHorizonPaymentsCall.mockResolvedValueOnce(
      pageOf([
        {
          type: "payment",
          transaction_hash: "tx-shared",
          created_at: NOW_ISO,
          from: OTHER_A,
          to: SUBJECT,
          asset_type: "native",
          amount: "1.0000000",
        },
        {
          type: "payment",
          transaction_hash: "tx-shared",
          created_at: NOW_ISO,
          from: SUBJECT,
          to: OTHER_B,
          asset_type: "native",
          amount: "1.0000000",
        },
      ]),
    );

    const stats = await fetchHorizonStats("https://horizon.example", SUBJECT);

    // Both legs share one transaction hash, so they count as one tx.
    expect(stats.txCount30d).toBe(1);
    expect(stats.volume30d).toBe(BigInt(20_000_000));
    // OTHER_A and OTHER_B are two distinct counterparties on that one tx.
    expect(stats.avgCounterparties).toBe(2);
  });

  // -- path_payment_strict_send ------------------------------------------

  it("path_payment_strict_send: counts the native source leg, ignores the non-native destination", async () => {
    const { fetchHorizonStats } = await import("./index");

    mockHorizonPaymentsCall.mockResolvedValueOnce(
      pageOf([
        {
          type: "path_payment_strict_send",
          transaction_hash: "tx-pps-native",
          created_at: NOW_ISO,
          from: SUBJECT,
          to: OTHER_A,
          // Sender spends 5 XLM...
          source_asset_type: "native",
          source_amount: "5.0000000",
          // ...and the recipient receives a non-native asset.
          asset_type: "credit_alphanum4",
          amount: "200.0000000",
        },
        {
          type: "path_payment_strict_send",
          transaction_hash: "tx-pps-nonnative",
          created_at: NOW_ISO,
          from: SUBJECT,
          to: OTHER_B,
          // Non-native source leg must not contribute volume.
          source_asset_type: "credit_alphanum4",
          source_amount: "999.0000000",
          asset_type: "native",
          amount: "12.0000000",
        },
      ]),
    );

    const stats = await fetchHorizonStats("https://horizon.example", SUBJECT);

    // Only the 5 XLM native source leg counts.
    expect(stats.volume30d).toBe(BigInt(50_000_000));
    expect(stats.txCount30d).toBe(2);
    expect(stats.avgCounterparties).toBe(1);
  });

  // -- path_payment_strict_receive ---------------------------------------

  it("path_payment_strict_receive: counts the native destination leg, ignores the non-native source", async () => {
    const { fetchHorizonStats } = await import("./index");

    mockHorizonPaymentsCall.mockResolvedValueOnce(
      pageOf([
        {
          type: "path_payment_strict_receive",
          transaction_hash: "tx-ppr-native",
          created_at: NOW_ISO,
          from: OTHER_A,
          to: SUBJECT,
          // Sender spends a non-native asset...
          source_asset_type: "credit_alphanum4",
          source_amount: "50.0000000",
          // ...and SUBJECT receives 10 XLM.
          asset_type: "native",
          amount: "10.0000000",
        },
        {
          type: "path_payment_strict_receive",
          transaction_hash: "tx-ppr-nonnative",
          created_at: NOW_ISO,
          from: OTHER_B,
          to: SUBJECT,
          // Native source leg must not count when the subject is the recipient.
          source_asset_type: "native",
          source_amount: "7.0000000",
          asset_type: "credit_alphanum4",
          amount: "300.0000000",
        },
      ]),
    );

    const stats = await fetchHorizonStats("https://horizon.example", SUBJECT);

    // Only the 10 XLM native destination leg counts.
    expect(stats.volume30d).toBe(BigInt(100_000_000));
    expect(stats.txCount30d).toBe(2);
    expect(stats.avgCounterparties).toBe(1);
  });

  it("path payments: counts the send leg and the receive leg independently", async () => {
    const { fetchHorizonStats } = await import("./index");

    mockHorizonPaymentsCall.mockResolvedValueOnce(
      pageOf([
        // SUBJECT spends 4 XLM through a strict-send path payment.
        {
          type: "path_payment_strict_send",
          transaction_hash: "tx-leg-send",
          created_at: NOW_ISO,
          from: SUBJECT,
          to: OTHER_A,
          source_asset_type: "native",
          source_amount: "4.0000000",
          asset_type: "credit_alphanum4",
          amount: "80.0000000",
        },
        // SUBJECT receives 6 XLM through a strict-receive path payment.
        {
          type: "path_payment_strict_receive",
          transaction_hash: "tx-leg-receive",
          created_at: NOW_ISO,
          from: OTHER_B,
          to: SUBJECT,
          source_asset_type: "credit_alphanum4",
          source_amount: "60.0000000",
          asset_type: "native",
          amount: "6.0000000",
        },
      ]),
    );

    const stats = await fetchHorizonStats("https://horizon.example", SUBJECT);

    // 4 XLM out + 6 XLM in = 10 XLM = 100_000_000 stroops
    expect(stats.volume30d).toBe(BigInt(100_000_000));
    expect(stats.txCount30d).toBe(2);
    expect(stats.avgCounterparties).toBe(1);
  });

  // -- create_account -----------------------------------------------------

  it("create_account: counts the tx and the counterparty without adding volume (subject funds)", async () => {
    const { fetchHorizonStats } = await import("./index");

    mockHorizonPaymentsCall.mockResolvedValueOnce(
      pageOf([
        {
          type: "create_account",
          transaction_hash: "tx-create-funded",
          created_at: NOW_ISO,
          funder: SUBJECT,
          account: NEW_ACCOUNT,
          starting_balance: "1.5000000",
        },
      ]),
    );

    const stats = await fetchHorizonStats("https://horizon.example", SUBJECT);

    // starting_balance is locked in the new account, not a payment flow.
    expect(stats.volume30d).toBe(BigInt(0));
    expect(stats.txCount30d).toBe(1);
    // The new account is the counterparty.
    expect(stats.avgCounterparties).toBe(1);
  });

  it("create_account: tracks the funder as counterparty when the subject is the new account", async () => {
    const { fetchHorizonStats } = await import("./index");

    mockHorizonPaymentsCall.mockResolvedValueOnce(
      pageOf([
        {
          type: "create_account",
          transaction_hash: "tx-create-received",
          created_at: NOW_ISO,
          funder: OTHER_A,
          account: SUBJECT,
          starting_balance: "2.0000000",
        },
      ]),
    );

    const stats = await fetchHorizonStats("https://horizon.example", SUBJECT);

    expect(stats.volume30d).toBe(BigInt(0));
    expect(stats.txCount30d).toBe(1);
    // The funder is the counterparty.
    expect(stats.avgCounterparties).toBe(1);
  });

  // -- claim_claimable_balance -------------------------------------------

  it("claim_claimable_balance: counts the tx only, with no volume and no self counterparty", async () => {
    const { fetchHorizonStats } = await import("./index");

    mockHorizonPaymentsCall.mockResolvedValueOnce(
      pageOf([
        {
          type: "claim_claimable_balance",
          transaction_hash: "tx-claim",
          created_at: NOW_ISO,
          claimant: SUBJECT,
        },
      ]),
    );

    const stats = await fetchHorizonStats("https://horizon.example", SUBJECT);

    expect(stats.volume30d).toBe(BigInt(0));
    expect(stats.txCount30d).toBe(1);
    // The claimant is the subject itself, so it is not a counterparty.
    expect(stats.avgCounterparties).toBe(0);
  });

  it("aggregates all five operation types in one page", async () => {
    const { fetchHorizonStats } = await import("./index");

    mockHorizonPaymentsCall.mockResolvedValueOnce(
      pageOf([
        {
          type: "payment",
          transaction_hash: "agg-tx1",
          created_at: NOW_ISO,
          from: OTHER_A,
          to: SUBJECT,
          asset_type: "native",
          amount: "2.0000000",
        },
        {
          type: "path_payment_strict_send",
          transaction_hash: "agg-tx2",
          created_at: NOW_ISO,
          from: SUBJECT,
          to: OTHER_B,
          source_asset_type: "native",
          source_amount: "3.0000000",
          asset_type: "credit_alphanum4",
          amount: "120.0000000",
        },
        {
          type: "path_payment_strict_receive",
          transaction_hash: "agg-tx3",
          created_at: NOW_ISO,
          from: OTHER_A,
          to: SUBJECT,
          source_asset_type: "credit_alphanum4",
          source_amount: "40.0000000",
          asset_type: "native",
          amount: "4.0000000",
        },
        {
          type: "create_account",
          transaction_hash: "agg-tx4",
          created_at: NOW_ISO,
          funder: SUBJECT,
          account: NEW_ACCOUNT,
          starting_balance: "1.0000000",
        },
        {
          type: "claim_claimable_balance",
          transaction_hash: "agg-tx5",
          created_at: NOW_ISO,
          claimant: SUBJECT,
        },
      ]),
    );

    const stats = await fetchHorizonStats("https://horizon.example", SUBJECT);

    // 2 (payment) + 3 (strict-send source) + 4 (strict-receive dest) = 9 XLM
    expect(stats.volume30d).toBe(BigInt(90_000_000));
    expect(stats.txCount30d).toBe(5);
    // counterparties: tx1→1, tx2→1, tx3→1, tx4→1, tx5→0 = 4 / 5 txns
    expect(stats.avgCounterparties).toBe(1);
  });
});


// ---------------------------------------------------------------------------
// Issue #518: a transient failure mid-pagination must not discard the subject
// ---------------------------------------------------------------------------

const PARTIAL_SUBJECT =
  "GBAD5234567234567234567234567234567234567234567234567233";

/** A transient Horizon failure (503) shaped like an SDK HTTP error. */
function transientHorizonError(): any {
  const err: any = new Error("503 Service Unavailable");
  err.response = { status: 503 };
  return err;
}

/** One page holding a single in-window 10 XLM payment on its own transaction. */
function pageWithPayment(
  subject: string,
  txHash: string,
  next: () => Promise<any>,
): any {
  return {
    records: [
      {
        type: "payment",
        transaction_hash: txHash,
        created_at: new Date().toISOString(),
        from: subject,
        to: "GOTHER234567234567234567234567234567234567234567234567234",
        asset_type: "native",
        amount: "10.0000000",
      },
    ],
    next,
  };
}

describe("Horizon partial pagination handling (#518)", () => {
  it("retries page.next() up to maxRetries, then commits the pages already read", async () => {
    const consoleWarn = jest
      .spyOn(console, "warn")
      .mockImplementation(() => {});

    const { fetchHorizonStats } = await import("./index");

    // Page 1 succeeds; the jump to page 2 fails transiently every time.
    const next = jest.fn().mockImplementation(() => {
      throw transientHorizonError();
    });
    mockHorizonPaymentsCall.mockResolvedValueOnce(
      pageWithPayment(PARTIAL_SUBJECT, "tx1", next),
    );

    const stats = await fetchHorizonStats(
      "https://horizon.example",
      PARTIAL_SUBJECT,
      { maxRetries: 2, retryBaseDelayMs: 1 },
    );

    // Initial attempt + 2 retries.
    expect(next).toHaveBeenCalledTimes(3);

    // Page 1's data survives instead of being thrown away.
    expect(stats.partial).toBe(true);
    expect(stats.txCount30d).toBe(1);
    expect(stats.volume30d).toBe(BigInt(100_000_000));
    expect(stats.avgCounterparties).toBe(1);
    expect(consoleWarn).toHaveBeenCalled();

    consoleWarn.mockRestore();
  });

  it("returns a complete result when a retried page.next() eventually succeeds", async () => {
    const consoleWarn = jest
      .spyOn(console, "warn")
      .mockImplementation(() => {});

    const { fetchHorizonStats } = await import("./index");

    // Page 2 fails once, then succeeds and yields a second payment.
    const next = jest
      .fn()
      .mockImplementationOnce(() => {
        throw transientHorizonError();
      })
      .mockImplementationOnce(() =>
        Promise.resolve(
          pageWithPayment(
            PARTIAL_SUBJECT,
            "tx2",
            jest.fn().mockResolvedValue({ records: [] }),
          ),
        ),
      );
    mockHorizonPaymentsCall.mockResolvedValueOnce(
      pageWithPayment(PARTIAL_SUBJECT, "tx1", next),
    );

    const stats = await fetchHorizonStats(
      "https://horizon.example",
      PARTIAL_SUBJECT,
      { maxRetries: 3, retryBaseDelayMs: 1 },
    );

    expect(next).toHaveBeenCalledTimes(2);
    // Both pages were aggregated, so nothing is missing.
    expect(stats.partial).toBe(false);
    expect(stats.txCount30d).toBe(2);
    expect(stats.volume30d).toBe(BigInt(200_000_000));

    consoleWarn.mockRestore();
  });

  it("rethrows a non-transient mid-pagination error instead of silently truncating", async () => {
    const { fetchHorizonStats } = await import("./index");

    const next = jest.fn().mockImplementation(() => {
      throw new Error("malformed response body");
    });
    mockHorizonPaymentsCall.mockResolvedValueOnce(
      pageWithPayment(PARTIAL_SUBJECT, "tx1", next),
    );

    await expect(
      fetchHorizonStats("https://horizon.example", PARTIAL_SUBJECT, {
        maxRetries: 2,
        retryBaseDelayMs: 1,
      }),
    ).rejects.toThrow("malformed response body");

    // Permanent errors are not retried.
    expect(next).toHaveBeenCalledTimes(1);
  });

  it("reports partial: false for a fully paginated fetch", async () => {
    const { fetchHorizonStats } = await import("./index");

    mockHorizonPaymentsCall.mockResolvedValueOnce(
      pageWithPayment(
        PARTIAL_SUBJECT,
        "tx1",
        jest.fn().mockResolvedValue({ records: [] }),
      ),
    );

    const stats = await fetchHorizonStats(
      "https://horizon.example",
      PARTIAL_SUBJECT,
      { maxRetries: 2, retryBaseDelayMs: 1 },
    );

    expect(stats.partial).toBe(false);
    expect(stats.txCount30d).toBe(1);
  });

  it("reports partial: false when the account has no history", async () => {
    const { fetchHorizonStats } = await import("./index");

    mockHorizonPaymentsCall.mockResolvedValueOnce({ records: [] });

    const stats = await fetchHorizonStats(
      "https://horizon.example",
      PARTIAL_SUBJECT,
    );

    expect(stats.partial).toBe(false);
    expect(stats.txCount30d).toBe(0);
  });
});

describe("FEEDER_ALLOW_PARTIAL_STATS gating (#518)", () => {
  const SUBJECT = config.subjects[0]!;

  /** Config tuned for fast retries; identity oracle is mocked as configured. */
  const fastConfig: FeederConfig = {
    ...config,
    maxRetries: 1,
    retryBaseDelayMs: 1,
  };

  /** Makes Horizon serve one page and then fail transiently forever. */
  function serveOnePageThenFail(): void {
    mockHorizonPaymentsCall.mockResolvedValue(
      pageWithPayment(
        SUBJECT,
        "tx1",
        jest.fn().mockImplementation(() => {
          throw transientHorizonError();
        }),
      ),
    );
  }

  function primeTransactionMocks(): void {
    mockServerInstance.simulateTransaction.mockResolvedValue({
      result: { retval: {} },
    });
    mockServerInstance.sendTransaction.mockResolvedValue({
      status: "PENDING",
      hash: "update-tx-stats-hash",
    });
    mockServerInstance.getTransaction.mockResolvedValue({ status: "SUCCESS" });
    mockServerInstance.getAccount.mockResolvedValue({
      sequenceNumber: () => "99",
    });
    jest.mocked(sdk.scValToNative).mockReturnValue(1);
  }

  it("submits partial stats and logs partial = true by default", async () => {
    const consoleLog = jest.spyOn(console, "log").mockImplementation(() => {});
    const consoleWarn = jest
      .spyOn(console, "warn")
      .mockImplementation(() => {});

    const feeder = new Feeder(fastConfig, {
      publicKey: () => "GFEEDER",
    } as unknown as Keypair);
    jest.spyOn(feeder, "getHasIdentityOracle").mockResolvedValue(true);
    primeTransactionMocks();
    serveOnePageThenFail();

    await feeder.feedSubject(SUBJECT);

    // The partial flag gets its own log line so monitoring can alert on it.
    expect(
      consoleLog.mock.calls.some((c) =>
        String(c[0]).includes("partial           = true"),
      ),
    ).toBe(true);
    // set_vc_count is skipped (identity oracle configured), so the single
    // submission here is update_tx_stats: partial data was still committed.
    expect(mockServerInstance.sendTransaction).toHaveBeenCalledTimes(1);

    consoleLog.mockRestore();
    consoleWarn.mockRestore();
  });

  it("suppresses update_tx_stats when allowPartialStats is false", async () => {
    const consoleLog = jest.spyOn(console, "log").mockImplementation(() => {});
    const consoleWarn = jest
      .spyOn(console, "warn")
      .mockImplementation(() => {});

    const feeder = new Feeder(
      { ...fastConfig, allowPartialStats: false },
      { publicKey: () => "GFEEDER" } as unknown as Keypair,
    );
    jest.spyOn(feeder, "getHasIdentityOracle").mockResolvedValue(true);
    primeTransactionMocks();
    serveOnePageThenFail();

    await feeder.feedSubject(SUBJECT);

    // Nothing was written on-chain for this subject.
    expect(mockServerInstance.sendTransaction).not.toHaveBeenCalled();
    expect(
      consoleWarn.mock.calls.some((c) =>
        String(c[0]).includes("skipping update_tx_stats"),
      ),
    ).toBe(true);

    consoleLog.mockRestore();
    consoleWarn.mockRestore();
  });

  it("still submits a complete fetch when allowPartialStats is false", async () => {
    const consoleLog = jest.spyOn(console, "log").mockImplementation(() => {});

    const feeder = new Feeder(
      { ...fastConfig, allowPartialStats: false },
      { publicKey: () => "GFEEDER" } as unknown as Keypair,
    );
    jest.spyOn(feeder, "getHasIdentityOracle").mockResolvedValue(true);
    primeTransactionMocks();
    mockHorizonPaymentsCall.mockResolvedValue(
      pageWithPayment(
        SUBJECT,
        "tx1",
        jest.fn().mockResolvedValue({ records: [] }),
      ),
    );

    await feeder.feedSubject(SUBJECT);

    expect(mockServerInstance.sendTransaction).toHaveBeenCalledTimes(1);
    expect(
      consoleLog.mock.calls.some((c) =>
        String(c[0]).includes("partial           = false"),
      ),
    ).toBe(true);

    consoleLog.mockRestore();
  });
});


// ---------------------------------------------------------------------------
// Issue #954: governanceId validation in the Feeder constructor
// ---------------------------------------------------------------------------

describe("Feeder governanceId validation (#954)", () => {
  it("accepts a Feeder when governanceId is omitted (field is optional)", () => {
    expect(
      () => new Feeder(config, { publicKey: () => "GFEEDER" } as any),
    ).not.toThrow();
  });

  it("accepts a well-formed governanceId", () => {
    const validId = "C" + "A".repeat(55);
    expect(
      () =>
        new Feeder(
          { ...config, governanceId: validId },
          { publicKey: () => "GFEEDER" } as any,
        ),
    ).not.toThrow();
  });

  it("rejects a governanceId that does not start with 'C'", () => {
    expect(
      () =>
        new Feeder(
          { ...config, governanceId: "G" + "A".repeat(55) },
          { publicKey: () => "GFEEDER" } as any,
        ),
    ).toThrow(/Invalid governanceId/);
  });

  it("rejects a governanceId that is the wrong length", () => {
    expect(
      () =>
        new Feeder(
          { ...config, governanceId: "C" + "A".repeat(10) },
          { publicKey: () => "GFEEDER" } as any,
        ),
    ).toThrow(/Invalid governanceId/);
  });

  it("rejects a governanceId that is not an address at all", () => {
    expect(
      () =>
        new Feeder(
          { ...config, governanceId: "not-a-contract" },
          { publicKey: () => "GFEEDER" } as any,
        ),
    ).toThrow(/Invalid governanceId/);
  });

  it("names the field and the expected shape in the error message", () => {
    expect(
      () =>
        new Feeder(
          { ...config, governanceId: "bad" },
          { publicKey: () => "GFEEDER" } as any,
        ),
    ).toThrow(/must start with 'C' and be 56 characters/);
  });
});

// ---------------------------------------------------------------------------
// #930: revocationRegistryId config validation
// ---------------------------------------------------------------------------

describe("Feeder config: revocationRegistryId validation", () => {
  const baseConfig: FeederConfig = { ...config };
  const keypair = { publicKey: () => "GAAAA" } as unknown as Keypair;

  it("throws when revocationRegistryId is not a valid C-address", () => {
    expect(() => new Feeder({ ...baseConfig, revocationRegistryId: "not-a-contract" }, keypair)).toThrow(
      /revocationRegistryId must be a valid Soroban contract ID \(C-address\)/,
    );
  });

  it("throws when revocationRegistryId is a G-address", () => {
    expect(
      () =>
        new Feeder(
          { ...baseConfig, revocationRegistryId: "GBUQWP3BOUZX34ULNQG23RQ6F4YUSXHTQSXE7XDZT4A65XJLQRGEZSM" },
          keypair,
        ),
    ).toThrow(/revocationRegistryId must be a valid Soroban contract ID/);
  });

  it("accepts a valid C-address", () => {
    expect(
      () =>
        new Feeder(
          {
            ...baseConfig,
            revocationRegistryId: "C" + "A".repeat(55),
          },
          keypair,
        ),
    ).not.toThrow();
  });
});

// ---------------------------------------------------------------------------
// #969: maxConsecutiveFailures config validation
// ---------------------------------------------------------------------------

describe("Feeder config: maxConsecutiveFailures validation (#969)", () => {
  const baseConfig: FeederConfig = { ...config };
  const keypair = { publicKey: () => "GFEEDER" } as any;

  it("accepts a Feeder when maxConsecutiveFailures is omitted (field is optional)", () => {
    expect(() => new Feeder(baseConfig, keypair)).not.toThrow();
  });

  it("accepts maxConsecutiveFailures === 1", () => {
    expect(() => new Feeder({ ...baseConfig, maxConsecutiveFailures: 1 }, keypair)).not.toThrow();
  });

  it("accepts a positive integer > 1", () => {
    expect(() => new Feeder({ ...baseConfig, maxConsecutiveFailures: 10 }, keypair)).not.toThrow();
  });

  it("rejects maxConsecutiveFailures === 0", () => {
    expect(() => new Feeder({ ...baseConfig, maxConsecutiveFailures: 0 }, keypair)).toThrow(
      /maxConsecutiveFailures must be >= 1/,
    );
  });

  it("rejects negative maxConsecutiveFailures", () => {
    expect(() => new Feeder({ ...baseConfig, maxConsecutiveFailures: -1 }, keypair)).toThrow(
      /maxConsecutiveFailures must be >= 1/,
    );
  });

  it("rejects non-integer maxConsecutiveFailures", () => {
    expect(() => new Feeder({ ...baseConfig, maxConsecutiveFailures: 2.5 }, keypair)).toThrow(
      /maxConsecutiveFailures must be >= 1/,
    );
  });

  it("names the field and requirement in the error message", () => {
    expect(() => new Feeder({ ...baseConfig, maxConsecutiveFailures: 0 }, keypair)).toThrow(
      /maxConsecutiveFailures must be >= 1 \(got 0\)/,
    );
  });
});


describe("Feeder config: revocationRegistryId optional", () => {
  const baseConfig: FeederConfig = { ...config };
  const keypair = { publicKey: () => "GAAAA" } as unknown as Keypair;

  it("accepts an omitted revocationRegistryId (optional field)", () => {
    expect(() => new Feeder(baseConfig, keypair)).not.toThrow();
  });
});

// ---------------------------------------------------------------------------
// #968: eventPollIntervalMs config validation
// ---------------------------------------------------------------------------

describe("Feeder config: eventPollIntervalMs validation", () => {
  const baseConfig: FeederConfig = { ...config };
  const keypair = { publicKey: () => "GAAAA" } as unknown as Keypair;

  it("throws when eventPollIntervalMs is below the minimum (0)", () => {
    expect(() => new Feeder({ ...baseConfig, eventPollIntervalMs: 0 }, keypair)).toThrow(
      /eventPollIntervalMs must be at least 1000/,
    );
  });

  it("throws when eventPollIntervalMs is below the minimum (999)", () => {
    expect(() => new Feeder({ ...baseConfig, eventPollIntervalMs: 999 }, keypair)).toThrow(
      /eventPollIntervalMs must be at least 1000 \(1 second\)/,
    );
  });

  it("accepts eventPollIntervalMs at the minimum boundary (1000)", () => {
    expect(() => new Feeder({ ...baseConfig, eventPollIntervalMs: 1000 }, keypair)).not.toThrow();
  });

  it("accepts eventPollIntervalMs above the minimum", () => {
    expect(() => new Feeder({ ...baseConfig, eventPollIntervalMs: 5000 }, keypair)).not.toThrow();
  });

  it("accepts an omitted eventPollIntervalMs (optional field)", () => {
    expect(() => new Feeder(baseConfig, keypair)).not.toThrow();
  });
});
