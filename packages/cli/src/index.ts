/**
 * Command-line interface for the Stellar DID Credit Protocol.
 *
 * Provides commands for anchoring DIDs, verifying credentials, computing and
 * reading credit scores, and querying protocol state (issuers, weights, VC
 * anchors, DID documents) — all backed by the on-chain Soroban contracts on
 * Stellar.
 *
 * @packageDocumentation
 */

import { Command } from "commander";
import { Keypair } from "@stellar/stellar-sdk";
import {
  StellarDIDCreditSDK,
  type VCRecord,
  type ScoringWeights,
} from "@stellar-did-credit/sdk";
import { loadConfig, type NetworkType } from "./config";

// ---------------------------------------------------------------------------
// Bootstrap
// ---------------------------------------------------------------------------

const program = new Command();

program
  .name("stellar-did")
  .description(
    "CLI for the Stellar DID Credit Protocol — anchor DIDs, check scores, " +
      "verify credentials, and compute credit scores on-chain.",
  )
  .version("0.1.0")
  .option(
    "--network <network>",
    "Stellar network to use (testnet, mainnet, futurenet)",
    "testnet"
  );

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/**
 * Reads a Stellar secret key string (starting with S) and returns a Keypair.
 * Prints an error and exits if the key is invalid.
 */
function parseSecret(secret: string): Keypair {
  try {
    return Keypair.fromSecret(secret);
  } catch {
    console.error(
      "Error: invalid Stellar secret key. Must be a 56-character string starting with 'S'.",
    );
    process.exit(1);
  }
}

/**
 * Validates a Stellar address (G... or C..., 56 base32 chars).
 */
function assertStellarAddress(label: string, addr: string): void {
  const upper = addr.toUpperCase();
  if (!/^[GC][A-Z2-7]{55}$/.test(upper)) {
    console.error(
      `Error: ${label} must be a valid Stellar address (G... or C..., 56 base32 characters). Got: ${addr}`,
    );
    process.exit(1);
  }
}

/**
 * Parses a hex-encoded VC hash into a 32-byte Buffer.
 */
function parseVcHash(hex: string): Buffer {
  if (hex.length !== 64 || !/^[0-9a-fA-F]{64}$/.test(hex)) {
    console.error(
      "Error: vc-hash must be a 64-character hex string (32 bytes).",
    );
    process.exit(1);
  }
  return Buffer.from(hex, "hex");
}

/**
 * Formats a BigInt value that represents stroops into a human-readable XLM
 * string (e.g., "100.0000000 XLM").
 */
function formatStroops(stroops: bigint): string {
  const abs = stroops < 0n ? -stroops : stroops;
  const xlm = Number(abs) / 10_000_000;
  const sign = stroops < 0n ? "-" : "";
  return `${sign}${xlm.toFixed(7)} XLM`;
}

/**
 * Formats a Unix timestamp (seconds) into an ISO-8601 string, or "N/A" if 0.
 */
function formatTimestamp(ts: number): string {
  if (ts === 0) return "N/A";
  return new Date(ts * 1000).toISOString();
}

/**
 * Print a ScoreRecord as a human-readable table to stdout.
 */
function printScoreRecord(record: {
  score: number;
  lastUpdated: number;
  vcCount: number;
  repaymentRate: number;
  txVolume30d: bigint;
  previousScore: number | null;
  computedAtLedger: number;
  stale: boolean;
}): void {
  console.log();
  console.log("┌─────────────────────────────────────┐");
  console.log(`│  Credit Score: ${String(record.score).padStart(3)}                   │`);
  console.log("├─────────────────────────────────────┤");
  console.log(
    `│  VC Count:            ${String(record.vcCount).padStart(13)} │`,
  );
  console.log(
    `│  Repayment Rate:      ${String(record.repaymentRate).padStart(10)} bps │`,
  );
  console.log(
    `│  TX Volume (30d):     ${formatStroops(record.txVolume30d).padStart(14)} │`,
  );
  console.log(
    `│  Previous Score:      ${record.previousScore !== null ? String(record.previousScore).padStart(10) : "N/A".padStart(13)} │`,
  );
  console.log(
    `│  Computed at Ledger:  ${String(record.computedAtLedger).padStart(10)} │`,
  );
  console.log(
    `│  Last Updated:        ${formatTimestamp(record.lastUpdated).slice(0, 19).padStart(13)} │`,
  );
  console.log(
    `│  Stale:               ${String(record.stale).padStart(13)} │`,
  );
  console.log("└─────────────────────────────────────┘");
  console.log();
}

/**
 * Print the scoring weights as a short human-readable table.
 */
