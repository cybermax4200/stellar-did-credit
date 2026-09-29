const fs = require('fs');

const path = 'packages/cli/src/index.ts';
let content = fs.readFileSync(path, 'utf8');

// Update imports
content = content.replace(
  'import { loadConfig, type NetworkType } from "./config";',
  'import { loadConfig, validateConfig, type NetworkType } from "./config";'
);

const rules = [
  { name: 'anchor-did', reqs: "['identityOracleId']", sim: false },
  { name: 'get-score', reqs: "['creditOracleId']", sim: true },
  { name: 'verify-vc', reqs: "['identityOracleId']", sim: true },
  { name: 'is-verified', reqs: "['identityOracleId']", sim: true },
  { name: 'vc-count', reqs: "['identityOracleId']", sim: true },
  { name: 'vcs', reqs: "['identityOracleId']", sim: true },
  { name: 'credential-type', reqs: "['identityOracleId']", sim: true },
  { name: 'issuers', reqs: "['identityOracleId']", sim: true },
  { name: 'compute-score', reqs: "['creditOracleId']", sim: false },
  { name: 'weights', reqs: "['creditOracleId']", sim: true },
  { name: 'anchor-vc', reqs: "['identityOracleId']", sim: false },
  { name: 'create-proposal', reqs: "['governanceId']", sim: false },
  { name: 'vote', reqs: "['governanceId']", sim: false },
  { name: 'execute', reqs: "['governanceId']", sim: false },
  { name: 'apply-weights', reqs: "['governanceId']", sim: false },
  { name: 'show', reqs: "['governanceId']", sim: true },
  { name: 'list', reqs: "['governanceId']", sim: true },
];

for (const rule of rules) {
  // Regex to find the block for the command and inject validateConfig
  // It looks for `.command("commandName")` and the following `const config = loadConfig(network);`
  const regex = new RegExp(`(\\.command\\("${rule.name}"\\)[\\s\\S]*?const config = loadConfig\\(network\\);)`, 'g');
  const inject = `\n    validateConfig(config, ${rule.reqs}${rule.sim ? ', true' : ''});`;
  
  content = content.replace(regex, `$1${inject}`);
}

fs.writeFileSync(path, content, 'utf8');
console.log('done');
