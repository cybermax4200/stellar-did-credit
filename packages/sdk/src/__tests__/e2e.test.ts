/**
 * E2E test against deployed testnet contracts.
 *
 * Gated behind RUN_E2E=true to avoid running in standard CI.
 * Requires:
 *   - Real contract addresses in deployments.testnet.json
 *   - E2E_ISSUER_SECRET: secret key of a pre-registered trusted issuer
 *   - Live Stellar testnet RPC + Friendbot
 *
 * Run with:
 *   RUN_E2E=true E2E_ISSUER_SECRET=S... pnpm test:e2e
 */

import * as path from "path";
import * as crypto from "crypto";
import { Keypair, Networks } from "@stellar/stellar-sdk";
import { StellarDIDCreditSDK, MIN_SCORE } from "../index";

const RPC_URL = "https://soroban-testnet.stellar.org";
const NETWORK_PASSPHRASE = Networks.TESTNET;
const FRIENDBOT_URL = "https://friendbot.stellar.org";
const DEPLOYMENTS_PATH = path.resolve(__dirname, "../../../../../deployments.testnet.json");
const TIMEOUT_MS = 60_000;

// Skip entire suite unless RUN_E2E=true
const runE2E = process.env.RUN_E2E === "true";
const describeE2E = runE2E ? describe : describe.skip;

describeE2E("E2E: deployed testnet contracts", () => {
  let sdk: StellarDIDCreditSDK;
  let subjectKeypair: Keypair;
  let issuerKeypair: Keypair;

  beforeAll(async () => {
    // Load deployed contract addresses
    // eslint-disable-next-line @typescript-eslint/no-var-requires
    const deployments = require(DEPLOYMENTS_PATH);
    const { contracts } = deployments;

    // Issuer must already be registered with the identity-oracle admin
    const issuerSecret = process.env.E2E_ISSUER_SECRET;
    if (!issuerSecret) {
      throw new Error("E2E_ISSUER_SECRET env var is required");
    }
    issuerKeypair = Keypair.fromSecret(issuerSecret);

    // Generate a fresh subject keypair and fund it via Friendbot
    subjectKeypair = Keypair.random();
    const fundRes = await fetch(`${FRIENDBOT_URL}/?addr=${subjectKeypair.publicKey()}`);
    if (!fundRes.ok) {
      throw new Error(`Friendbot funding failed: ${fundRes.statusText}`);
    }

    sdk = new StellarDIDCreditSDK({
      identityOracleId: contracts["identity-oracle"],
      creditOracleId: contracts["credit-oracle"],
      revocationRegistryId: contracts["revocation-registry"],
      networkPassphrase: NETWORK_PASSPHRASE,
      rpcUrl: RPC_URL,
      simAccount: subjectKeypair.publicKey(),
    });
  }, TIMEOUT_MS);

  it("anchorDID stores the subject DID document CID on-chain", async () => {
    const cid = "QmE2Etest" + subjectKeypair.publicKey().slice(0, 8);
    const txHash = await sdk.anchorDID(subjectKeypair, cid);
    expect(typeof txHash).toBe("string");
    expect(txHash.length).toBeGreaterThan(0);
  }, TIMEOUT_MS);

  it("issueVC anchors a VC hash from the registered issuer", async () => {
    const vcHash = Buffer.from(
      crypto.createHash("sha256").update("e2e-test-vc-" + subjectKeypair.publicKey()).digest()
    );
    const txHash = await sdk.issueVC(issuerKeypair, subjectKeypair.publicKey(), vcHash);
    expect(typeof txHash).toBe("string");
    expect(txHash.length).toBeGreaterThan(0);
  }, TIMEOUT_MS);

  it("computeScore persists a score and getScore returns score > MIN_SCORE", async () => {
    const record = await sdk.computeScore(subjectKeypair, subjectKeypair.publicKey());
    expect(record.score).toBeGreaterThan(MIN_SCORE);
  }, TIMEOUT_MS);
});
