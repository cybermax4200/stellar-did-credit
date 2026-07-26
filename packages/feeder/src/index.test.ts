import { Feeder } from "./index";
import type { FeederConfig } from "./index";
import type { Keypair } from "@stellar/stellar-sdk";

jest.mock("@stellar/stellar-sdk", () => ({
  SorobanRpc: {
    Server: jest.fn().mockImplementation(() => ({})),
  },
  Contract: jest.fn(),
  TransactionBuilder: jest.fn(),
  BASE_FEE: "100",
  Account: jest.fn(),
  scValToNative: jest.fn(),
  nativeToScVal: jest.fn(),
  Address: jest.fn(),
  xdr: {},
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

describe("Horizon rate limiting handling", () => {
  it("retries on 429 and respects Retry-After header", async () => {
    const consoleWarn = jest.spyOn(console, "warn").mockImplementation(() => {});

    // Mock Horizon.Server to simulate a 429 on first call with Retry-After=1
    const mockCall = jest
      .fn()
      .mockImplementationOnce(() => {
        const err: any = new Error("429 Too Many Requests");
        err.response = { status: 429, headers: new Map([["retry-after", "1"]]) };
        throw err;
      })
      .mockImplementationOnce(() => Promise.resolve({ records: [] }));

    const payments = () => ({ forAccount: () => ({ order: () => ({ limit: () => ({ call: mockCall }) }) }) });
    (Horizon as any).Server = jest.fn().mockImplementation(() => ({ payments }));

    const stats = await (await import("./index")).fetchHorizonStats("https://horizon.example", "GADDR");
    expect(stats.txCount30d).toBe(0);
    expect(consoleWarn).toHaveBeenCalled();

    consoleWarn.mockRestore();
  });
});
