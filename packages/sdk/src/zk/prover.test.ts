/**
 * Unit tests for the ZK prover module.
 *
 * Tests cover:
 * - Scoring component computation
 * - Score commitment generation
 * - Blinding factor generation
 * - collectWitness (mocked SDK)
 * - generateScoreRangeProof (mocked snarkjs)
 * - Proof bundle serialization
 * - Error handling for invalid inputs
 */

import {
  computeScoringComponents,
  computeScoreCommitment,
  generateBlinding,
  collectWitness,
  generateScoreRangeProof,
  verifyProof,
  MIN_SCORE,
  MAX_SCORE,
} from "./prover";
import type {
  ScoreWitness,
  ZKPublicParams,
  Groth16Proof,
} from "./prover";
import type { StellarDIDCreditSDK, ScoreRecord, ScoringWeights } from "../index";

// ---------------------------------------------------------------------------
// Mock snarkjs
// ---------------------------------------------------------------------------

const mockGroth16FullProve = jest.fn();
const mockGroth16Verify = jest.fn();

jest.mock("snarkjs", () => ({
  groth16: {
    fullProve: (...args: unknown[]) => mockGroth16FullProve(...args),
    verify: (...args: unknown[]) => mockGroth16Verify(...args),
  },
}));

// ---------------------------------------------------------------------------
// Test fixtures
// ---------------------------------------------------------------------------

function makeScoreRecord(overrides: Partial<ScoreRecord> = {}): ScoreRecord {
  return {
    score: 650,
    lastUpdated: 1_700_000_000,
    vcCount: 3,
    repaymentRate: 8500,
    txVolume30d: 2_000_000n,
    previousScore: 620,
    computedAtLedger: 1_234_567,
    stale: false,
    ...overrides,
  };
}

function makeScoringWeights(overrides: Partial<ScoringWeights> = {}): ScoringWeights {
  return {
    vcWeight: 50,
    txWeight: 25,
    repaymentWeight: 25,
    ...overrides,
  };
}

function makeMockSDK(scoreRecord: ScoreRecord | null = makeScoreRecord(), weights: ScoringWeights = makeScoringWeights()): StellarDIDCreditSDK {
  return {
    getScore: jest.fn().mockResolvedValue(scoreRecord),
    getWeights: jest.fn().mockResolvedValue(weights),
  } as unknown as StellarDIDCreditSDK;
}

function makeScoreWitness(overrides: Partial<ScoreWitness> = {}): ScoreWitness {
  return {
    score: 650,
    vcCount: 3,
    txVolume30d: 2_000_000n,
    avgCounterparties: 5,
    repaymentRate: 8500,
    lastUpdated: 1_700_000_000,
    computedAtLedger: 1_234_567,
    stale: false,
    vcWeight: 50,
    txWeight: 25,
    repaymentWeight: 25,
    vcScore: 60,
    txScore: 20,
    repayScore: 85,
    counterpartyBonus: 0,
    composite: 61,
    blinding: 123456789n,
    ...overrides,
  };
}

