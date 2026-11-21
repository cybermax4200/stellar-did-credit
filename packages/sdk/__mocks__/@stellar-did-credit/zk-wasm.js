// Manual mock for @stellar-did-credit/zk-wasm (WASM package not built in test env)
module.exports = {
  generate_score_proof: jest.fn().mockReturnValue(Buffer.alloc(64, 0)),
  compute_score_commitment: jest.fn().mockReturnValue(Buffer.alloc(32, 0)),
};
