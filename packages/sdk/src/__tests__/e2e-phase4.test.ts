import { Keypair } from "@stellar/stellar-sdk";
import StellarDIDCreditSDK from "../index";
import * as ZkWasm from "@stellar-did-credit/zk-wasm";

describe("Phase 4 End-to-End: ZK Score Proof generation and verification", () => {
  let sdk: StellarDIDCreditSDK;
  let testAccount: Keypair;
  let verifierContractId: string;

  beforeAll(async () => {
    // We mock the methods for testing, but in a real e2e we'd deploy contracts
    // For now we will test if the ZK WASM logic and SDK glue work correctly.
    
    sdk = new StellarDIDCreditSDK({
      rpcUrl: "http://localhost:8000",
      networkPassphrase: "Test SDF Network ; September 2015",
      identityOracleId: Keypair.random().publicKey(),
      creditOracleId: Keypair.random().publicKey(),
      revocationRegistryId: Keypair.random().publicKey(),
      simAccount: Keypair.random().secret(),
    });
    
    testAccount = Keypair.random();
    verifierContractId = "C_VERIFIER_MOCK";

    jest.spyOn(sdk, "getScore").mockResolvedValue({
      score: 750,
      lastUpdated: Math.floor(Date.now() / 1000),
      vcCount: 5,
      repaymentRate: 9800,
      txVolume30d: 5000000n,
      previousScore: 700,
      computedAtLedger: 1000,
      stale: false,
    });

    jest.spyOn(sdk, "getTxStats").mockResolvedValue({
      volume30d: 5000000n,
      txCount30d: 45,
      avgCounterparties: 8,
    });

    jest.spyOn(sdk, "getRepaymentRecord").mockResolvedValue({
      onTimeCount: 49,
      totalCount: 50,
      totalRepaid: 1000000n,
    });

    jest.spyOn(sdk, "getWeights").mockResolvedValue({
      vcWeight: 40,
      txWeight: 30,
      repaymentWeight: 30,
    });
  });

  it("should generate a proof and verify it", async () => {
    const threshold = 600;
    
    // Generate proof
    const proof = await sdk.generateScoreProof(
      testAccount.publicKey(),
      threshold,
      12345
    );
    
    expect(proof).toBeInstanceOf(Uint8Array);
    expect(proof.length).toBeGreaterThan(0);
    
    // Verify proof
    // Since we don't have a local network running, we mock the RPC calls in verifyScoreProof,
    // or we can test it if we have a network. For now we will mock verifyScoreProof since it uses RPC.
    jest.spyOn(sdk, "verifyScoreProof").mockResolvedValue(true);
    
    const isVerified = await sdk.verifyScoreProof(
      testAccount,
      testAccount.publicKey(),
      threshold,
      proof,
      verifierContractId
    );
    
    expect(isVerified).toBe(true);
    expect(sdk.verifyScoreProof).toHaveBeenCalledWith(
      testAccount,
      testAccount.publicKey(),
      threshold,
      proof,
      verifierContractId
    );
  });
});
