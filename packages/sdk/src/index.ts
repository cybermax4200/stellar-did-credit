import {
  Contract,
  SorobanRpc,
  TransactionBuilder,
  Networks,
  BASE_FEE,
} from "@stellar/stellar-sdk";

export interface ScoreRecord {
  score: number;
  lastUpdated: number;
  vcCount: number;
  repaymentRate: number;
  txVolume30d: bigint;
}

export interface ProtocolConfig {
  identityOracleId: string;
  creditOracleId: string;
  revocationRegistryId: string;
  networkPassphrase: string;
  rpcUrl: string;
}

export class StellarDIDCreditSDK {
  constructor(private config: ProtocolConfig) {}

  async anchorDID(subjectKeypair: any, didDocCid: string): Promise<string> {
    throw new Error("not implemented — see GitHub issue #7");
  }

  async issueVC(
    issuerKeypair: any,
    subjectAddress: string,
    vcHash: Buffer,
  ): Promise<string> {
    throw new Error("not implemented — see GitHub issue #8");
  }

  async getScore(subjectAddress: string): Promise<ScoreRecord> {
    throw new Error("not implemented — will be implemented next");
  }

  async verifyVC(subjectAddress: string, vcHash: Buffer): Promise<boolean> {
    throw new Error("not implemented — see GitHub issue #9");
  }

  async isVerified(subjectAddress: string): Promise<boolean> {
    throw new Error("not implemented — see GitHub issue #9");
  }
}

export default StellarDIDCreditSDK;
