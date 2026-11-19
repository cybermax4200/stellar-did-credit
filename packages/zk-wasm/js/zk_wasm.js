import init, * as bindings from "./web/zk_wasm.js";

export { generate_score_proof } from "./web/zk_wasm.js";

let initialization;

/** Initialize once from WASM bytes, a module, a Response, or a fetchable URL. */
export async function initZkWasm(input) {
  if (!initialization) {
    initialization = (async () => {
      let source = input ?? new URL("./web/zk_wasm_bg.wasm", import.meta.url);
      if (typeof source === "string") source = new URL(source, import.meta.url);
      if (source instanceof URL && source.protocol === "file:" &&
          typeof process !== "undefined" && process.versions?.node) {
        const nodeFs = "node:fs/promises";
        const { readFile } = await import(/* webpackIgnore: true */ /* @vite-ignore */ nodeFs);
        source = await readFile(source);
      }
      await init({ module_or_path: source });
      return bindings;
    })().catch((error) => {
      initialization = undefined;
      throw error;
    });
  }
  return initialization;
}

export default initZkWasm;