function printWeights(weights: ScoringWeights): void {
  console.log();
  console.log("┌─────────────────────────────────────┐");
  console.log(`│  VC Weight:        ${String(weights.vcWeight).padStart(13)} │`);
  console.log(
    `│  TX Weight:        ${String(weights.txWeight).padStart(13)} │`,
  );
  console.log(
    `│  Repayment Weight: ${String(weights.repaymentWeight).padStart(13)} │`,
  );
  console.log("└─────────────────────────────────────┘");
  console.log();
}

/**
 * Print a list of credential anchors as a readable table.
 */
function printVCRecords(records: VCRecord[]): void {
  if (records.length === 0) {
    console.log();
    console.log("No verifiable credentials anchored for this subject.");
    return;
  }

  console.log();
  for (const record of records) {
    console.log("┌─────────────────────────────────────┐");
    console.log(
      `│  Hash:    ${record.vcHash.toString("hex").slice(0, 12)}…${record.vcHash.toString("hex").slice(-12)}`,
    );
    console.log(`│  Issuer:  ${record.issuer}`);
    console.log(
      `│  Anchored: ${formatTimestamp(record.anchoredAt).slice(0, 19).padStart(13)} │`,
    );
    console.log(
      `│  Revoked: ${String(record.revoked).padStart(14)} │`,
    );
    console.log("└─────────────────────────────────────┘");
  }
  console.log();
}

// ---------------------------------------------------------------------------
// Command: anchor-did
// ---------------------------------------------------------------------------

program
  .command("anchor-did")
  .description(
    "Anchor a DID document on-chain by storing its IPFS CID in the identity-oracle contract.",
  )
  .argument("<subject-secret>", "Stellar secret key of the DID subject (starts with S)")
  .argument("<did-doc-cid>", "IPFS CID of the DID document (e.g. Qm...)")
  .action(async (subjectSecret: string, didDocCid: string) => {
    const options = program.opts();
    const network = options.network as NetworkType;
    const config = loadConfig(network);
    const keypair = parseSecret(subjectSecret);
    const publicKey = keypair.publicKey();

    const sdk = new StellarDIDCreditSDK(config);

    console.log(`Anchoring DID for ${publicKey} on ${network}...`);
    console.log(`  DID Doc CID: ${didDocCid}`);

    try {
      const txHash = await sdk.anchorDID(keypair, didDocCid);
      console.log();
      console.log("Success!");
      console.log(`  Transaction: ${txHash}`);
      const explorerBase = network === 'mainnet' ? 'https://stellar.expert/explorer/public' : 'https://stellar.expert/explorer/testnet';
      console.log(
        `  Explorer:    ${explorerBase}/tx/${txHash}`,
      );
    } catch (err) {
      console.error(
        "Failed:",
        err instanceof Error ? err.message : err,
      );
      process.exit(1);
    }
  });

// ---------------------------------------------------------------------------
// Command: get-score
// ---------------------------------------------------------------------------

program
  .command("get-score")
  .description(
    "Fetch the on-chain credit score for a subject address from the credit-oracle.",
  )
  .argument("<subject-address>", "Stellar G... address of the subject")
  .option("--json", "Output the full ScoreRecord as JSON")
  .action(async (subjectAddress: string, options: { json?: boolean }) => {
    const globalOptions = program.opts();
    const network = globalOptions.network as NetworkType;
    const config = loadConfig(network);
    const upperAddr = subjectAddress.toUpperCase();
    assertStellarAddress("subject-address", upperAddr);

    const sdk = new StellarDIDCreditSDK(config);

    console.log(`Fetching credit score for ${upperAddr} on ${network}...`);

    try {
      const score = await sdk.getScore(upperAddr);

      if (!score) {
        console.log();
        console.log("No score computed yet for this address.");
        console.log(
          'Run "stellar-did compute-score" to compute one, or ask a feeder to sync data first.',
        );
        return;
      }

      if (options.json) {
        console.log(JSON.stringify(score, (key, value) => {
          if (typeof value === "bigint") return value.toString();
          return value;
        }, 2));
      } else {
        printScoreRecord(score);
      }
    } catch (err) {
      console.error(
        "Failed:",
        err instanceof Error ? err.message : err,
      );
      process.exit(1);
    }
  });

// ---------------------------------------------------------------------------
// Command: verify-vc
// ---------------------------------------------------------------------------

