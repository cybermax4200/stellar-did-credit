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

  it("mid-pagination transient error retries and optionally returns partial stats", async () => {
    const { fetchHorizonStats } = await import("./index");
    
    const mockNext = jest.fn()
      .mockRejectedValueOnce(Object.assign(new Error("Transient"), { code: "ETIMEDOUT" }))
      .mockRejectedValueOnce(Object.assign(new Error("Transient"), { code: "ETIMEDOUT" }))
      .mockRejectedValueOnce(Object.assign(new Error("Transient"), { code: "ETIMEDOUT" }));

    mockHorizonPaymentsCall.mockResolvedValueOnce({
      records: [{
        type: "payment",
        transaction_hash: "hash1",
        created_at: new Date().toISOString(),
        asset_type: "native",
        amount: "10.0",
        from: "GBAD5234567234567234567234567234567234567234567234567233",
        to: "GOTHER"
      }],
      next: mockNext,
    });

    const consoleWarnSpy = jest.spyOn(console, "warn").mockImplementation(() => {});

    // Run fetchHorizonStats with maxRetries=2, allowPartialStats=true
    const stats = await fetchHorizonStats("https://horizon.example", "GBAD5234567234567234567234567234567234567234567234567233", 2, true);
    
    expect(stats.volume30d).toBe(BigInt(100_000_000));
    expect(stats.txCount30d).toBe(1);
    expect(stats.partial).toBe(true);
    expect(mockNext).toHaveBeenCalledTimes(3); // 1 initial try + 2 retries
    
    consoleWarnSpy.mockRestore();
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

  it("getActiveVcCount returns 0 on simulation error", async () => {
    const { getActiveVcCount } = await import("./index");

    mockServerInstance.simulateTransaction.mockResolvedValueOnce({
      error: "Simulation failed",
    });

    const count = await getActiveVcCount(mockServerInstance as any, config, "GBAD5234567234567234567234567234567234567234567234567235");

    expect(count).toBe(0);
    expect(consoleErrorSpy).toHaveBeenCalledWith(
      expect.stringContaining("get_active_vc_count simulation failed"),
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

