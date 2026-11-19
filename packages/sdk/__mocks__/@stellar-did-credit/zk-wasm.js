// Manual mock for @stellar-did-credit/zk-wasm (WASM package not built in test env)
module.exports = {
  initZkWasm: jest.fn().mockResolvedValue(undefined),
  generate_score_proof: jest.fn().mockReturnValue(Buffer.alloc(64, 0)),
};
