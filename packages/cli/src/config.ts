import { existsSync, readFileSync } from "fs";
import { resolve } from "path";
import type { ProtocolConfig } from "@stellar-did-credit/sdk";

export type NetworkType = 'testnet' | 'mainnet' | 'futurenet';

/**
 * Network configurations for Stellar networks.
 */
const NETWORK_CONFIGS: Record<NetworkType, Partial<ProtocolConfig>> = {
  testnet: {
    networkPassphrase: "Test SDF Network ; September 2015",
    rpcUrl: "https://soroban-testnet.stellar.org",
    simAccount: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
  },
  mainnet: {
    networkPassphrase: "Public Global Stellar Network ; September 2015",
    rpcUrl: "https://soroban-rpc.mainnet.stellarchain.io",
    // Note: SIM_ACCOUNT for mainnet must be set via env var to a funded account
    simAccount: "",
  },
  futurenet: {
    networkPassphrase: "Test SDF Future Network ; October 2022",
    rpcUrl: "https://rpc-futurenet.stellar.org",
    simAccount: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
  },
};

/**
 * Default configuration values that apply across all networks.
 */
const BASE_DEFAULTS: Partial<ProtocolConfig> = {
  timeoutSeconds: 30,
  maxRetries: 3,
};

/**
 * Configuration file name searched in the current working directory and
 * the user's home directory (in that order).
 */
const CONFIG_FILE_NAMES = [
  "stellar-did-config.json",
  ".stellar-did-rc.json",
];

/**
 * Load the CLI configuration by merging values from (in order of precedence):
 *   1. Environment variables (highest priority)
 *   2. A JSON config file (searched in cwd then $HOME)
 *   3. Network-specific defaults (testnet/mainnet/futurenet)
 *   4. Base defaults
 *
 * Required fields throw if not set anywhere.
 */
