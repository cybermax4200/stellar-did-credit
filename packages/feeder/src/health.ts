import * as http from "http";

/** Snapshot returned by GET /health. */
export interface HealthSnapshot {
  status: "ok";
  lastCycleAt: string | null;
  successCount: number;
  failureCount: number;
}

/**
 * In-memory health state for the feeder daemon.
 * Tracks per-subject sync outcomes across all completed feed cycles.
 */
export class HealthTracker {
  private lastCycleAt: string | null = null;
  private successCount = 0;
  private failureCount = 0;
  /** null = no cycle has completed yet */
  private lastCycleSucceeded: boolean | null = null;

  /** Record the outcome of a completed feed cycle. */
  recordCycleResult(succeeded: number, failed: number): void {
    this.lastCycleAt = new Date().toISOString();
    this.successCount += succeeded;
    this.failureCount += failed;
    this.lastCycleSucceeded = failed === 0;
  }

  getSnapshot(): HealthSnapshot {
    return {
      status: "ok",
      lastCycleAt: this.lastCycleAt,
      successCount: this.successCount,
      failureCount: this.failureCount,
    };
  }

  /** True only after at least one cycle completed with zero failures. */
  isReady(): boolean {
    return this.lastCycleSucceeded === true;
  }
}

/**
 * Starts a non-blocking HTTP server exposing /health and /ready.
 * Uses Node's built-in http module — no external dependencies.
 */
export function createHealthServer(
  port: number,
  tracker: HealthTracker,
): http.Server {
  const server = http.createServer((req, res) => {
    const path = req.url?.split("?")[0];

    if (req.method === "GET" && path === "/health") {
      const body = JSON.stringify(tracker.getSnapshot());
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(body);
      return;
    }

    if (req.method === "GET" && path === "/ready") {
      if (tracker.isReady()) {
        res.writeHead(200, { "Content-Type": "application/json" });
        res.end(JSON.stringify({ status: "ready" }));
      } else {
        res.writeHead(503, { "Content-Type": "application/json" });
        res.end(JSON.stringify({ status: "not ready" }));
      }
      return;
    }

    res.writeHead(404);
    res.end();
  });

  server.listen(port);
  return server;
}

/**
 * Parses HEALTH_PORT from the environment.
 * Returns undefined when unset; exits the process on invalid values.
 */
export function parseHealthPort(raw: string | undefined): number | undefined {
  if (raw === undefined || raw.trim() === "") {
    return undefined;
  }

  const trimmed = raw.trim();
  if (!/^\d+$/.test(trimmed)) {
    console.error(
      `Error: HEALTH_PORT must be a positive integer (got "${raw}").`,
    );
    process.exit(1);
  }

  const port = parseInt(trimmed, 10);
  if (port <= 0 || port > 65535) {
    console.error(
      `Error: HEALTH_PORT must be between 1 and 65535 (got ${port}).`,
    );
    process.exit(1);
  }

  return port;
}
