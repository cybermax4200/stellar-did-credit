/**
 * Minimal issuer script for the stellar-did-credit protocol.
 *
 * Usage:
 *   ISSUER_SECRET=YOUR_STELLAR_SECRET_KEY npm run issue -- --subject GSUBJECT... --kyc-level basic --country NG
 *
 * Required environment variables:
 *   ISSUER_SECRET        — Stellar secret key of a registered issuer (starts with S)
 *   IDENTITY_ORACLE_ID   — Contract address of the identity-oracle
 *   CREDIT_ORACLE_ID     — Contract address of the credit-oracle
 *   REVOCATION_REG_ID    — Contract address of the revocation-registry
 *
 * Optional environment variables:
 *   NETWORK_PASSPHRASE   — Defaults to Stellar testnet passphrase
 *   RPC_URL              — Defaults to the public Soroban testnet RPC
 *   SIM_ACCOUNT          — Any funded testnet account used as fee source for read-only sims
 */

import { createHash } from "crypto";
import { Keypair } from "@stellar/stellar-sdk";
import canonicalize from "canonicalize";
import { StellarDIDCreditSDK } from "@stellar-did-credit/sdk";

// ---------------------------------------------------------------------------
// CLI argument parsing
// ---------------------------------------------------------------------------

function parseArgs(argv: string[]): Record<string, string> {
  const args: Record<string, string> = {};
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg.startsWith("--")) {
      const key = arg.slice(2);
      const value = argv[i + 1] ?? "";
      args[key] = value;
      i++;
    }
  }
  return args;
}

const args = parseArgs(process.argv.slice(2));

const subjectAddress = args["subject"];
const kycLevel = args["kyc-level"] ?? "basic";
const country = args["country"] ?? "XX";

if (!subjectAddress) {
  console.error("Error: --subject <G...> is required");
  process.exit(1);
}

// ---------------------------------------------------------------------------
// Environment configuration
// ---------------------------------------------------------------------------

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value) {
    console.error(`Error: environment variable ${name} is not set`);
    process.exit(1);
  }
  return value;
}

const issuerSecret = requireEnv("ISSUER_SECRET");
const identityOracleId =
  process.env["IDENTITY_ORACLE_ID"] ??
  "CXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX";
const creditOracleId =
  process.env["CREDIT_ORACLE_ID"] ??
  "CXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX";
const revocationRegId =
  process.env["REVOCATION_REG_ID"] ??
  "CXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX";
const networkPassphrase =
  process.env["NETWORK_PASSPHRASE"] ?? "Test SDF Network ; September 2015";
const rpcUrl =
  process.env["RPC_URL"] ?? "https://soroban-testnet.stellar.org";
const simAccount =
  process.env["SIM_ACCOUNT"] ??
  "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";

// ---------------------------------------------------------------------------
// Build the Verifiable Credential JSON-LD document
// ---------------------------------------------------------------------------

const issuerKeypair = Keypair.fromSecret(issuerSecret);
const issuerAddress = issuerKeypair.publicKey();
const issuerDid = `did:stellar:testnet:${issuerAddress}`;
const subjectDid = `did:stellar:testnet:${subjectAddress}`;

const vc = {
  "@context": ["https://www.w3.org/2018/credentials/v1"],
  type: ["VerifiableCredential", "KYCCredential"],
  issuer: issuerDid,
  issuanceDate: new Date().toISOString(),
  credentialSubject: {
    id: subjectDid,
    kycLevel,
    country,
    verifiedAt: new Date().toISOString(),
  },
};

console.log("\nVerifiable Credential:");
console.log(JSON.stringify(vc, null, 2));

// ---------------------------------------------------------------------------
// Canonicalize (RFC 8785 JCS) and SHA-256 hash the credential
// ---------------------------------------------------------------------------

const canonical = canonicalize(vc);
if (!canonical) {
  throw new Error("canonicalize() returned undefined — check your VC object");
}

console.log("\nCanonical form:");
console.log(canonical);

const vcHash: Buffer = createHash("sha256")
  .update(Buffer.from(canonical, "utf8"))
  .digest();

console.log("\nSHA-256 hash (hex):", vcHash.toString("hex"));

// ---------------------------------------------------------------------------
// Anchor the hash on-chain via the SDK
// ---------------------------------------------------------------------------

async function main(): Promise<void> {
  const sdk = new StellarDIDCreditSDK({
    identityOracleId,
    creditOracleId,
    revocationRegistryId: revocationRegId,
    networkPassphrase,
    rpcUrl,
    simAccount,
  });

  console.log("\nAnchoring credential hash on-chain...");
  console.log("  Issuer :", issuerAddress);
  console.log("  Subject:", subjectAddress);

  const txHash = await sdk.issueVC(issuerKeypair, subjectAddress, vcHash);

  console.log("\nSuccess!");
  console.log("  Transaction hash:", txHash);
  console.log(
    "  View on explorer:",
    `https://stellar.expert/explorer/testnet/tx/${txHash}`
  );

  // Verify the anchor is readable
  const confirmed = await sdk.verifyVC(subjectAddress, vcHash);
  console.log("  Verified on-chain:", confirmed);

  // Print the off-chain VC and its hash — the issuer should store both
  console.log("\n--- Store this record off-chain ---");
  console.log({
    vcHash: vcHash.toString("hex"),
    txHash,
    subject: subjectAddress,
    issuer: issuerAddress,
    anchoredAt: new Date().toISOString(),
    vc,
  });
}

main().catch((err) => {
  console.error("\nFailed:", err instanceof Error ? err.message : err);
  process.exit(1);
});
