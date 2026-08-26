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
// Command: governance
// ---------------------------------------------------------------------------

const governance = program.command("governance").description("Governance commands for protocol weight updates");

/**
 * Create a governance proposal to update scoring weights.
 */
governance
  .command("create-proposal")
  .description("Create a governance proposal to update scoring weights")
  .argument("<proposer-secret>", "Stellar secret key of the registered proposer (starts with S)")
  .argument("<vc-weight>", "Weight percentage for verifiable credentials (0-100)")
  .argument("<tx-weight>", "Weight percentage for transaction stats (0-100)")
  .argument("<repay-weight>", "Weight percentage for repayment history (0-100)")
  .option("--voting-period <ledgers>", "Voting period in ledgers (default: 17280 ~24h)")
  .option("--delay <ledgers>", "Execution delay in ledgers (default: 17280 ~24h)")
  .addHelpText(
    "after",
    `
Example:
  $ stellar-did governance create-proposal S... 40 30 30 --voting-period 17280 --delay 17280
`
  )
  .action(async (proposerSecret: string, vcWeightStr: string, txWeightStr: string, repayWeightStr: string, cmdOptions: { votingPeriod?: string, delay?: string }) => {
    const options = program.opts();
    const network = options.network as NetworkType;
    const config = loadConfig(network);
    const keypair = parseSecret(proposerSecret);

    const vc = parseInt(vcWeightStr, 10);
    const tx = parseInt(txWeightStr, 10);
    const repay = parseInt(repayWeightStr, 10);

    if (isNaN(vc) || isNaN(tx) || isNaN(repay) || vc < 0 || tx < 0 || repay < 0 || (vc + tx + repay) !== 100) {
      console.error("Error: Weights must be positive integers that sum to 100.");
      process.exit(1);
    }

    const votingPeriod = cmdOptions.votingPeriod ? parseInt(cmdOptions.votingPeriod, 10) : 17280;
    const executionDelay = cmdOptions.delay ? parseInt(cmdOptions.delay, 10) : 17280;

    const sdk = new StellarDIDCreditSDK(config);

    console.log(`Creating proposal on ${network}...`);
    try {
      const proposalId = await sdk.governance.createProposal(
        keypair,
        { vcWeight: vc, txWeight: tx, repaymentWeight: repay },
        votingPeriod,
        executionDelay
      );
      console.log(`Success! Proposal created with ID: ${proposalId.toString()}`);
    } catch (err) {
      console.error("Failed:", err instanceof Error ? err.message : String(err));
      process.exit(1);
    }
  });

/**
 * Cast a vote on a governance proposal.
 */
governance
  .command("vote")
  .description("Cast a weighted vote on an open proposal")
  .argument("<voter-secret>", "Stellar secret key of the registered voter")
  .argument("<proposal-id>", "The numeric ID of the proposal")
  .argument("<vote>", "Vote choice: 'for' or 'against'")
  .argument("<weight>", "Your registered voting weight")
  .addHelpText(
    "after",
    `
Example:
  $ stellar-did governance vote S... 1 for 100
`
  )
  .action(async (voterSecret: string, proposalIdStr: string, voteChoice: string, weightStr: string) => {
    const options = program.opts();
    const network = options.network as NetworkType;
    const config = loadConfig(network);
    const keypair = parseSecret(voterSecret);

    const proposalId = BigInt(proposalIdStr);
    const voteFor = voteChoice.toLowerCase() === 'for';
    if (!voteFor && voteChoice.toLowerCase() !== 'against') {
      console.error("Error: vote must be 'for' or 'against'");
      process.exit(1);
    }
    const weight = BigInt(weightStr);

    const sdk = new StellarDIDCreditSDK(config);

    console.log(`Casting vote on proposal ${proposalId} on ${network}...`);
    try {
      const txHash = await sdk.governance.vote(keypair, proposalId, voteFor, weight);
      console.log(`Success! Vote transaction: ${txHash}`);
    } catch (err) {
      console.error("Failed:", err instanceof Error ? err.message : String(err));
      process.exit(1);
    }
  });

/**
 * Execute a passed governance proposal.
 */
governance
  .command("execute")
  .description("Execute a passing proposal (queues new weights in the credit-oracle)")
  .argument("<payer-secret>", "Stellar secret key to pay the transaction fee")
  .argument("<proposal-id>", "The numeric ID of the proposal to execute")
  .addHelpText(
    "after",
    `
Note: The double-timelock model means execution merely queues the new weights.
You must wait approximately 24 hours (or the configured delay) before running 'apply-weights'.

Example:
  $ stellar-did governance execute S... 1
`
  )
  .action(async (payerSecret: string, proposalIdStr: string) => {
    const options = program.opts();
    const network = options.network as NetworkType;
    const config = loadConfig(network);
    const keypair = parseSecret(payerSecret);
    const proposalId = BigInt(proposalIdStr);
    const sdk = new StellarDIDCreditSDK(config);

    console.log(`Executing proposal ${proposalId} on ${network}...`);
    try {
      const txHash = await sdk.governance.execute(keypair, proposalId);
      console.log(`Success! Execute transaction: ${txHash}`);
      console.log("Note: Weights are now queued. Use 'apply-weights' after the timelock expires.");
    } catch (err) {
      console.error("Failed:", err instanceof Error ? err.message : String(err));
      process.exit(1);
    }
  });