program
  .command("verify-vc")
  .description(
    "Verify that a specific verifiable credential hash is valid and non-revoked on-chain.",
  )
  .argument("<subject-address>", "Stellar G... address of the credential subject")
  .argument(
    "<vc-hash>",
    "SHA-256 hash of the verifiable credential (64 hex characters)",
  )
  .action(async (subjectAddress: string, vcHashHex: string) => {
    const options = program.opts();
    const network = options.network as NetworkType;
    const config = loadConfig(network);
    const upperAddr = subjectAddress.toUpperCase();
    assertStellarAddress("subject-address", upperAddr);
    const vcHash = parseVcHash(vcHashHex);

    const sdk = new StellarDIDCreditSDK(config);

    console.log(`Verifying VC for ${upperAddr} on ${network}...`);
    console.log(`  VC Hash: ${vcHashHex}`);

    try {
      const result = await sdk.verifyVC(upperAddr, vcHash);

      console.log();
      if (result) {
        console.log("✅ VC is VALID and non-revoked on-chain.");
      } else {
        console.log("❌ VC is NOT valid — either not found or has been revoked.");
      }
    } catch (err) {
      console.error(
        "Failed:",
        err instanceof Error ? err.message : err,
      );
      process.exit(1);
    }
  });

// ---------------------------------------------------------------------------
// Command: compute-score
// ---------------------------------------------------------------------------

program
  .command("compute-score")
  .description(
    "Compute and persist a credit score for a subject address on-chain. " +
      "Requires a funded keypair to pay transaction fees.",
  )
  .argument("<payer-secret>", "Stellar secret key of the fee payer (starts with S)")
  .argument("<subject-address>", "Stellar G... address of the subject")
  .option("--json", "Output the full ScoreRecord as JSON")
  .action(
    async (
      payerSecret: string,
      subjectAddress: string,
      options: { json?: boolean },
    ) => {
      const globalOptions = program.opts();
      const network = globalOptions.network as NetworkType;
      const config = loadConfig(network);
      const keypair = parseSecret(payerSecret);
      const upperAddr = subjectAddress.toUpperCase();
      assertStellarAddress("subject-address", upperAddr);

      const sdk = new StellarDIDCreditSDK(config);

      console.log(`Computing credit score for ${upperAddr} on ${network}...`);
      console.log(`  Payer: ${keypair.publicKey()}`);

      try {
        const score = await sdk.computeScore(keypair, upperAddr);

        if (options.json) {
          console.log(JSON.stringify(score, (key, value) => {
            if (typeof value === "bigint") return value.toString();
            return value;
          }, 2));
        } else {
          printScoreRecord(score);
        }
      } catch (err) {
        console.error(
          "Failed:",
          err instanceof Error ? err.message : err,
        );
        process.exit(1);
      }
    },
  );

// ---------------------------------------------------------------------------
// Command: is-verified
// ---------------------------------------------------------------------------

program
  .command("is-verified")
  .description(
    "Check whether a subject has at least one active, non-revoked verifiable credential.",
  )
  .argument("<subject-address>", "Stellar G... address of the subject")
  .action(async (subjectAddress: string) => {
    const options = program.opts();
    const network = options.network as NetworkType;
    const config = loadConfig(network);
    const upperAddr = subjectAddress.toUpperCase();
    assertStellarAddress("subject-address", upperAddr);

    const sdk = new StellarDIDCreditSDK(config);

    console.log(`Checking verification status for ${upperAddr} on ${network}...`);

    try {
      const verified = await sdk.isVerified(upperAddr);

      console.log();
      if (verified) {
        console.log("✅ Subject is VERIFIED.");
      } else {
        console.log("❌ Subject is NOT verified — no active credentials found.");
      }
    } catch (err) {
      console.error(
        "Failed:",
        err instanceof Error ? err.message : err,
      );
      process.exit(1);
    }
  });

// ---------------------------------------------------------------------------
// Command: vc-count
// ---------------------------------------------------------------------------

program
  .command("vc-count")
  .description(
    "Returns the number of active (non-revoked) verifiable credentials for a subject.",
  )
  .argument("<subject-address>", "Stellar G... address of the subject")
  .action(async (subjectAddress: string) => {
    const options = program.opts();
    const network = options.network as NetworkType;
    const config = loadConfig(network);
    const upperAddr = subjectAddress.toUpperCase();
    assertStellarAddress("subject-address", upperAddr);

    const sdk = new StellarDIDCreditSDK(config);

    console.log(`Fetching active VC count for ${upperAddr} on ${network}...`);

    try {
      const count = await sdk.getVCCount(upperAddr);

      console.log();
      console.log(`Active VC count: ${count}`);
    } catch (err) {
      console.error(
        "Failed:",
        err instanceof Error ? err.message : err,
      );
      process.exit(1);
    }
  });

// ---------------------------------------------------------------------------
// Command: vcs
// ---------------------------------------------------------------------------

