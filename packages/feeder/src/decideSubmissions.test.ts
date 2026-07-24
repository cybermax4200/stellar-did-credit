import { decideSubmissions, statsEqual } from "./index";
import type { LastSubmitted, TxStats } from "./index";

function makeStats(overrides: Partial<TxStats> = {}): TxStats {
  return {
    volume30d: 0n,
    txCount30d: 0,
    avgCounterparties: 0,
    ...overrides,
  };
}

describe("statsEqual", () => {
  it("returns true for identical stats", () => {
    const a = makeStats({ volume30d: 5_000_000n, txCount30d: 3, avgCounterparties: 2 });
    const b = makeStats({ volume30d: 5_000_000n, txCount30d: 3, avgCounterparties: 2 });
    expect(statsEqual(a, b)).toBe(true);
  });

  it("returns true for two separate all-zero stats objects", () => {
    expect(statsEqual(makeStats(), makeStats())).toBe(true);
  });

  it("returns false when volume30d differs", () => {
    const a = makeStats({ volume30d: 1_000_000n });
    const b = makeStats({ volume30d: 2_000_000n });
    expect(statsEqual(a, b)).toBe(false);
  });

  it("returns false when txCount30d differs", () => {
    const a = makeStats({ txCount30d: 1 });
    const b = makeStats({ txCount30d: 2 });
    expect(statsEqual(a, b)).toBe(false);
  });

  it("returns false when avgCounterparties differs", () => {
    const a = makeStats({ avgCounterparties: 1 });
    const b = makeStats({ avgCounterparties: 2 });
    expect(statsEqual(a, b)).toBe(false);
  });

  it("compares bigint volume30d by value, not by reference", () => {
    // Two independently-constructed bigints with the same value must be
    // treated as equal — this would fail if the comparison ever became a
    // reference/object equality check instead of a primitive `===`.
    const a = makeStats({ volume30d: BigInt("123456789012345") });
    const b = makeStats({ volume30d: BigInt("123456789012345") });
    expect(statsEqual(a, b)).toBe(true);
  });
});

describe("decideSubmissions", () => {
  it("submits both on a subject's first sync, even when everything is zero", () => {
    // This is the exact empty-account scenario from the bug report: Horizon
    // returns no payment records, so vcCount and stats are all zero — but
    // since there's no prior submission, both must still go out once.
    const decision = decideSubmissions(undefined, 0, makeStats());
    expect(decision).toEqual({ vcCountChanged: true, statsChanged: true });
  });

  it("submits both on a subject's first sync with non-zero values", () => {
    const decision = decideSubmissions(
      undefined,
      3,
      makeStats({ volume30d: 10n, txCount30d: 2, avgCounterparties: 1 }),
    );
    expect(decision).toEqual({ vcCountChanged: true, statsChanged: true });
  });

  it("skips both when nothing changed since the last submission", () => {
    const stats = makeStats({ volume30d: 50n, txCount30d: 4, avgCounterparties: 2 });
    const previous: LastSubmitted = { vcCount: 5, stats };
    const decision = decideSubmissions(previous, 5, { ...stats });
    expect(decision).toEqual({ vcCountChanged: false, statsChanged: false });
  });

  it("repeatedly skips both across multiple unchanged cycles for an empty account", () => {
    // Same fixed point reached from an empty-account first sync.
    const zero = makeStats();
    let previous: LastSubmitted | undefined = undefined;

    const firstCycle = decideSubmissions(previous, 0, zero);
    expect(firstCycle).toEqual({ vcCountChanged: true, statsChanged: true });
    previous = { vcCount: 0, stats: zero };

    for (let cycle = 0; cycle < 3; cycle++) {
      const decision = decideSubmissions(previous, 0, makeStats());
      expect(decision).toEqual({ vcCountChanged: false, statsChanged: false });
    }
  });

  it("flags only vcCountChanged when only the vc count changed", () => {
    const stats = makeStats({ volume30d: 50n, txCount30d: 4, avgCounterparties: 2 });
    const previous: LastSubmitted = { vcCount: 5, stats };
    const decision = decideSubmissions(previous, 6, { ...stats });
    expect(decision).toEqual({ vcCountChanged: true, statsChanged: false });
  });

  it("flags only statsChanged when only the stats changed", () => {
    const stats = makeStats({ volume30d: 50n, txCount30d: 4, avgCounterparties: 2 });
    const previous: LastSubmitted = { vcCount: 5, stats };
    const decision = decideSubmissions(previous, 5, {
      ...stats,
      txCount30d: 5,
    });
    expect(decision).toEqual({ vcCountChanged: false, statsChanged: true });
  });

  it("flags both when both changed", () => {
    const stats = makeStats({ volume30d: 50n, txCount30d: 4, avgCounterparties: 2 });
    const previous: LastSubmitted = { vcCount: 5, stats };
    const decision = decideSubmissions(previous, 6, {
      ...stats,
      volume30d: 999n,
    });
    expect(decision).toEqual({ vcCountChanged: true, statsChanged: true });
  });

  it("treats a vc count change from a non-zero value back to zero as a change", () => {
    const stats = makeStats();
    const previous: LastSubmitted = { vcCount: 2, stats };
    const decision = decideSubmissions(previous, 0, stats);
    expect(decision.vcCountChanged).toBe(true);
  });
});
