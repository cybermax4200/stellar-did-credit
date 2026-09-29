import { createHash } from "crypto";
import canonicalize from "canonicalize";

/** Input fields that fully determine a KYC credential's content hash. */
export interface KycCredentialInput {
  issuerDid: string;
  subjectDid: string;
  kycLevel: string;
  country: string;
  issuanceDate: string;
  verifiedAt: string;
}

/** Builds a minimal KYC Verifiable Credential matching docs/issuer-guide.md. */
export function buildKycCredential(input: KycCredentialInput): object {
  return {
    "@context": ["https://www.w3.org/2018/credentials/v1"],
    type: ["VerifiableCredential", "KYCCredential"],
    issuer: input.issuerDid,
    issuanceDate: input.issuanceDate,
    credentialSubject: {
      id: input.subjectDid,
      kycLevel: input.kycLevel,
      verifiedAt: input.verifiedAt,
      country: input.country,
    },
  };
}

/**
 * SHA-256 hash of the JCS-canonicalized VC bytes.
 * Matches docs/issuer-guide.md: canonicalize → utf8 → sha256 digest.
 */
export function hashVC(vc: object): Buffer {
  const canonical = canonicalize(vc);
  if (!canonical) {
    throw new Error("canonicalize returned undefined — check your VC object");
  }

  const vcHash = createHash("sha256")
    .update(Buffer.from(canonical, "utf8"))
    .digest();

  if (vcHash.length !== 32) {
    throw new Error(`Expected 32 bytes, got ${vcHash.length}`);
  }

  return vcHash;
}
