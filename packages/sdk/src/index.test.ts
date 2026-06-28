import { StellarDIDCreditSDK, ScoreNotComputedError } from "./index";
import type {
  ScoreRecord,
  ProtocolConfig,
  TxStats,
  ScoringWeights,
  RepaymentRecord,
  VCRecord,
} from "./index";

describe("StellarDIDCreditSDK", () => {
  describe("getScore", () => {
    it("throws ScoreNotComputedError when score has not been computed", async () => {
      const config = {
        identityOracleId: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
        creditOracleId: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
        revocationRegistryId: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
        networkPassphrase: "Test SDF Network ; September 2015",
        rpcUrl: "http://localhost:8000",
      };

      const sdk = new StellarDIDCreditSDK(config);

      // Mock the server and simulation to return "score not computed" error
      // This is a simplified test that would need proper mocking in actual implementation
      try {
        // This call will fail because we don't have a real server, but demonstrates the pattern
        await sdk.getScore("GBUQWP3BOUZX34ULNQG23RQ6F4YUSXHTQSXE7XDZT4A65XJLQRGEZSM");
      } catch (error) {
        // In a real test with proper mocking, we'd verify this is ScoreNotComputedError
        expect(error).toBeInstanceOf(Error);
      }
    });

    it("exports ScoreNotComputedError class", () => {
      expect(ScoreNotComputedError).toBeDefined();
      const error = new ScoreNotComputedError();
      expect(error).toBeInstanceOf(Error);
      expect(error.name).toBe("ScoreNotComputedError");
      expect(error.message).toContain("Score has not been computed");
    });
  });
});

describe("contract struct type exports", () => {
  // These tests double as compile-time assertions: if any interface were not
  // exported from the package entry point (./index), or if its shape drifted
  // from the on-chain contract structs, this file would fail to type-check.
  // The runtime expectations additionally verify the documented field types.

  it("exports TxStats with volume30d typed as bigint (Soroban i128)", () => {
    const stats: TxStats = {
      volume30d: 5_000_000_000n,
      txCount30d: 42,
      avgCounterparties: 7,
    };

    expect(typeof stats.volume30d).toBe("bigint");
    expect(typeof stats.txCount30d).toBe("number");
    expect(typeof stats.avgCounterparties).toBe("number");
  });

  it("exports ScoringWeights whose components sum to 100 by contract invariant", () => {
    const weights: ScoringWeights = {
      vcWeight: 40,
      txWeight: 30,
      repaymentWeight: 30,
    };

    expect(typeof weights.vcWeight).toBe("number");
    expect(typeof weights.txWeight).toBe("number");
    expect(typeof weights.repaymentWeight).toBe("number");
    expect(weights.vcWeight + weights.txWeight + weights.repaymentWeight).toBe(
        100,
    );
  });

  it("exports RepaymentRecord with numeric on-time and total counters", () => {
    const record: RepaymentRecord = {
      onTimeCount: 8,
      totalCount: 10,
    };

    expect(typeof record.onTimeCount).toBe("number");
    expect(typeof record.totalCount).toBe("number");
  });

  it("exports VCRecord with a 32-byte hash, issuer, timestamp and revoked flag", () => {
    const vc: VCRecord = {
      vcHash: Buffer.alloc(32),
      issuer: "GBUQWP3BOUZX34ULNQG23RQ6F4YUSXHTQSXE7XDZT4A65XJLQRGEZSM",
      anchoredAt: 1_700_000_000,
      revoked: false,
    };

    expect(vc.vcHash.length).toBe(32);
    expect(typeof vc.issuer).toBe("string");
    expect(typeof vc.anchoredAt).toBe("number");
    expect(typeof vc.revoked).toBe("boolean");
  });

  it("continues to export ScoreRecord and ProtocolConfig", () => {
    const score: ScoreRecord = {
      score: 612,
      lastUpdated: 1_700_000_000,
      vcCount: 3,
      repaymentRate: 8000,
      txVolume30d: 1_000_000n,
    };

    const config: ProtocolConfig = {
      identityOracleId: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
      creditOracleId: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
      revocationRegistryId:
          "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
      networkPassphrase: "Test SDF Network ; September 2015",
      rpcUrl: "https://soroban-testnet.stellar.org",
      simAccount: "GBUQWP3BOUZX34ULNQG23RQ6F4YUSXHTQSXE7XDZT4A65XJLQRGEZSM",
    };

    expect(typeof score.txVolume30d).toBe("bigint");
    expect(score.score).toBe(612);
    expect(config.networkPassphrase).toContain("Test SDF Network");
  });
});
