let bindings;

/** CommonJS keeps synchronous proof generation and also supports explicit init. */
exports.initZkWasm = async function initZkWasm(input) {
  if (input === undefined) {
    bindings ??= require("./node/zk_wasm.js");
  } else {
    const esm = await import("./zk_wasm.js");
    bindings = await esm.initZkWasm(input);
  }
  return bindings;
};

exports.generate_score_proof = function generate_score_proof(...args) {
  bindings ??= require("./node/zk_wasm.js");
  return bindings.generate_score_proof(...args);
};
