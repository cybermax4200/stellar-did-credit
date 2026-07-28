/**
 * Command-line interface for the Stellar DID Credit Protocol.
 *
 * Provides commands for anchoring DIDs, checking credit scores, verifying
 * credentials, and computing credit scores — all backed by the on-chain
 * Soroban contracts on Stellar.
 *
 * @packageDocumentation
 */

import { Command } from "commander";
import {
  Keypair,
  Contract,
  SorobanRpc,
  TransactionBuilder,
  BASE_FEE,
  Account,
  scValToNative,
  nativeToScVal,
  Address,
} from "@stellar/stellar-sdk";
import { StellarDIDCreditSDK } from "@stellar-did-credit/sdk";
import { loadConfig } from "./config";

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
  .version("0.1.0");

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
    const config = loadConfig();
    const keypair = parseSecret(subjectSecret);
    const publicKey = keypair.publicKey();

    const sdk = new StellarDIDCreditSDK(config);

    console.log(`Anchoring DID for ${publicKey}...`);
    console.log(`  DID Doc CID: ${didDocCid}`);

    try {
      const txHash = await sdk.anchorDID(keypair, didDocCid);
      console.log();
      console.log("Success!");
      console.log(`  Transaction: ${txHash}`);
      console.log(
        `  Explorer:    https://stellar.expert/explorer/testnet/tx/${txHash}`,
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
    const config = loadConfig();
    const upperAddr = subjectAddress.toUpperCase();
    assertStellarAddress("subject-address", upperAddr);

    const sdk = new StellarDIDCreditSDK(config);

    console.log(`Fetching credit score for ${upperAddr}...`);

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
    const config = loadConfig();
    const upperAddr = subjectAddress.toUpperCase();
    assertStellarAddress("subject-address", upperAddr);
    const vcHash = parseVcHash(vcHashHex);

    const server = new SorobanRpc.Server(config.rpcUrl);
    const contract = new Contract(config.identityOracleId);
    const sourceAccount = new Account(config.simAccount, "0");

    console.log(`Verifying VC for ${upperAddr}...`);
    console.log(`  VC Hash: ${vcHashHex}`);

    try {
      const hashScVal = nativeToScVal(new Uint8Array(vcHash), { type: "bytes" });

      const tx = new TransactionBuilder(sourceAccount, {
        fee: config.baseFee || BASE_FEE,
        networkPassphrase: config.networkPassphrase,
      })
        .addOperation(
          contract.call(
            "verify_vc",
            new Address(upperAddr).toScVal(),
            hashScVal,
          ),
        )
        .setTimeout(config.timeoutSeconds ?? 30)
        .build();

      const sim = await server.simulateTransaction(tx);

      if (SorobanRpc.Api.isSimulationError(sim)) {
        throw new Error(`Simulation failed: ${sim.error}`);
      }

      if (!SorobanRpc.Api.isSimulationSuccess(sim)) {
        throw new Error("Simulation returned unexpected response");
      }

      const result = scValToNative(sim.result!.retval) as boolean;

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
      const config = loadConfig();
      const keypair = parseSecret(payerSecret);
      const upperAddr = subjectAddress.toUpperCase();
      assertStellarAddress("subject-address", upperAddr);

      const sdk = new StellarDIDCreditSDK(config);

      console.log(`Computing credit score for ${upperAddr}...`);
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
// Parse
// ---------------------------------------------------------------------------

// If this file was invoked directly (not imported), run the CLI.
if (require.main === module) {
  program.parse();
}
