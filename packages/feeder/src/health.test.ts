import * as http from "http";
import {
  HealthTracker,
  createHealthServer,
  parseHealthPort,
} from "./health";

function httpGet(
  port: number,
  path: string,
): Promise<{ statusCode: number; body: string }> {
  return new Promise((resolve, reject) => {
    http.get(`http://127.0.0.1:${port}${path}`, (res) => {
      let body = "";
      res.on("data", (chunk) => {
        body += chunk;
      });
      res.on("end", () => {
        resolve({ statusCode: res.statusCode ?? 0, body });
      });
    }).on("error", reject);
  });
}

describe("HealthTracker", () => {
  it("starts with empty state", () => {
    const tracker = new HealthTracker();
    expect(tracker.getSnapshot()).toEqual({
      status: "ok",
      lastCycleAt: null,
      successCount: 0,
      failureCount: 0,
      deadLetterCount: 0,
      deadLetterSubjects: [],
    });
    expect(tracker.isReady()).toBe(false);
  });

  it("replaces dead-letter state instead of accumulating subjects", () => {
    const tracker = new HealthTracker();
    tracker.setDeadLetterSubjects(["subject-a", "subject-b"]);
    expect(tracker.getSnapshot()).toMatchObject({
      deadLetterCount: 2,
      deadLetterSubjects: ["subject-a", "subject-b"],
    });

    tracker.setDeadLetterSubjects(["subject-b"]);
    expect(tracker.getSnapshot()).toMatchObject({
      deadLetterCount: 1,
      deadLetterSubjects: ["subject-b"],
    });

    tracker.setDeadLetterSubjects([]);
    expect(tracker.getSnapshot()).toMatchObject({
      deadLetterCount: 0,
      deadLetterSubjects: [],
    });
  });

  it("isolates dead-letter state from mutations of inputs and snapshots", () => {
    const tracker = new HealthTracker();
    const subjects = ["subject-a"];
    tracker.setDeadLetterSubjects(subjects);

    subjects.push("subject-b");
    tracker.getSnapshot().deadLetterSubjects.push("subject-c");

    expect(tracker.getSnapshot()).toMatchObject({
      deadLetterCount: 1,
      deadLetterSubjects: ["subject-a"],
    });
  });

  it("records cycle results and updates readiness", () => {
    const tracker = new HealthTracker();
    tracker.recordCycleResult(2, 0);
    expect(tracker.isReady()).toBe(true);
    expect(tracker.getSnapshot().successCount).toBe(2);
    expect(tracker.getSnapshot().failureCount).toBe(0);
    expect(tracker.getSnapshot().lastCycleAt).toMatch(
      /^\d{4}-\d{2}-\d{2}T/,
    );

    tracker.recordCycleResult(1, 1);
    // Readiness remains true once a successful cycle has completed
    // at least once, even if later cycles have failures.
    expect(tracker.isReady()).toBe(true);
    expect(tracker.getSnapshot().successCount).toBe(3);
    expect(tracker.getSnapshot().failureCount).toBe(1);
  });
});