export function loadConfig(network?: NetworkType): ProtocolConfig {
  // 1. Start with base defaults and network-specific defaults
  const selectedNetwork = network || getNetworkFromEnv() || 'testnet';
  const config: Record<string, unknown> = { 
    ...BASE_DEFAULTS,
    ...NETWORK_CONFIGS[selectedNetwork],
  };

  // 2. Try loading from a config file
  const configPath = findConfigFile();
  if (configPath) {
    try {
      const raw = JSON.parse(readFileSync(configPath, "utf-8"));
      // Support both flat keys and nested "contracts" structure (from deployments.testnet.json)
      if (raw.contracts) {
        mergeFromContractsBlock(config, raw.contracts);
      }
      // Flat top-level keys take precedence over contracts block
      mergeConfigOverrides(config, raw);
    } catch (err) {
      console.error(
        `Warning: failed to parse config file ${configPath}:`,
        err instanceof Error ? err.message : err,
      );
    }
  }

  // 3. Environment variables (highest priority)
  mergeEnvOverrides(config);

  // 4. Validate required fields
  assertRequired(config, selectedNetwork);

  return config as unknown as ProtocolConfig;
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

function getNetworkFromEnv(): NetworkType | null {
  const networkEnv = process.env['NETWORK']?.toLowerCase();
  if (networkEnv === 'testnet' || networkEnv === 'mainnet' || networkEnv === 'futurenet') {
    return networkEnv;
  }
  return null;
}

function findConfigFile(): string | null {
  const cwd = process.cwd();
  const home = process.env["HOME"] ?? process.env["USERPROFILE"] ?? "/";

  for (const name of CONFIG_FILE_NAMES) {
    for (const dir of [cwd, home]) {
      const candidate = resolve(dir, name);
      if (existsSync(candidate)) {
        return candidate;
      }
    }
  }
  return null;
}

function mergeFromContractsBlock(
  config: Record<string, unknown>,
  contracts: Record<string, string>,
): void {
  const mapping: Record<string, string> = {
    "identity-oracle": "identityOracleId",
    "credit-oracle": "creditOracleId",
    "revocation-registry": "revocationRegistryId",
  };
  for (const [contractName, configKey] of Object.entries(mapping)) {
    if (contracts[contractName] && !config[configKey]) {
      config[configKey] = contracts[contractName];
    }
  }
}

function mergeConfigOverrides(
  config: Record<string, unknown>,
  raw: Record<string, unknown>,
): void {
  const knownKeys = new Set([
    "identityOracleId",
    "creditOracleId",
    "revocationRegistryId",
    "networkPassphrase",
    "rpcUrl",
    "simAccount",
    "timeoutSeconds",
    "maxRetries",
    "baseFee",
  ]);

  for (const key of knownKeys) {
    if (raw[key] !== undefined && raw[key] !== null) {
      config[key] = raw[key];
    }
  }
}

function mergeEnvOverrides(config: Record<string, unknown>): void {
  const mapping: Record<string, string> = {
    IDENTITY_ORACLE_ID: "identityOracleId",
    CREDIT_ORACLE_ID: "creditOracleId",
    REVOCATION_REGISTRY_ID: "revocationRegistryId",
    NETWORK_PASSPHRASE: "networkPassphrase",
    RPC_URL: "rpcUrl",
    SIM_ACCOUNT: "simAccount",
  };

  for (const [envVar, configKey] of Object.entries(mapping)) {
    const value = process.env[envVar];
    if (value) {
      config[configKey] = value;
    }
  }

  // Numeric env vars with NaN validation
  const timeout = process.env["TIMEOUT_SECONDS"];
  if (timeout !== undefined) {
    const parsed = parseInt(timeout, 10);
    if (!Number.isNaN(parsed) && parsed > 0) {
      config["timeoutSeconds"] = parsed;
    } else {
      console.error(`Warning: TIMEOUT_SECONDS="${timeout}" is not a valid positive integer; using default`);
    }
  }

  const retries = process.env["MAX_RETRIES"];
  if (retries !== undefined) {
    const parsed = parseInt(retries, 10);
    if (!Number.isNaN(parsed) && parsed >= 0) {
      config["maxRetries"] = parsed;
    } else {
      console.error(`Warning: MAX_RETRIES="${retries}" is not a valid non-negative integer; using default`);
    }
  }

  if (process.env["BASE_FEE"]) {
    config["baseFee"] = process.env["BASE_FEE"];
  }
}

function assertRequired(config: Record<string, unknown>, network: NetworkType): void {
  const required: string[] = [
    "identityOracleId",
    "creditOracleId", 
    "revocationRegistryId",
  ];
  const missing = required.filter((k) => !config[k]);
  if (missing.length > 0) {
    if (network === 'mainnet') {
      console.error(
        `Error: No ${network} contract IDs configured. Set ${missing.map(k => {
          switch(k) {
            case 'identityOracleId': return 'IDENTITY_ORACLE_ID=C...';
            case 'creditOracleId': return 'CREDIT_ORACLE_ID=C...';
            case 'revocationRegistryId': return 'REVOCATION_REGISTRY_ID=C...';
            default: return `${k.toUpperCase()}=C...`;
          }
        }).join(', ')} for mainnet.`
      );
    } else {
      console.error(
        `Error: Missing required configuration for: ${missing.join(", ")}`,
      );
      console.error(
        "Set them via environment variables (IDENTITY_ORACLE_ID, CREDIT_ORACLE_ID, " +
          "REVOCATION_REGISTRY_ID) or a config file (stellar-did-config.json).",
      );
    }
    process.exit(1);
  }

  // Validate mainnet SIM_ACCOUNT requirement
  if (network === 'mainnet' && !config.simAccount) {
    console.error(
      "Error: SIM_ACCOUNT is required for mainnet operations. Set SIM_ACCOUNT=G... to a funded mainnet account."
    );
    process.exit(1);
  }
}
