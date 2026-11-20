import { spawnSync } from "node:child_process";
import { copyFile, readFile, rm, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
await rm(new URL("../pkg/", import.meta.url), { recursive: true, force: true });
for (const [target, directory] of [["web", "web"], ["nodejs", "node"]]) {
  const result = spawnSync("wasm-pack", ["build", "--release", "--target", target, "--out-dir", `pkg/${directory}`], {
    cwd: root,
    stdio: "inherit",
  });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}
// wasm-pack's node output must remain CommonJS beneath the ESM package root.
await writeFile(new URL("../pkg/node/package.json", import.meta.url), '{"type":"commonjs"}\n');
for (const name of ["zk_wasm.js", "zk_wasm_cjs.cjs"]) {
  await copyFile(new URL(`../js/${name}`, import.meta.url), new URL(`../pkg/${name}`, import.meta.url));
}
const types = await readFile(new URL("../pkg/web/zk_wasm.d.ts", import.meta.url), "utf8");
await writeFile(new URL("../pkg/zk_wasm.d.ts", import.meta.url), `
export { generate_score_proof } from "./web/zk_wasm.js";
import type { InitInput } from "./web/zk_wasm.js";
/** Initialize the prover with optional bytes, a module, Response, or URL. */
export declare function initZkWasm(input?: InitInput): Promise<typeof import("./web/zk_wasm.js")>;
export default initZkWasm;
`);
// Read the generated declaration to fail early if wasm-bindgen changes its API.
if (!types.includes("InitInput")) throw new Error("Missing generated initialization types");
