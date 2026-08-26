import { buildKycCredential, hashVC } from "./hash";

describe("hashVC idempotency", () => {
  const input = {
    issuerDid:
      "did:stellar:testnet:GISSUER11111111111111111111111111111111111111111111111111",
    subjectDid:
      "did:stellar:testnet:GSUBJECT1111111111111111111111111111111111111111111111111",
    kycLevel: "basic",
    country: "NG",
    issuanceDate: "2026-06-28T12:00:00Z",
    verifiedAt: "2026-06-28T10:00:00Z",
  };

  it("produces the same hash for identical VC content", () => {
    const vc1 = buildKycCredential(input);
    const vc2 = buildKycCredential(input);
    expect(hashVC(vc1).toString("hex")).toBe(hashVC(vc2).toString("hex"));
  });

  it("produces a different hash when VC content changes", () => {
    const vc1 = buildKycCredential(input);
    const vc2 = buildKycCredential({ ...input, kycLevel: "enhanced" });
    expect(hashVC(vc1).toString("hex")).not.toBe(hashVC(vc2).toString("hex"));
  });

  it("returns a 32-byte SHA-256 digest", () => {
    const hash = hashVC(buildKycCredential(input));
    expect(hash).toHaveLength(32);
  });
});