program
  .command("vcs")
  .description(
    "List all verifiable credential anchors for a subject, including revoked entries.",
  )
  .argument("<subject-address>", "Stellar G... address of the subject")
  .action(async (subjectAddress: string) => {
    const options = program.opts();
    const network = options.network as NetworkType;
    const config = loadConfig(network);
    const upperAddr = subjectAddress.toUpperCase();
    assertStellarAddress("subject-address", upperAddr);

    const sdk = new StellarDIDCreditSDK(config);

    console.log(`Fetching verifiable credentials for ${upperAddr} on ${network}...`);

    try {
      const records = await sdk.getVCs(upperAddr);
      printVCRecords(records);
    } catch (err) {
      console.error(
        "Failed:",
        err instanceof Error ? err.message : err,
      );
      process.exit(1);
    }
  });

// ---------------------------------------------------------------------------
// Command: credential-type
// ---------------------------------------------------------------------------

program
  .command("credential-type")
  .description(
    "Fetch the credential type label anchored for a subject's VC hash (e.g. kyc, employment).",
  )
  .argument("<subject-address>", "Stellar G... address of the credential subject")
  .argument(
    "<vc-hash>",
    "SHA-256 hash of the verifiable credential (64 hex characters)",
  )
  .action(async (subjectAddress: string, vcHashHex: string) => {
    const options = program.opts();
    const network = options.network as NetworkType;
    const config = loadConfig(network);
    const upperAddr = subjectAddress.toUpperCase();
    assertStellarAddress("subject-address", upperAddr);
    const vcHash = parseVcHash(vcHashHex);

    const sdk = new StellarDIDCreditSDK(config);

    console.log(`Fetching credential type for ${upperAddr} on ${network}...`);
    console.log(`  VC Hash: ${vcHashHex}`);

    try {
      const credentialType = await sdk.getCredentialType(upperAddr, vcHash);

      console.log();
      console.log(`Credential type: ${credentialType}`);
    } catch (err) {
      console.error(
        "Failed:",
        err instanceof Error ? err.message : err,
      );
      process.exit(1);
    }
  });

// ---------------------------------------------------------------------------
// Command: did-doc
// ---------------------------------------------------------------------------

program
  .command("did-doc")
  .description(
    "Fetch the IPFS CID of the DID document anchored for a subject address.",
  )
  .argument("<subject-address>", "Stellar G... address of the subject")
  .action(async (subjectAddress: string) => {
    const options = program.opts();
    const network = options.network as NetworkType;
    const config = loadConfig(network);
    const upperAddr = subjectAddress.toUpperCase();
    assertStellarAddress("subject-address", upperAddr);

    const sdk = new StellarDIDCreditSDK(config);

    console.log(`Fetching DID document for ${upperAddr} on ${network}...`);

    try {
      const cid = await sdk.getDIDDocument(upperAddr);

      console.log();
      if (cid) {
        console.log(`DID Document CID: ${cid}`);
      } else {
        console.log("No DID document anchored for this address.");
      }
    } catch (err) {
      console.error(
        "Failed:",
        err instanceof Error ? err.message : err,
      );
      process.exit(1);
    }
  });

// ---------------------------------------------------------------------------
// Command: issuers
// ---------------------------------------------------------------------------

program
  .command("issuers")
  .description("List all currently registered trusted credential issuers.")
  .action(async () => {
    const options = program.opts();
    const network = options.network as NetworkType;
    const config = loadConfig(network);

    const sdk = new StellarDIDCreditSDK(config);

    console.log(`Fetching registered issuers on ${network}...`);

    try {
      const issuers = await sdk.getRegisteredIssuers();

      console.log();
      if (issuers.length === 0) {
        console.log("No issuers registered.");
        return;
      }
      issuers.forEach((issuer, index) => {
        console.log(`${index + 1}. ${issuer}`);
      });
    } catch (err) {
      console.error(
        "Failed:",
        err instanceof Error ? err.message : err,
      );
      process.exit(1);
    }
  });

// ---------------------------------------------------------------------------
// Command: weights
// ---------------------------------------------------------------------------

program
  .command("weights")
  .description(
    "Fetch the current scoring weights configured on the credit-oracle contract.",
  )
  .action(async () => {
    const options = program.opts();
    const network = options.network as NetworkType;
    const config = loadConfig(network);

    const sdk = new StellarDIDCreditSDK(config);

    console.log(`Fetching scoring weights on ${network}...`);

    try {
      const weights = await sdk.getWeights();
      printWeights(weights);
    } catch (err) {
      console.error(
        "Failed:",
        err instanceof Error ? err.message : err,
      );
      process.exit(1);
    }
  });

// ---------------------------------------------------------------------------
// Parse
// ---------------------------------------------------------------------------

// If this file was invoked directly (not imported), run the CLI.
if (require.main === module) {
  program.parse();
}