function makeGroth16Proof(): Groth16Proof {
  return {
    pi_a: ["123", "456", "789"],
    pi_b: [
      ["111", "222"],
      ["333", "444"],
      ["555", "666"],
    ],
    pi_c: ["777", "888", "999"],
    protocol: "groth16",
    curve: "bn128",
  };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("ZK Prover Module", () => {
  // -----------------------------------------------------------------------
  // computeScoringComponents
  // -----------------------------------------------------------------------
  describe("computeScoringComponents", () => {
    it("computes correct component scores for a typical witness", () => {
      const result = computeScoringComponents({
        vcCount: 3,
        txVolume30d: 2_000_000n,
        avgCounterparties: 5,
        repaymentRate: 8500,
        vcWeight: 50,
        txWeight: 25,
        repaymentWeight: 25,
      });

      expect(result.vcScore).toBe(60); // min(3 * 20, 100)
      expect(result.txScore).toBe(0); // min(2_000_000n / 100_000_000n, 100) = min(0, 100) — integer division
      expect(result.repayScore).toBe(85); // floor(8500 / 100)
      expect(result.counterpartyBonus).toBe(0); // 5 < 10
    });

    it("applies counterparty bonus when avgCounterparties >= 10", () => {
      const result = computeScoringComponents({
        vcCount: 3,
        txVolume30d: 2_000_000n,
        avgCounterparties: 10,
        repaymentRate: 8500,
        vcWeight: 50,
        txWeight: 25,
        repaymentWeight: 25,
      });

      expect(result.counterpartyBonus).toBe(10);
    });

    it("caps vcScore at 100", () => {
      const result = computeScoringComponents({
        vcCount: 10,
        txVolume30d: 0n,
        avgCounterparties: 0,
        repaymentRate: 0,
        vcWeight: 50,
        txWeight: 25,
        repaymentWeight: 25,
      });

      expect(result.vcScore).toBe(100);
    });

    it("caps txScore at 100", () => {
      const result = computeScoringComponents({
        vcCount: 0,
        txVolume30d: 50_000_000_000n, // 500 points → capped at 100
        avgCounterparties: 0,
        repaymentRate: 0,
        vcWeight: 50,
        txWeight: 25,
        repaymentWeight: 25,
      });

      expect(result.txScore).toBe(100);
    });

    it("caps the final score at MAX_SCORE", () => {
      const result = computeScoringComponents({
        vcCount: 100,
        txVolume30d: 100_000_000_000n,
        avgCounterparties: 100,
        repaymentRate: 10000,
        vcWeight: 50,
        txWeight: 25,
        repaymentWeight: 25,
      });

      expect(result.score).toBeLessThanOrEqual(MAX_SCORE);
    });

    it("floors the final score at MIN_SCORE", () => {
      const result = computeScoringComponents({
        vcCount: 0,
        txVolume30d: 0n,
        avgCounterparties: 0,
        repaymentRate: 0,
        vcWeight: 50,
        txWeight: 25,
        repaymentWeight: 25,
      });

      expect(result.score).toBeGreaterThanOrEqual(MIN_SCORE);
    });
  });

  // -----------------------------------------------------------------------
  // computeScoreCommitment
  // -----------------------------------------------------------------------
  describe("computeScoreCommitment", () => {
    it("returns a 32-byte Uint8Array", () => {
      const commitment = computeScoreCommitment(
        650,
        3,
        2_000_000n,
        5,
        8500,
        1_700_000_000,
        1_234_567,
        123456789n,
      );

      expect(commitment).toBeInstanceOf(Uint8Array);
      expect(commitment.length).toBe(32);
    });

    it("produces deterministic output for the same inputs", () => {
      const inputs: Parameters<typeof computeScoreCommitment> = [
        650, 3, 2_000_000n, 5, 8500, 1_700_000_000, 1_234_567, 123456789n,
      ];

      const a = computeScoreCommitment(...inputs);
      const b = computeScoreCommitment(...inputs);

      expect(Buffer.from(a).toString("hex")).toBe(
        Buffer.from(b).toString("hex"),
      );
    });

    it("produces different output when score changes", () => {
      const a = computeScoreCommitment(
        650, 3, 2_000_000n, 5, 8500, 1_700_000_000, 1_234_567, 123456789n,
      );
      const b = computeScoreCommitment(
        651, 3, 2_000_000n, 5, 8500, 1_700_000_000, 1_234_567, 123456789n,
      );

      expect(Buffer.from(a).toString("hex")).not.toBe(
        Buffer.from(b).toString("hex"),
      );
    });

    it("produces different output when blinding changes", () => {
      const a = computeScoreCommitment(
        650, 3, 2_000_000n, 5, 8500, 1_700_000_000, 1_234_567, 123456789n,
      );
      const b = computeScoreCommitment(
        650, 3, 2_000_000n, 5, 8500, 1_700_000_000, 1_234_567, 987654321n,
      );

      expect(Buffer.from(a).toString("hex")).not.toBe(
        Buffer.from(b).toString("hex"),
      );
    });
  });

  // -----------------------------------------------------------------------
  // generateBlinding
  // -----------------------------------------------------------------------
  describe("generateBlinding", () => {
    it("returns a bigint", () => {
      const blinding = generateBlinding();
      expect(typeof blinding).toBe("bigint");
    });

    it("generates unique values on successive calls", () => {
      const a = generateBlinding();
      const b = generateBlinding();
      expect(a).not.toBe(b);
    });

    it("is non-zero", () => {
      const blinding = generateBlinding();
      expect(blinding).not.toBe(0n);
    });
  });

  // -----------------------------------------------------------------------
  // collectWitness
  // -----------------------------------------------------------------------
  describe("collectWitness", () => {
    it("constructs a full witness from on-chain data", async () => {
      const scoreRecord = makeScoreRecord({ score: 650, vcCount: 3, repaymentRate: 8500, txVolume30d: 2_000_000n });
      const weights = makeScoringWeights();
      const sdk = makeMockSDK(scoreRecord, weights);

      const witness = await collectWitness(sdk, "GAAAAAAA...");

      expect(witness.score).toBe(650);
      expect(witness.vcCount).toBe(3);
      expect(witness.repaymentRate).toBe(8500);
      expect(witness.txVolume30d).toBe(2_000_000n);
      expect(witness.vcWeight).toBe(50);
      expect(witness.txWeight).toBe(25);
      expect(witness.repaymentWeight).toBe(25);
      expect(typeof witness.blinding).toBe("bigint");
    });

    it("calls getScore and getWeights on the SDK", async () => {
      const sdk = makeMockSDK();
      await collectWitness(sdk, "GAAAAAAA...");

      expect(sdk.getScore).toHaveBeenCalledWith("GAAAAAAA...");
      expect(sdk.getWeights).toHaveBeenCalled();
    });

    it("throws when no score is computed", async () => {
      const sdk = makeMockSDK(null);
      await expect(collectWitness(sdk, "GAAAAAAA...")).rejects.toThrow(
        "No score computed for GAAAAAAA...",
      );
    });

    it("populates intermediate component scores", async () => {
      const scoreRecord = makeScoreRecord({
        score: 712,
        vcCount: 5,
        txVolume30d: 5_000_000n,
        repaymentRate: 9200,
      });
      const sdk = makeMockSDK(scoreRecord);

      const witness = await collectWitness(sdk, "GAAAAAAA...");

      expect(witness.vcScore).toBe(100); // min(5 * 20, 100)
      expect(witness.txScore).toBe(0); // min(5_000_000n / 100_000_000n, 100) = min(0, 100) — integer division
      expect(witness.repayScore).toBe(92); // floor(9200 / 100)
    });
  });

  // -----------------------------------------------------------------------
  // generateScoreRangeProof
  // -----------------------------------------------------------------------
  describe("generateScoreRangeProof", () => {
    const mockPublicParams: ZKPublicParams = {
      verificationKeyUrl: "/path/to/vkey.json",
      circuitWasmUrl: "/path/to/circuit.wasm",
    };

    beforeEach(() => {
      mockGroth16FullProve.mockReset();
      mockGroth16Verify.mockReset();
    });

    it("generates a valid proof bundle", async () => {
      const mockProof = makeGroth16Proof();
      const mockPublicSignals = ["650", "1234567", "777777"];
      mockGroth16FullProve.mockResolvedValue({
        proof: mockProof,
        publicSignals: mockPublicSignals,
      });

      const witness = makeScoreWitness({ score: 650 });
      const result = await generateScoreRangeProof(witness, 500, mockPublicParams);

      expect(result.proof).toEqual(mockProof);
      expect(result.bundle.circuitVersion).toBe(1);
      expect(typeof result.bundle.proof).toBe("string");
      expect(result.publicInputs.threshold).toBe(500);
      expect(result.publicInputs.snapshotLedger).toBe(1_234_567);
    });

    it("passes circuit artifacts to snarkjs.groth16.fullProve", async () => {
      mockGroth16FullProve.mockResolvedValue({
        proof: makeGroth16Proof(),
        publicSignals: [],
      });

      const witness = makeScoreWitness({ score: 700 });
      await generateScoreRangeProof(witness, 600, mockPublicParams);

      expect(mockGroth16FullProve).toHaveBeenCalledWith(
        expect.any(Object),
        "/path/to/circuit.wasm",
        "/path/to/vkey.json",
      );
    });

    it("includes all witness fields as circuit inputs", async () => {
      mockGroth16FullProve.mockResolvedValue({
        proof: makeGroth16Proof(),
        publicSignals: [],
      });

      const witness = makeScoreWitness({ score: 650 });
      await generateScoreRangeProof(witness, 500, mockPublicParams);

      const circuitInput = mockGroth16FullProve.mock.calls[0]?.[0] as Record<string, unknown>;
      expect(circuitInput.score).toBe(650);
      expect(circuitInput.vc_count).toBe(3);
      expect(circuitInput.repayment_rate).toBe(8500);
      expect(circuitInput.threshold).toBe(500);
      expect(circuitInput.vc_score).toBe(60);
      expect(circuitInput.tx_score).toBe(20);
      expect(circuitInput.repay_score).toBe(85);
    });

    it("throws when score does not exceed threshold", async () => {
      const witness = makeScoreWitness({ score: 500 });
      await expect(
        generateScoreRangeProof(witness, 500, mockPublicParams),
      ).rejects.toThrow("does not exceed threshold");
    });

    it("throws when score is below threshold", async () => {
      const witness = makeScoreWitness({ score: 499 });
      await expect(
        generateScoreRangeProof(witness, 500, mockPublicParams),
      ).rejects.toThrow("does not exceed threshold");
    });

    it("uses custom circuitVersion from config", async () => {
      mockGroth16FullProve.mockResolvedValue({
        proof: makeGroth16Proof(),
        publicSignals: [],
      });

      const witness = makeScoreWitness({ score: 700 });
      const result = await generateScoreRangeProof(
        witness,
        600,
        mockPublicParams,
        { circuitVersion: 3 },
      );

      expect(result.bundle.circuitVersion).toBe(3);
    });

    it("includes the domain separator from public params", async () => {
      mockGroth16FullProve.mockResolvedValue({
        proof: makeGroth16Proof(),
        publicSignals: [],
      });

      const domainSep = new Uint8Array([1, 2, 3, 4]);
      const witness = makeScoreWitness({ score: 700 });
      const result = await generateScoreRangeProof(
        witness,
        600,
        { ...mockPublicParams, domainSeparator: domainSep },
      );

      expect(result.publicInputs.domainSeparator).toEqual(domainSep);
    });

    it("wraps snarkjs errors with descriptive messages", async () => {
      mockGroth16FullProve.mockRejectedValue(new Error("WASM load failed"));

      const witness = makeScoreWitness({ score: 700 });
      await expect(
        generateScoreRangeProof(witness, 600, mockPublicParams),
      ).rejects.toThrow("snarkjs proof generation failed");
    });

    it("encodes the proof as base64 in the bundle", async () => {
      const mockProof = makeGroth16Proof();
      mockGroth16FullProve.mockResolvedValue({
        proof: mockProof,
        publicSignals: [],
      });

      const witness = makeScoreWitness({ score: 700 });
      const result = await generateScoreRangeProof(
        witness,
        600,
        mockPublicParams,
      );

      const decoded = JSON.parse(
        Buffer.from(result.bundle.proof, "base64").toString("utf-8"),
      );
      expect(decoded).toEqual(mockProof);
    });

    it("generates a deterministic score commitment for the same witness", async () => {
      mockGroth16FullProve.mockResolvedValue({
        proof: makeGroth16Proof(),
        publicSignals: [],
      });

      const witness = makeScoreWitness({ score: 650 });
      const result1 = await generateScoreRangeProof(witness, 500, mockPublicParams);
      const result2 = await generateScoreRangeProof(witness, 500, mockPublicParams);

      // Same witness + same blinding → same commitment
      expect(Buffer.from(result1.publicInputs.scoreCommitment).toString("hex")).toBe(
        Buffer.from(result2.publicInputs.scoreCommitment).toString("hex"),
      );
    });
  });

  // -----------------------------------------------------------------------
  // verifyProof
  // -----------------------------------------------------------------------
  describe("verifyProof", () => {
    it("delegates to snarkjs.groth16.verify", async () => {
      mockGroth16Verify.mockResolvedValue(true);

      const vk = { protocol: "groth16" };
      const publicSignals = ["650", "1234567"];
      const proof = makeGroth16Proof();

      const result = await verifyProof(vk, publicSignals, proof);

      expect(result).toBe(true);
      expect(mockGroth16Verify).toHaveBeenCalledWith(vk, publicSignals, proof);
    });

    it("returns false for an invalid proof", async () => {
      mockGroth16Verify.mockResolvedValue(false);

      const result = await verifyProof({}, [], makeGroth16Proof());
      expect(result).toBe(false);
    });
  });

  // -----------------------------------------------------------------------
  // Type exports
  // -----------------------------------------------------------------------
  describe("type exports", () => {
    it("exports MIN_SCORE and MAX_SCORE constants", () => {
      expect(MIN_SCORE).toBe(300);
      expect(MAX_SCORE).toBe(850);
    });
  });
});
