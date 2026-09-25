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

import { Command, InvalidArgumentError } from "commander";
import {
  Account,
  Address,
  BASE_FEE,
  Contract,
  Keypair,
  nativeToScVal,
  scValToNative,
  SorobanRpc,
  TransactionBuilder,
  xdr,
} from "@stellar/stellar-sdk";
import {
  StellarDIDCreditSDK,
  scoringWeightsToScVal,
  type VCRecord,
  type ScoringWeights,
} from "@stellar-did-credit/sdk";
import { loadConfig, validateConfig, type NetworkType } from "./config";
import { readBatchCsv, writeBatchResults, type BatchResult } from "./batch";

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

function parseBatchVcHash(hex: string): Buffer {
  if (hex.length !== 64 || !/^[0-9a-fA-F]{64}$/.test(hex)) {
    throw new Error("vc_hash_hex must be a 64-character hex string (32 bytes)");
  }
  return Buffer.from(hex, "hex");
}

function validateBatchSubject(subject: string): string {
  const upper = subject.toUpperCase();
  if (!/^[G][A-Z2-7]{55}$/.test(upper)) {
    throw new Error("subject must be a valid Stellar G... address");
  }
  return upper;
}

function sleep(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
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
 * Simulates a contract transaction for --dry-run mode without broadcasting.
 */
async function simulateDryRun(params: {
  rpcUrl: string;
  networkPassphrase: string;
  sourceKeypair: Keypair;
  contractId: string;
  operation: xdr.Operation;
  label: string;
}): Promise<void> {
  const { rpcUrl, networkPassphrase, sourceKeypair, contractId, operation, label } = params;
  const server = new SorobanRpc.Server(rpcUrl);
  const publicKey = sourceKeypair.publicKey();

  console.log(`\nSimulating ${label} (--dry-run)...`);
  console.log(`  Source:   ${publicKey}`);
  console.log(`  Contract: ${contractId}`);

  try {
    let sourceAccount: Account;
    try {
      const accountData = await server.getAccount(publicKey);
      sourceAccount = new Account(publicKey, accountData.sequenceNumber());
    } catch {
      sourceAccount = new Account(publicKey, "0");
    }

    const tx = new TransactionBuilder(sourceAccount, {
      fee: BASE_FEE,
      networkPassphrase,
    })
      .addOperation(operation)
      .setTimeout(30)
      .build();

    const sim = await server.simulateTransaction(tx);

    if (SorobanRpc.Api.isSimulationError(sim)) {
      console.error(`\n❌ Simulation Failed:`);
      console.error(`  Error: ${sim.error || "Simulation returned error"}`);
      process.exit(1);
      return;
    }

    if (!SorobanRpc.Api.isSimulationSuccess(sim)) {
      console.error(`\n❌ Simulation Failed:`);
      console.error(`  Error: Unexpected simulation response`);
      process.exit(1);
      return;
    }

    const gasCost = sim.minResourceFee ? `${sim.minResourceFee} stroops` : "0 stroops";
    console.log(`\n✅ Simulation Successful (Transaction will succeed without broadcasting)`);
    console.log(`  Simulated Gas Cost (minResourceFee): ${gasCost}`);
    if (sim.cost) {
      console.log(`  CPU Instructions: ${sim.cost.cpuInsns}`);
      console.log(`  Memory: ${sim.cost.memBytes} bytes`);
    }

    if (sim.result?.retval) {
      try {
        const val = scValToNative(sim.result.retval);
        console.log(`  Expected Result: ${typeof val === "object" ? JSON.stringify(val) : val}`);
      } catch {
        console.log(`  Expected Result: success`);
      }
    } else {
      console.log(`  Expected Result: success (void)`);
    }

    process.exit(0);
  } catch (err) {
    console.error(`\n❌ Simulation Failed:`, err instanceof Error ? err.message : String(err));
    process.exit(1);
  }
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
  .option("--dry-run", "Simulate transaction execution without broadcasting")
  .action(async (subjectSecret: string, didDocCid: string, cmdOptions: { dryRun?: boolean }) => {
    const options = program.opts();
    const network = options.network as NetworkType;
    const config = loadConfig(network);
    validateConfig(config, ['identityOracleId']);
    const keypair = parseSecret(subjectSecret);
    const publicKey = keypair.publicKey();

    if (cmdOptions.dryRun) {
      const contract = new Contract(config.identityOracleId!);
      const operation = contract.call(
        "anchor_did",
        new Address(publicKey).toScVal(),
        nativeToScVal(didDocCid),
      );
      await simulateDryRun({
        rpcUrl: config.rpcUrl,
        networkPassphrase: config.networkPassphrase,
        sourceKeypair: keypair,
        contractId: config.identityOracleId!,
        operation,
        label: "anchor-did",
      });
      return;
    }

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
    validateConfig(config, ['creditOracleId'], true);
    const upperAddr = subjectAddress.toUpperCase();
    assertStellarAddress("subject-address", upperAddr);

    const sdk = new StellarDIDCreditSDK(config);

    if (!options.json) {
      console.log(`Fetching credit score for ${upperAddr} on ${network}...`);
    }

    try {
      const score = await sdk.getScore(upperAddr);

      if (score === null) {
        if (options.json) {
          console.log(JSON.stringify({ score: null }));
        } else {
          console.log();
          console.log(
            "No credit score has been computed for this subject yet. Run `compute-score` first.",
          );
        }
        process.exit(0);
        return;
      }

      if (options.json) {
        console.log(JSON.stringify(score, (key, value) => {
          if (typeof value === "bigint") return value.toString();
          return value;
        }, 2));
        process.exit(0);
        return;
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
    validateConfig(config, ['identityOracleId'], true);
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
  .option("--dry-run", "Simulate transaction execution without broadcasting")
  .action(
    async (
      payerSecret: string,
      subjectAddress: string,
      cmdOptions: { json?: boolean; dryRun?: boolean },
    ) => {
      const globalOptions = program.opts();
      const network = globalOptions.network as NetworkType;
      const config = loadConfig(network);
      validateConfig(config, ['creditOracleId']);
      const keypair = parseSecret(payerSecret);
      const upperAddr = subjectAddress.toUpperCase();
      assertStellarAddress("subject-address", upperAddr);

      if (cmdOptions.dryRun) {
        const contract = new Contract(config.creditOracleId!);
        const operation = contract.call(
          "compute_score",
          new Address(upperAddr).toScVal(),
        );
        await simulateDryRun({
          rpcUrl: config.rpcUrl,
          networkPassphrase: config.networkPassphrase,
          sourceKeypair: keypair,
          contractId: config.creditOracleId!,
          operation,
          label: "compute-score",
        });
        return;
      }

      const sdk = new StellarDIDCreditSDK(config);

      console.log(`Computing credit score for ${upperAddr} on ${network}...`);
      console.log(`  Payer: ${keypair.publicKey()}`);

      try {
        const score = await sdk.computeScore(keypair, upperAddr);

        if (cmdOptions.json) {
          console.log(JSON.stringify({ score }, null, 2));
        } else {
          console.log(`\n✅ Computed Score: ${score}`);
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
  .option("--json", "Output as JSON")
  .action(async (subjectAddress: string, cmdOptions: { json?: boolean }) => {
    const options = program.opts();
    const network = options.network as NetworkType;
    const config = loadConfig(network);
    validateConfig(config, ['identityOracleId'], true);
    const upperAddr = subjectAddress.toUpperCase();
    assertStellarAddress("subject-address", upperAddr);

    const sdk = new StellarDIDCreditSDK(config);

    if (!cmdOptions.json) {
      console.log(`Checking verification status for ${upperAddr} on ${network}...`);
    }

    try {
      const verified = await sdk.isVerified(upperAddr);

      if (cmdOptions.json) {
        console.log(JSON.stringify({ isVerified: verified }));
        process.exit(0);
      } else {
        console.log();
        if (verified) {
          console.log("✅ Subject is VERIFIED.");
        } else {
          console.log("❌ Subject is NOT verified — no active credentials found.");
        }
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
  .option("--json", "Output as JSON")
  .action(async (subjectAddress: string, cmdOptions: { json?: boolean }) => {
    const options = program.opts();
    const network = options.network as NetworkType;
    const config = loadConfig(network);
    validateConfig(config, ['identityOracleId'], true);
    const upperAddr = subjectAddress.toUpperCase();
    assertStellarAddress("subject-address", upperAddr);

    const sdk = new StellarDIDCreditSDK(config);

    if (!cmdOptions.json) {
      console.log(`Fetching active VC count for ${upperAddr} on ${network}...`);
    }

    try {
      const count = await sdk.getVCCount(upperAddr);

      if (cmdOptions.json) {
        console.log(JSON.stringify({ vcCount: count }));
        process.exit(0);
      } else {
        console.log();
        console.log(`Active VC count: ${count}`);
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
    validateConfig(config, ['identityOracleId'], true);
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
    validateConfig(config, ['identityOracleId'], true);
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
  .option("--json", "Output as JSON")
  .action(async (subjectAddress: string, cmdOptions: { json?: boolean }) => {
    const options = program.opts();
    const network = options.network as NetworkType;
    const config = loadConfig(network);
    const upperAddr = subjectAddress.toUpperCase();
    assertStellarAddress("subject-address", upperAddr);

    const sdk = new StellarDIDCreditSDK(config);

    if (!cmdOptions.json) {
      console.log(`Fetching DID document for ${upperAddr} on ${network}...`);
    }

    try {
      const cid = await sdk.getDIDDocument(upperAddr);

      if (cmdOptions.json) {
        console.log(JSON.stringify({ didDocument: cid || null }));
        process.exit(0);
      } else {
        console.log();
        if (cid) {
          console.log(`DID Document CID: ${cid}`);
        } else {
          console.log("No DID document anchored for this address.");
        }
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
    validateConfig(config, ['identityOracleId'], true);

    const sdk = new StellarDIDCreditSDK(config);

    console.log(`Fetching registered issuers on ${network}...`);

    try {
      const issuers = await sdk.listIssuers();

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
    validateConfig(config, ['creditOracleId'], true);

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
// Command: anchor-vc
// ---------------------------------------------------------------------------

/**
 * Anchor a verifiable credential hash on-chain.
 */
program
  .command("anchor-vc")
  .description("Anchor a verifiable credential hash on-chain as a registered issuer.")
  .argument("<issuer-secret>", "Stellar secret key of the registered issuer (starts with S)")
  .argument("<subject-address>", "Stellar G... address of the credential subject")
  .argument("<vc-hash>", "SHA-256 hash of the verifiable credential (64 hex characters)")
  .option("--type <type>", "Optional credential type label (e.g. kyc, employment)")
  .option("--dry-run", "Simulate transaction execution without broadcasting")
  .addHelpText(
    "after",
    `
Example:
  $ stellar-did anchor-vc S... G... 5c4146... --type kyc
`
  )
  .action(async (issuerSecret: string, subjectAddress: string, vcHashHex: string, cmdOptions: { type?: string; dryRun?: boolean }) => {
    const options = program.opts();
    const network = options.network as NetworkType;
    const config = loadConfig(network);
    validateConfig(config, ['identityOracleId']);
    
    const keypair = parseSecret(issuerSecret);
    const upperAddr = subjectAddress.toUpperCase();
    assertStellarAddress("subject-address", upperAddr);
    const vcHash = parseVcHash(vcHashHex);

    if (cmdOptions.dryRun) {
      const contract = new Contract(config.identityOracleId!);
      const hashScVal = nativeToScVal(new Uint8Array(vcHash), { type: "bytes" });
      const operation = cmdOptions.type
        ? contract.call(
            "anchor_vc_typed",
            new Address(keypair.publicKey()).toScVal(),
            new Address(upperAddr).toScVal(),
            hashScVal,
            nativeToScVal(cmdOptions.type),
          )
        : contract.call(
            "anchor_vc",
            new Address(keypair.publicKey()).toScVal(),
            new Address(upperAddr).toScVal(),
            hashScVal,
          );
      await simulateDryRun({
        rpcUrl: config.rpcUrl,
        networkPassphrase: config.networkPassphrase,
        sourceKeypair: keypair,
        contractId: config.identityOracleId!,
        operation,
        label: "anchor-vc",
      });
      return;
    }

    const sdk = new StellarDIDCreditSDK(config);

    console.log(`Anchoring VC for ${upperAddr} on ${network}...`);
    console.log(`  Issuer:  ${keypair.publicKey()}`);
    console.log(`  VC Hash: ${vcHashHex}`);
    if (cmdOptions.type) {
      console.log(`  Type:    ${cmdOptions.type}`);
    }

    try {
      // @ts-ignore
      const txHash = await sdk.issueVC(keypair, upperAddr, vcHash, cmdOptions.type);

      console.log();
      console.log("Success!");
      console.log(`  Transaction: ${txHash}`);
      const explorerBase = network === 'mainnet' ? 'https://stellar.expert/explorer/public' : 'https://stellar.expert/explorer/testnet';
      console.log(`  Explorer:    ${explorerBase}/tx/${txHash}`);
    } catch (err) {
      let msg = err instanceof Error ? err.message : String(err);
      if (msg.includes("IssuerNotRegistered")) {
         msg = "IssuerNotRegistered. Hint: Ensure this issuer is registered with the admin.";
      } else if (msg.includes("DuplicateVC")) {
         msg = "DuplicateVC. This VC hash has already been anchored.";
      }
      console.error("Failed:", msg);
      process.exit(1);
    }
  });

// ---------------------------------------------------------------------------
// Command: batch-anchor-vc
// ---------------------------------------------------------------------------

program
  .command("batch-anchor-vc")
  .description("Anchor verifiable credentials from a CSV file, one at a time.")
  .argument("<csv-file>", "CSV containing subject,vc_hash_hex,credential_type")
  .requiredOption(
    "--issuer-secret <secret>",
    "Stellar secret key of the registered issuer (starts with S)",
    process.env["ISSUER_SECRET"],
  )
  .option("--delay-ms <milliseconds>", "Delay between transactions", "200")
  .option("--result-file <path>", "Output JSON result path", "batch-result.json")
  .action(async (
    csvFile: string,
    cmdOptions: { issuerSecret: string; delayMs: string; resultFile: string },
  ) => {
    const options = program.opts();
    const network = options.network as NetworkType;
    const config = loadConfig(network);
    validateConfig(config, ["identityOracleId"]);
    const issuer = parseSecret(cmdOptions.issuerSecret);
    const delayMs = Number(cmdOptions.delayMs);
    if (!Number.isInteger(delayMs) || delayMs < 0) {
      console.error("Error: delay-ms must be a non-negative integer.");
      process.exit(1);
    }

    let entries;
    try {
      entries = readBatchCsv(csvFile);
    } catch (err) {
      console.error("Failed to read CSV:", err instanceof Error ? err.message : String(err));
      process.exit(1);
      return;
    }

    const sdk = new StellarDIDCreditSDK(config);
    const results: BatchResult[] = [];
    console.log(`Processing ${entries.length} VC${entries.length === 1 ? "" : "s"} on ${network}...`);

    for (const [index, entry] of entries.entries()) {
      const progress = `${index + 1}/${entries.length}`;
      const resultBase = { subject: entry.subject, vc_hash: entry.vcHashHex };
      try {
        const upperSubject = validateBatchSubject(entry.subject);
        const vcHash = parseBatchVcHash(entry.vcHashHex);

        if (await sdk.verifyVC(upperSubject, vcHash)) {
          results.push({ ...resultBase, status: "skipped" });
          console.log(`[${progress}] skipped ${upperSubject} (already anchored)`);
        } else {
          const txHash = await sdk.issueVC(issuer, upperSubject, vcHash);
          results.push({ ...resultBase, status: "success", txHash });
          console.log(`[${progress}] success ${upperSubject} (${entry.credentialType})`);
        }
      } catch (err) {
        const error = err instanceof Error ? err.message : String(err);
        results.push({ ...resultBase, status: "failed", error });
        console.error(`[${progress}] failed ${entry.subject}: ${error}`);
      }

      writeBatchResults(cmdOptions.resultFile, results);
      if (index < entries.length - 1 && delayMs > 0) {
        await sleep(delayMs);
      }
    }

    const summary = results.reduce(
      (counts, result) => ({ ...counts, [result.status]: counts[result.status] + 1 }),
      { success: 0, failed: 0, skipped: 0 },
    );
    console.log(`Completed: ${summary.success} succeeded, ${summary.skipped} skipped, ${summary.failed} failed.`);
    console.log(`Results written to ${cmdOptions.resultFile}`);
  });

program
  .command("prove-score")
  .description("Generate and submit a zero-knowledge proof for a credit score threshold")
  .requiredOption("-s, --subject <address>", "Subject public key")
  .requiredOption("-t, --threshold <number>", "Threshold score to prove", parseFloat)
  .requiredOption("-v, --verifier <contractId>", "Contract ID of the score-range-verifier")
  .option("-b, --blinding <number>", "Blinding factor (default: random)", parseFloat)
  .action(async (cmdOptions) => {
    const options = program.opts();
    const network = options.network as NetworkType;
    const config = loadConfig(network);
    validateConfig(config, ['identityOracleId', 'creditOracleId', 'rpcUrl', 'networkPassphrase'], false);
    
    const sdk = new StellarDIDCreditSDK(config);
    const secretKey = process.env.STELLAR_SECRET || config.simAccount;
    const kp = parseSecret(secretKey);
    const subject = cmdOptions.subject.toUpperCase();
    assertStellarAddress("subject", subject);

    const blinding = cmdOptions.blinding !== undefined ? cmdOptions.blinding : Math.floor(Math.random() * 1000000);

    console.log(`Generating score proof for ${subject} with threshold ${cmdOptions.threshold}...`);
    let proof: Uint8Array;
    try {
      proof = await sdk.generateScoreProof(subject, cmdOptions.threshold, blinding);
      console.log(`Proof generated successfully. (${proof.length} bytes)`);
    } catch (err: unknown) {
      console.error(`Error generating proof: ${err instanceof Error ? err.message : String(err)}`);
      process.exit(1);
    }

    console.log(`Submitting proof to verifier contract ${cmdOptions.verifier}...`);
    try {
      const result = await sdk.verifyScoreProof(
        kp,
        subject,
        cmdOptions.threshold,
        proof,
        cmdOptions.verifier
      );
      if (result) {
        console.log(`Proof successfully verified by the contract!`);
      } else {
        console.log(`Proof verification failed!`);
        process.exit(1);
      }
    } catch (err: unknown) {
      console.error(`Error verifying proof: ${err instanceof Error ? err.message : String(err)}`);
      process.exit(1);
    }
  });

// ---------------------------------------------------------------------------
// Command group: governance
// ---------------------------------------------------------------------------

const governance = program
  .command("governance")
  .description("Protocol governance commands.");

governance
  .command("create-proposal")
  .description("Create a scoring-weight proposal.")
  .argument("<proposer-secret>", "Stellar secret key of the proposer (starts with S)")
  .argument("<vc-weight>", "VC score weight (0-100)", parseInt)
  .argument("<tx-weight>", "Transaction history score weight (0-100)", parseInt)
  .argument("<repay-weight>", "Repayment score weight (0-100)", parseInt)
  .option("--voting-period <ledgers>", "Voting period in ledgers", parseInt, 17280)
  .option("--delay <ledgers>", "Execution delay in ledgers", parseInt, 0)
  .option("--dry-run", "Simulate transaction execution without broadcasting")
  .action(
    async (
      proposerSecret: string,
      vcWeight: number,
      txWeight: number,
      repayWeight: number,
      cmdOptions: { votingPeriod: number; delay: number; dryRun?: boolean },
    ) => {
      const globalOptions = program.opts();
      const network = globalOptions.network as NetworkType;
      const config = loadConfig(network);
      validateConfig(config, ['governanceId']);
      const keypair = parseSecret(proposerSecret);

      const weights: ScoringWeights = {
        vcWeight,
        txWeight,
        repaymentWeight: repayWeight,
      };

      if (cmdOptions.dryRun) {
        const contract = new Contract(config.governanceId!);
        const operation = contract.call(
          "create_proposal",
          new Address(keypair.publicKey()).toScVal(),
          scoringWeightsToScVal(weights),
          nativeToScVal(cmdOptions.votingPeriod, { type: "u32" }),
          nativeToScVal(cmdOptions.delay, { type: "u32" }),
        );
        await simulateDryRun({
          rpcUrl: config.rpcUrl,
          networkPassphrase: config.networkPassphrase,
          sourceKeypair: keypair,
          contractId: config.governanceId!,
          operation,
          label: "governance create-proposal",
        });
        return;
      }

      const sdk = new StellarDIDCreditSDK(config);
      console.log(`Creating governance proposal on ${network}...`);
      console.log(`  Proposer: ${keypair.publicKey()}`);

      try {
        const proposalId = await sdk.governance.createProposal(
          keypair,
          weights,
          cmdOptions.votingPeriod,
          cmdOptions.delay,
        );
        console.log();
        console.log("Success!");
        console.log(`  Proposal ID: ${proposalId.toString()}`);
      } catch (err) {
        console.error("Failed:", err instanceof Error ? err.message : err);
        process.exit(1);
      }
    },
  );

governance
  .command("execute")
  .description("Execute an approved governance proposal.")
  .argument("<payer-secret>", "Stellar secret key of the fee payer (starts with S)")
  .argument("<proposal-id>", "ID of the proposal to execute")
  .option("--dry-run", "Simulate transaction execution without broadcasting")
  .action(
    async (
      payerSecret: string,
      proposalId: string,
      cmdOptions: { dryRun?: boolean },
    ) => {
      const globalOptions = program.opts();
      const network = globalOptions.network as NetworkType;
      const config = loadConfig(network);
      validateConfig(config, ['governanceId']);
      const keypair = parseSecret(payerSecret);

      if (cmdOptions.dryRun) {
        const contract = new Contract(config.governanceId!);
        const operation = contract.call(
          "execute",
          nativeToScVal(BigInt(proposalId), { type: "u64" }),
        );
        await simulateDryRun({
          rpcUrl: config.rpcUrl,
          networkPassphrase: config.networkPassphrase,
          sourceKeypair: keypair,
          contractId: config.governanceId!,
          operation,
          label: "governance execute",
        });
        return;
      }

      const sdk = new StellarDIDCreditSDK(config);
      console.log(`Executing governance proposal ${proposalId} on ${network}...`);

      try {
        const txHash = await sdk.governance.execute(keypair, proposalId);
        console.log();
        console.log("Success!");
        console.log(`  Transaction: ${txHash}`);
      } catch (err) {
        console.error("Failed:", err instanceof Error ? err.message : err);
        process.exit(1);
      }
    },
  );

governance
  .command("apply-weights")
  .description("Apply queued scoring weights after the credit-oracle timelock expires.")
  .argument("<payer-secret>", "Stellar secret key of the fee payer (starts with S)")
  .option("--dry-run", "Simulate transaction execution without broadcasting")
  .action(
    async (
      payerSecret: string,
      cmdOptions: { dryRun?: boolean },
    ) => {
      const globalOptions = program.opts();
      const network = globalOptions.network as NetworkType;
      const config = loadConfig(network);
      validateConfig(config, ['governanceId']);
      const keypair = parseSecret(payerSecret);

      if (cmdOptions.dryRun) {
        const contract = new Contract(config.governanceId!);
        const operation = contract.call("apply_weights");
        await simulateDryRun({
          rpcUrl: config.rpcUrl,
          networkPassphrase: config.networkPassphrase,
          sourceKeypair: keypair,
          contractId: config.governanceId!,
          operation,
          label: "governance apply-weights",
        });
        return;
      }

      const sdk = new StellarDIDCreditSDK(config);
      console.log(`Applying governance weights on ${network}...`);

      try {
        const txHash = await sdk.governance.applyWeights(keypair);
        console.log();
        console.log("Success!");
        console.log(`  Transaction: ${txHash}`);
      } catch (err) {
        console.error("Failed:", err instanceof Error ? err.message : err);
        process.exit(1);
      }
    },
  );

/**
 * Parses a voter weight into a bigint. Weights must be non-negative integers;
 * when `allowZero` is false, zero is rejected too.
 */
function parseVoterWeight(value: string, allowZero: boolean): bigint {
  if (!/^\d+$/.test(value)) {
    throw new InvalidArgumentError("weight must be a non-negative integer.");
  }
  const weight = BigInt(value);
  if (!allowZero && weight === 0n) {
    throw new InvalidArgumentError(
      "weight must be greater than 0 (use update-voter-weight with 0 to deregister).",
    );
  }
  return weight;
}

function parseRegisterWeight(value: string): bigint {
  return parseVoterWeight(value, false);
}

function parseUpdateWeight(value: string): bigint {
  return parseVoterWeight(value, true);
}

/**
 * Shared implementation for the voter-admin governance commands.
 */
async function runVoterAdminCommand(params: {
  method: "registerVoter" | "updateVoterWeight";
  contractFn: "register_voter" | "update_voter_weight";
  label: string;
  progress: string;
  adminSecret: string;
  voter: string;
  weight: bigint;
  dryRun?: boolean;
}): Promise<void> {
  const { method, contractFn, label, progress, adminSecret, voter, weight } = params;
  const network = program.opts().network as NetworkType;
  const config = loadConfig(network);
  validateConfig(config, ['governanceId']);
  const keypair = parseSecret(adminSecret);
  assertStellarAddress("voter", voter);

  if (params.dryRun) {
    const contract = new Contract(config.governanceId!);
    const operation = contract.call(
      contractFn,
      new Address(keypair.publicKey()).toScVal(),
      new Address(voter).toScVal(),
      nativeToScVal(weight, { type: "i128" }),
    );
    await simulateDryRun({
      rpcUrl: config.rpcUrl,
      networkPassphrase: config.networkPassphrase,
      sourceKeypair: keypair,
      contractId: config.governanceId!,
      operation,
      label,
    });
    return;
  }

  const sdk = new StellarDIDCreditSDK(config);
  console.log(`${progress} on ${network}...`);
  console.log(`  Admin:  ${keypair.publicKey()}`);
  console.log(`  Voter:  ${voter}`);
  console.log(`  Weight: ${weight.toString()}`);

  try {
    const txHash = await sdk.governance[method](keypair, voter, weight);
    const explorerBase = network === 'mainnet' ? 'https://stellar.expert/explorer/public' : 'https://stellar.expert/explorer/testnet';
    console.log();
    console.log("Success!");
    console.log(`  Transaction: ${txHash}`);
    console.log(`  Explorer:    ${explorerBase}/tx/${txHash}`);
  } catch (err) {
    console.error("Failed:", err instanceof Error ? err.message : err);
    process.exit(1);
  }
}

governance
  .command("register-voter")
  .description(
    "Register a new governance voter with a voting weight (admin only). " +
      "Voters must be registered before proposals can reach quorum.",
  )
  .argument("<admin-secret>", "Stellar secret key of the governance admin (starts with S)")
  .argument("<voter-address>", "Stellar address of the voter (G...)")
  .argument("<weight>", "Voting weight to assign (positive integer)", parseRegisterWeight)
  .option("--dry-run", "Simulate transaction execution without broadcasting")
  .action(
    async (
      adminSecret: string,
      voter: string,
      weight: bigint,
      cmdOptions: { dryRun?: boolean },
    ) => {
      await runVoterAdminCommand({
        method: "registerVoter",
        contractFn: "register_voter",
        label: "governance register-voter",
        progress: "Registering governance voter",
        adminSecret,
        voter,
        weight,
        dryRun: cmdOptions.dryRun,
      });
    },
  );

governance
  .command("update-voter-weight")
  .description(
    "Update the voting weight of a registered voter (admin only). " +
      "Set weight to 0 to deregister the voter.",
  )
  .argument("<admin-secret>", "Stellar secret key of the governance admin (starts with S)")
  .argument("<voter-address>", "Stellar address of the voter (G...)")
  .argument("<weight>", "New voting weight (non-negative integer; 0 deregisters)", parseUpdateWeight)
  .option("--dry-run", "Simulate transaction execution without broadcasting")
  .action(
    async (
      adminSecret: string,
      voter: string,
      weight: bigint,
      cmdOptions: { dryRun?: boolean },
    ) => {
      await runVoterAdminCommand({
        method: "updateVoterWeight",
        contractFn: "update_voter_weight",
        label: "governance update-voter-weight",
        progress: "Updating governance voter weight",
        adminSecret,
        voter,
        weight,
        dryRun: cmdOptions.dryRun,
      });
    },
  );

// ---------------------------------------------------------------------------
// Parse
// ---------------------------------------------------------------------------

// If this file was invoked directly (not imported), run the CLI.
if (require.main === module) {
  program.parse();
}

export { program };

