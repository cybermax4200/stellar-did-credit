import { StellarDIDCreditSDK, ScoreNotComputedError } from "./index";

const TESTNET_PASSPHRASE = "Test SDF Network ; September 2015";
const MAINNET_PASSPHRASE = "Public Global Stellar Network ; September 2015";
const SIM_ACCOUNT = "GAAZI4TCR3TY5OJHCTJC2A4QSY6CJWJH5IAJTGKIN2ER7LBNVKOCCWN";
const CUSTOM_SIM_ACCOUNT = "GBXVQJSV5YFRF5BBTPQM3KW6ZKZFAPFGWVL7IVJLQ7PGJHV5LJIXNJ4";

const BASE_CONFIG = {
  identityOracleId: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
  creditOracleId: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
  revocationRegistryId: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
  rpcUrl: "http://localhost:8000",
};

describe("StellarDIDCreditSDK", () => {
  describe("getScore", () => {
    it("throws ScoreNotComputedError when score has not been computed", async () => {
      const sdk = new StellarDIDCreditSDK({
        ...BASE_CONFIG,
        networkPassphrase: TESTNET_PASSPHRASE,
      });

      try {
        await sdk.getScore("GBUQWP3BOUZX34ULNQG23RQ6F4YUSXHTQSXE7XDZT4A65XJLQRGEZSM");
      } catch (error) {
        expect(error).toBeInstanceOf(Error);
      }
    });

    it("exports ScoreNotComputedError class", () => {
      expect(ScoreNotComputedError).toBeDefined();
      const error = new ScoreNotComputedError();
      expect(error).toBeInstanceOf(Error);
      expect(error.name).toBe("ScoreNotComputedError");
      expect(error.message).toContain("Score has not been computed");
    });

    it("includes address in ScoreNotComputedError when provided", () => {
      const addr = "GBUQWP3BOUZX34ULNQG23RQ6F4YUSXHTQSXE7XDZT4A65XJLQRGEZSM";
      const error = new ScoreNotComputedError(addr);
      expect(error.message).toContain(addr);
    });
  });

  describe("simulationAccount resolution", () => {
    it("uses SIM_ACCOUNT on testnet when simulationAccount is not configured", () => {
      const sdk = new StellarDIDCreditSDK({
        ...BASE_CONFIG,
        networkPassphrase: TESTNET_PASSPHRASE,
      });
      expect((sdk as any).getSimulationAccount()).toBe(SIM_ACCOUNT);
    });

    it("uses the configured simulationAccount on any network", () => {
      const sdk = new StellarDIDCreditSDK({
        ...BASE_CONFIG,
        networkPassphrase: MAINNET_PASSPHRASE,
        simulationAccount: CUSTOM_SIM_ACCOUNT,
      });
      expect((sdk as any).getSimulationAccount()).toBe(CUSTOM_SIM_ACCOUNT);
    });

    it("uses the configured simulationAccount even on testnet", () => {
      const sdk = new StellarDIDCreditSDK({
        ...BASE_CONFIG,
        networkPassphrase: TESTNET_PASSPHRASE,
        simulationAccount: CUSTOM_SIM_ACCOUNT,
      });
      expect((sdk as any).getSimulationAccount()).toBe(CUSTOM_SIM_ACCOUNT);
    });

    it("throws when simulationAccount is not set on a non-testnet network", () => {
      const sdk = new StellarDIDCreditSDK({
        ...BASE_CONFIG,
        networkPassphrase: MAINNET_PASSPHRASE,
      });
      expect(() => (sdk as any).getSimulationAccount()).toThrow(
        "simulationAccount must be provided in ProtocolConfig for non-testnet networks",
      );
    });

    it("throws for a custom network passphrase without simulationAccount", () => {
      const sdk = new StellarDIDCreditSDK({
        ...BASE_CONFIG,
        networkPassphrase: "My Custom Network ; 2024",
      });
      expect(() => (sdk as any).getSimulationAccount()).toThrow(
        "simulationAccount must be provided",
      );
    });
  });
});
