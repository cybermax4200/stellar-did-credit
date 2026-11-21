import assert from "node:assert/strict";
import { test, afterEach } from "node:test";
import {
  computeScoreCommitmentAsync,
  generateScoreProofAsync,
  generateTierProofAsync,
  terminateProofWorker,
} from "./index.js";
import type {
  ScoreProofRequest,
  TierProofRequest,
  WorkerLike,
  WorkerRequest,
  WorkerResponse,
} from "./types.js";

const bytes32 = (b: number) => new Uint8Array(32).fill(b);

const base = {
  vcPoints: 50,
  txVolume30d: 50_000_000_000,
  avgCounterparties: 10,
  onTimeCount: 50,
  totalCount: 100,
  totalRepaid: 20_000_000_000,
  vcWeight: 40,
  txWeight: 30,
  repaymentWeight: 30,
  vcCount: 10,
  lastUpdated: 1_700_000_000n,
  computedAtLedger: 1_234_567,
  stale: false,
  blinding: 42,
};
const bindings = {
  subject: bytes32(1),
  creditOracleId: bytes32(2),
  snapshotLedger: 12345,
  domainSeparator: bytes32(3),
};
const scoreReq: ScoreProofRequest = { ...base, ...bindings, threshold: 600 };
const tierReq: TierProofRequest = { ...base, ...bindings, tierLevel: 1 };

/** A fake worker: records requests and lets the test decide how to answer. */
class FakeWorker implements WorkerLike {
  requests: WorkerRequest[] = [];
  terminated = false;
  private message: Array<(ev: { data: WorkerResponse }) => void> = [];
  private error: Array<(ev: { message?: string }) => void> = [];
  constructor(private readonly respond?: (req: WorkerRequest, w: FakeWorker) => void) {}
  postMessage(req: WorkerRequest) {
    this.requests.push(req);
    // Answer asynchronously, like a real worker.
    setTimeout(() => this.respond?.(req, this), 0);
  }
  terminate() {
    this.terminated = true;
  }
  addEventListener(type: string, l: never) {
    if (type === "message") this.message.push(l);
    if (type === "error") this.error.push(l);
  }
  reply(res: WorkerResponse) {
    this.message.forEach((l) => l({ data: res }));
  }
  crash(message: string) {
    this.error.forEach((l) => l({ message }));
  }
}

afterEach(() => terminateProofWorker());

const echo = (req: WorkerRequest, w: FakeWorker) =>
  w.reply({ id: req.id, ok: true, bytes: new Uint8Array(req.kind === "commitment" ? 32 : 384) });

test("score proof is delegated to the worker and resolves with its bytes", async () => {
  const w = new FakeWorker(echo);
  const proof = await generateScoreProofAsync(scoreReq, { workerFactory: () => w });
  assert.equal(proof.length, 384);
  assert.equal(w.requests.length, 1);
  assert.equal(w.requests[0].kind, "score");
  assert.deepEqual((w.requests[0].payload as ScoreProofRequest).threshold, 600);
});

test("tier proofs and commitments use their own job kinds on the same worker", async () => {
  const w = new FakeWorker(echo);
  const opts = { workerFactory: () => w };
  assert.equal((await generateTierProofAsync(tierReq, opts)).length, 384);
  assert.equal((await computeScoreCommitmentAsync(base, opts)).length, 32);
  assert.deepEqual(w.requests.map((r) => r.kind), ["tier", "commitment"]);
});

test("concurrent jobs are matched to their responses by id", async () => {
  const w = new FakeWorker((req, fw) => {
    // Reply out of order: later requests first.
    const marker = req.kind === "score" ? (req.payload as ScoreProofRequest).threshold : 0;
    setTimeout(() => fw.reply({ id: req.id, ok: true, bytes: new Uint8Array([marker & 0xff]) }), 20 - (marker % 20));
  });
  const opts = { workerFactory: () => w };
  const [a, b, c] = await Promise.all([
    generateScoreProofAsync({ ...scoreReq, threshold: 601 }, opts),
    generateScoreProofAsync({ ...scoreReq, threshold: 602 }, opts),
    generateScoreProofAsync({ ...scoreReq, threshold: 603 }, opts),
  ]);
  assert.deepEqual([a[0], b[0], c[0]], [601 & 0xff, 602 & 0xff, 603 & 0xff]);
});

test("worker-reported errors reject the matching promise", async () => {
  const w = new FakeWorker((req, fw) => fw.reply({ id: req.id, ok: false, error: "boom" }));
  await assert.rejects(generateScoreProofAsync(scoreReq, { workerFactory: () => w }), /boom/);
});

test("a worker crash rejects in-flight jobs and the next call gets a fresh worker", async () => {
  const first = new FakeWorker((_req, fw) => fw.crash("oom"));
  await assert.rejects(generateScoreProofAsync(scoreReq, { workerFactory: () => first }), /oom/);
  assert.equal(first.terminated, true);

  const second = new FakeWorker(echo);
  const proof = await generateScoreProofAsync(scoreReq, { workerFactory: () => second });
  assert.equal(proof.length, 384);
  assert.equal(second.requests.length, 1);
});

test("timeout rejects and recycles the worker", async () => {
  const w = new FakeWorker(); // never answers
  await assert.rejects(
    generateScoreProofAsync(scoreReq, { workerFactory: () => w, timeoutMs: 20 }),
    /timed out/,
  );
  assert.equal(w.terminated, true);
});

test("abort rejects immediately; an already-aborted signal never posts", async () => {
  const w = new FakeWorker(); // never answers
  const ac = new AbortController();
  const p = generateScoreProofAsync(scoreReq, { workerFactory: () => w, signal: ac.signal });
  ac.abort();
  await assert.rejects(p, /abort/i);

  const before = w.requests.length;
  await assert.rejects(generateScoreProofAsync(scoreReq, { workerFactory: () => w, signal: ac.signal }), /abort/i);
  assert.equal(w.requests.length, before);
});

test("invalid input is rejected before anything reaches the worker", async () => {
  const w = new FakeWorker(echo);
  const opts = { workerFactory: () => w };
  const bad: Array<[string, () => Promise<unknown>]> = [
    ["short subject", () => generateScoreProofAsync({ ...scoreReq, subject: new Uint8Array(31) }, opts)],
    ["long oracle id", () => generateScoreProofAsync({ ...scoreReq, creditOracleId: new Uint8Array(33) }, opts)],
    ["weights != 100", () => generateScoreProofAsync({ ...scoreReq, vcWeight: 90 }, opts)],
    ["negative volume", () => generateScoreProofAsync({ ...scoreReq, txVolume30d: -1 }, opts)],
    ["unsafe integer", () => generateScoreProofAsync({ ...scoreReq, totalRepaid: 2 ** 60 }, opts)],
    ["fractional ledger", () => generateScoreProofAsync({ ...scoreReq, snapshotLedger: 1.5 }, opts)],
    ["threshold > u32", () => generateScoreProofAsync({ ...scoreReq, threshold: 2 ** 32 }, opts)],
    ["lastUpdated not bigint", () => generateScoreProofAsync({ ...scoreReq, lastUpdated: 5 as unknown as bigint }, opts)],
    ["tier 0", () => generateTierProofAsync({ ...tierReq, tierLevel: 0 as never }, opts)],
    ["tier 4", () => generateTierProofAsync({ ...tierReq, tierLevel: 4 as never }, opts)],
  ];
  for (const [name, fn] of bad) {
    await assert.rejects(fn(), Error, name);
  }
  assert.equal(w.requests.length, 0);
});
