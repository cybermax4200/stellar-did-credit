import { parseBatchCsv, writeBatchResults, type BatchResult } from "./batch";
import { readFileSync, rmSync } from "fs";

describe("batch CSV helpers", () => {
  it("parses the required columns, including quoted values", () => {
    expect(parseBatchCsv(
      'subject,vc_hash_hex,credential_type\nGABC,0123,"employment, verified"\n',
    )).toEqual([{
      subject: "GABC",
      vcHashHex: "0123",
      credentialType: "employment, verified",
    }]);
  });

  it("rejects an invalid header", () => {
    expect(() => parseBatchCsv("subject,hash,type\nGABC,0123,kyc")).toThrow(
      "CSV header must be: subject,vc_hash_hex,credential_type",
    );
  });

  it("writes per-entry result tracking", () => {
    const path = "batch-result.test.json";
    const results: BatchResult[] = [
      { subject: "GABC", vc_hash: "0123", status: "success", txHash: "tx-1" },
      { subject: "GDEF", vc_hash: "4567", status: "failed", error: "timeout" },
      { subject: "GHIJ", vc_hash: "89ab", status: "skipped" },
    ];
    try {
      writeBatchResults(path, results);
      expect(JSON.parse(readFileSync(path, "utf8"))).toEqual(results);
    } finally {
      rmSync(path, { force: true });
    }
  });
});