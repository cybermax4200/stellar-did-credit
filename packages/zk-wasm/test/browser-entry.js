import { initZkWasm, generate_score_proof } from "@stellar-did-credit/zk-wasm";

globalThis.zkWasmSmoke = initZkWasm().then((bindings) => {
  if (bindings.generate_score_proof !== generate_score_proof) {
    throw new Error("Bundled prover export does not match initialized bindings");
  }
  return true;
});
