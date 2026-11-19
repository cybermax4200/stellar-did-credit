const assert = require('node:assert/strict');
const fs = require('node:fs');
const crypto = require('node:crypto');
const path = require('node:path');

const baselineDir = path.resolve(process.argv[2] || 'pkg-baseline');
const optimizedDir = path.resolve(process.argv[3] || 'pkg');
const baselineWasm = fs.readFileSync(path.join(baselineDir, 'zk_wasm_bg.wasm'));
const optimizedWasm = fs.readFileSync(path.join(optimizedDir, 'zk_wasm_bg.wasm'));
const baseline = require(path.join(baselineDir, 'zk_wasm.js'));
const optimized = require(path.join(optimizedDir, 'zk_wasm.js'));

const bytes = new Uint8Array(32);
const args = [50, 50_000_000_000, 10, 50, 100, 20_000_000_000, 40, 30, 30,
  10, 1_700_000_000n, 1234567, false, 42, 600, bytes, bytes, 12345, bytes];
const originalProof = baseline.generate_score_proof(...args);
const optimizedProof = optimized.generate_score_proof(...args);
assert.equal(originalProof.length, 384);
assert.deepEqual(optimizedProof, originalProof, 'optimization changed the generated proof');
assert.equal(crypto.createHash('sha256').update(optimizedProof).digest('hex'),
  'fda979c3143191b9a141f8e69be614085656bb3b539388875e26013edf599f88',
  'proof differs from the cryptographically verified circuit fixture');
assert.ok(optimizedWasm.length <= baselineWasm.length * 0.7,
  `optimized WASM ${optimizedWasm.length} is not at least 30% smaller than ${baselineWasm.length}`);
const wasmModule = new WebAssembly.Module(optimizedWasm);
assert.equal(WebAssembly.Module.customSections(wasmModule, 'name').length, 0);
assert.equal(WebAssembly.Module.customSections(wasmModule, 'producers').length, 0);
for (const section of ['.debug_info', '.debug_line', '.debug_abbrev', '.debug_str', '.debug_ranges']) {
  assert.equal(WebAssembly.Module.customSections(wasmModule, section).length, 0, `${section} was not stripped`);
}
console.log(`WASM: ${baselineWasm.length} -> ${optimizedWasm.length} bytes; identical 384-byte proof`);
