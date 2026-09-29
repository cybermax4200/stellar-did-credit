import { validateConfig } from "./config";
import type { ProtocolConfig } from "@stellar-did-credit/sdk";

describe("validateConfig", () => {
  let originalConsoleError: typeof console.error;
  let originalProcessExit: typeof process.exit;
  let mockConsoleError: jest.Mock;
  let mockProcessExit: jest.Mock;

  beforeEach(() => {
    originalConsoleError = console.error;
    originalProcessExit = process.exit;

    mockConsoleError = jest.fn();
    mockProcessExit = jest.fn((code?: string | number | null | undefined) => {
      throw new Error(`process.exit called with ${code}`);
    }) as unknown as jest.Mock;

    console.error = mockConsoleError;
    process.exit = mockProcessExit as unknown as typeof process.exit;
  });

  afterEach(() => {
    console.error = originalConsoleError;
    process.exit = originalProcessExit;
  });

  it("should pass when all required fields are present", () => {
    const config: Partial<ProtocolConfig> = {
      identityOracleId: "C1",
      creditOracleId: "C2",
    };

    expect(() => validateConfig(config, ["identityOracleId", "creditOracleId"])).not.toThrow();
    expect(mockConsoleError).not.toHaveBeenCalled();
    expect(mockProcessExit).not.toHaveBeenCalled();
  });

  it("should fail with correct error message when IDENTITY_ORACLE_ID is missing", () => {
    const config: Partial<ProtocolConfig> = {
      creditOracleId: "C2",
    };

    expect(() => validateConfig(config, ["identityOracleId", "creditOracleId"])).toThrow("process.exit called with 1");
    expect(mockConsoleError).toHaveBeenCalledWith(
      "Error: Missing required config: IDENTITY_ORACLE_ID. Set via environment variable or stellar-did-config.json."
    );
  });

  it("should fail with correct error message when CREDIT_ORACLE_ID is missing", () => {
    const config: Partial<ProtocolConfig> = {
      identityOracleId: "C1",
    };

    expect(() => validateConfig(config, ["identityOracleId", "creditOracleId"])).toThrow("process.exit called with 1");
    expect(mockConsoleError).toHaveBeenCalledWith(
      "Error: Missing required config: CREDIT_ORACLE_ID. Set via environment variable or stellar-did-config.json."
    );
  });

  it("should fail with correct error message when GOVERNANCE_ID is missing", () => {
    const config: Partial<ProtocolConfig> = {};

    expect(() => validateConfig(config, ["governanceId"])).toThrow("process.exit called with 1");
    expect(mockConsoleError).toHaveBeenCalledWith(
      "Error: Missing required config: GOVERNANCE_ID. Set via environment variable or stellar-did-config.json."
    );
  });

  it("should fail with correct error message when multiple fields are missing", () => {
    const config: Partial<ProtocolConfig> = {};

    expect(() => validateConfig(config, ["identityOracleId", "revocationRegistryId"])).toThrow("process.exit called with 1");
    expect(mockConsoleError).toHaveBeenCalledWith(
      "Error: Missing required config: IDENTITY_ORACLE_ID, REVOCATION_REGISTRY_ID. Set via environment variable or stellar-did-config.json."
    );
  });

  it("should fail when SIM_ACCOUNT is missing and requiresSimAccount is true", () => {
    const config: Partial<ProtocolConfig> = {
      identityOracleId: "C1",
      // simAccount missing
    };

    expect(() => validateConfig(config, ["identityOracleId"], true)).toThrow("process.exit called with 1");
    expect(mockConsoleError).toHaveBeenCalledWith(
      "Error: Missing required config: SIM_ACCOUNT. Set via environment variable or stellar-did-config.json."
    );
  });

  it("should pass when SIM_ACCOUNT is provided and requiresSimAccount is true", () => {
    const config: Partial<ProtocolConfig> = {
      identityOracleId: "C1",
      simAccount: "G1",
    };

    expect(() => validateConfig(config, ["identityOracleId"], true)).not.toThrow();
    expect(mockConsoleError).not.toHaveBeenCalled();
    expect(mockProcessExit).not.toHaveBeenCalled();
  });
});