/**
 * Apply queued weights after the credit-oracle timelock expires.
 */
governance
  .command("apply-weights")
  .description("Apply weights queued by a previously executed proposal")
  .argument("<payer-secret>", "Stellar secret key to pay the transaction fee")
  .addHelpText(
    "after",
    `
Note: This must be called only after the credit-oracle's fixed timelock has
expired, approximately 24 hours after 'execute' was successful.

Example:
  $ stellar-did governance apply-weights S...
`
  )
  .action(async (payerSecret: string) => {
    const options = program.opts();
    const network = options.network as NetworkType;
    const config = loadConfig(network);
    const keypair = parseSecret(payerSecret);
    const sdk = new StellarDIDCreditSDK(config);

    console.log(`Applying pending weights on ${network}...`);
    try {
      const txHash = await sdk.governance.applyWeights(keypair);
      console.log(`Success! Apply weights transaction: ${txHash}`);
    } catch (err) {
      console.error("Failed:", err instanceof Error ? err.message : String(err));
      process.exit(1);
    }
  });

/**
 * Show details of a governance proposal.
 */
governance
  .command("show")
  .description("Show a human-readable view of a proposal's state")
  .argument("<proposal-id>", "The numeric ID of the proposal to show")
  .action(async (proposalIdStr: string) => {
    const options = program.opts();
    const network = options.network as NetworkType;
    const config = loadConfig(network);
    const proposalId = BigInt(proposalIdStr);
    const sdk = new StellarDIDCreditSDK(config);

    try {
      const proposal = await sdk.governance.getProposal(proposalId);
      if (!proposal) {
        console.error(`Proposal ${proposalId} not found.`);
        process.exit(1);
      }

      console.log(`Proposal ${proposalId}`);
      console.log(`  Proposer:      ${proposal.proposer}`);
      console.log(`  Weights:       VC=${proposal.proposedWeights.vcWeight}% TX=${proposal.proposedWeights.txWeight}% REPAY=${proposal.proposedWeights.repaymentWeight}%`);
      console.log(`  Votes:         FOR: ${proposal.votesFor.toString()} | AGAINST: ${proposal.votesAgainst.toString()}`);
      console.log(`  Quorum:        ${proposal.quorumRequired.toString()}`);
      console.log(`  Expiry Ledger: ${proposal.expiryLedger.toString()}`);
      console.log(`  Delay:         ${proposal.executionDelayLedgers.toString()} ledgers`);
      
      const passing = proposal.votesFor > proposal.votesAgainst && (proposal.votesFor + proposal.votesAgainst) >= proposal.quorumRequired;
      console.log(`  Passing:       ${passing}`);
      console.log(`  Executed:      ${proposal.executed}`);
      console.log(`  Cancelled:     ${proposal.cancelled}`);
    } catch (err) {
      console.error("Failed:", err instanceof Error ? err.message : String(err));
      process.exit(1);
    }
  });

/**
 * List governance proposals.
 */
governance
  .command("list")
  .description("List governance proposals")
  .option("--from <id>", "Starting proposal ID (default: 0)")
  .option("--limit <n>", "Number of proposals to fetch (default: 10)")
  .action(async (cmdOptions: { from?: string, limit?: string }) => {
    const options = program.opts();
    const network = options.network as NetworkType;
    const config = loadConfig(network);
    const fromId = cmdOptions.from ? BigInt(cmdOptions.from) : 0n;
    const limit = cmdOptions.limit ? parseInt(cmdOptions.limit, 10) : 10;
    const sdk = new StellarDIDCreditSDK(config);

    try {
      const proposals = await sdk.governance.listProposals(fromId, limit);
      if (proposals.length === 0) {
        console.log("No proposals found.");
        return;
      }

      for (let i = 0; i < proposals.length; i++) {
        const p = proposals[i];
        const currentId = fromId + BigInt(i);
        console.log(`[${currentId}] Weights: ${p.proposedWeights.vcWeight}/${p.proposedWeights.txWeight}/${p.proposedWeights.repaymentWeight} | FOR: ${p.votesFor} AGAINST: ${p.votesAgainst} | Executed: ${p.executed}`);
      }
    } catch (err) {
      console.error("Failed:", err instanceof Error ? err.message : String(err));
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