describe("createHealthServer", () => {
  let server: http.Server;
  let port: number;
  let tracker: HealthTracker;

  beforeEach((done) => {
    tracker = new HealthTracker();
    server = createHealthServer(0, tracker);
    server.on("listening", () => {
      const address = server.address();
      if (address && typeof address !== "string") {
        port = address.port;
      }
      done();
    });
  });

  afterEach((done) => {
    server.close(done);
  });

  it("GET /health returns 200 with expected JSON before any cycle", async () => {
    const res = await httpGet(port, "/health");
    expect(res.statusCode).toBe(200);
    expect(JSON.parse(res.body)).toEqual({
      status: "ok",
      lastCycleAt: null,
      successCount: 0,
      failureCount: 0,
      deadLetterCount: 0,
      deadLetterSubjects: [],
    });
  });

  it("GET /health includes current dead-letter subjects after failures", async () => {
    tracker.setDeadLetterSubjects(["subject-a", "subject-b"]);
    tracker.recordCycleResult(1, 2);

    const res = await httpGet(port, "/health");
    expect(res.statusCode).toBe(200);
    expect(JSON.parse(res.body)).toEqual({
      status: "ok",
      lastCycleAt: expect.any(String),
      successCount: 1,
      failureCount: 2,
      deadLetterCount: 2,
      deadLetterSubjects: ["subject-a", "subject-b"],
    });
  });

  it("GET /health reflects recovery while preserving cumulative failures", async () => {
    tracker.setDeadLetterSubjects(["subject-a", "subject-b"]);
    tracker.recordCycleResult(0, 2);
    tracker.setDeadLetterSubjects(["subject-b"]);
    tracker.recordCycleResult(1, 1);

    const partialRecovery = await httpGet(port, "/health");
    expect(partialRecovery.statusCode).toBe(200);
    expect(JSON.parse(partialRecovery.body)).toMatchObject({
      failureCount: 3,
      deadLetterCount: 1,
      deadLetterSubjects: ["subject-b"],
    });

    tracker.setDeadLetterSubjects([]);
    tracker.recordCycleResult(2, 0);

    const fullRecovery = await httpGet(port, "/health");
    expect(fullRecovery.statusCode).toBe(200);
    expect(JSON.parse(fullRecovery.body)).toMatchObject({
      failureCount: 3,
      deadLetterCount: 0,
      deadLetterSubjects: [],
    });
  });

  it("GET /ready returns 503 before the first cycle completes", async () => {
    const res = await httpGet(port, "/ready");
    expect(res.statusCode).toBe(503);
    expect(JSON.parse(res.body)).toEqual({ status: "not ready" });
  });

  it("dead-letter metrics preserve readiness before and after a successful cycle", async () => {
    tracker.setDeadLetterSubjects(["subject-a"]);
    tracker.recordCycleResult(0, 1);
    expect((await httpGet(port, "/ready")).statusCode).toBe(503);

    tracker.setDeadLetterSubjects([]);
    tracker.recordCycleResult(1, 0);
    tracker.setDeadLetterSubjects(["subject-a"]);
    tracker.recordCycleResult(0, 1);

    const ready = await httpGet(port, "/ready");
    expect(ready.statusCode).toBe(200);
    expect(JSON.parse(ready.body)).toEqual({ status: "ready" });
    expect(JSON.parse((await httpGet(port, "/health")).body)).toMatchObject({
      status: "ok",
      deadLetterCount: 1,
      deadLetterSubjects: ["subject-a"],
    });
  });

  it("GET /ready returns 200 after a successful cycle", async () => {
    const tracker = new HealthTracker();
    tracker.recordCycleResult(1, 0);

    await new Promise<void>((resolve) => {
      server.close(() => {
        server = createHealthServer(0, tracker);
        server.on("listening", () => resolve());
      });
    });

    const address = server.address();
    const readyPort =
      address && typeof address !== "string" ? address.port : port;

    const res = await httpGet(readyPort, "/ready");
    expect(res.statusCode).toBe(200);
    expect(JSON.parse(res.body)).toEqual({ status: "ready" });
  });

  it("GET /ready returns 503 after a failed cycle", async () => {
    const tracker = new HealthTracker();
    // A failed cycle should not mark readiness; until a successful
    // cycle occurs, /ready stays 503.
    tracker.recordCycleResult(0, 1);

    await new Promise<void>((resolve) => {
      server.close(() => {
        server = createHealthServer(0, tracker);
        server.on("listening", () => resolve());
      });
    });

    const address = server.address();
    const readyPort =
      address && typeof address !== "string" ? address.port : port;

    const res = await httpGet(readyPort, "/ready");
    expect(res.statusCode).toBe(503);
    expect(JSON.parse(res.body)).toEqual({ status: "not ready" });
  });
});

describe("parseHealthPort", () => {
  const exitSpy = jest
    .spyOn(process, "exit")
    .mockImplementation((() => undefined) as never);

  afterEach(() => {
    exitSpy.mockClear();
  });

  afterAll(() => {
    exitSpy.mockRestore();
  });

  it("returns undefined when unset", () => {
    expect(parseHealthPort(undefined)).toBeUndefined();
    expect(parseHealthPort("")).toBeUndefined();
    expect(parseHealthPort("   ")).toBeUndefined();
  });

  it("parses a valid port", () => {
    expect(parseHealthPort("8080")).toBe(8080);
  });

  it("exits on invalid port values", () => {
    parseHealthPort("abc");
    expect(exitSpy).toHaveBeenCalledWith(1);

    exitSpy.mockClear();
    parseHealthPort("0");
    expect(exitSpy).toHaveBeenCalledWith(1);
  });
});
