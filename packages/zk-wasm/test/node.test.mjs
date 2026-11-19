import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { test } from "node:test";

const cwd = new URL("../", import.meta.url);
const run = (code, esm = true) => {
  const result = spawnSync(process.execPath, [...(esm ? ["--input-type=module"] : []), "-e", code], {
    cwd, encoding: "utf8", timeout: 60_000,
  });
  assert.equal(result.status, 0, result.stderr || result.error?.message);
};

for (const mode of ["default", "buffer", "arraybuffer", "url", "response", "module"]) {
  test(`Node ESM initializes from ${mode}`, () => run(`
    import assert from "node:assert/strict";
    import { readFile } from "node:fs/promises";
    import { initZkWasm, generate_score_proof } from "@stellar-did-credit/zk-wasm";
    const url = new URL("./pkg/web/zk_wasm_bg.wasm", import.meta.url);
    const bytes = await readFile(url);
    const inputs = {
      default: undefined,
      buffer: bytes,
      arraybuffer: bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength),
      url,
      response: new Response(bytes, { headers: { "Content-Type": "application/wasm" } }),
      module: await WebAssembly.compile(bytes),
    };
    const wasm = await initZkWasm(inputs[${JSON.stringify(mode)}]);
    assert.equal(wasm.generate_score_proof, generate_score_proof);
    assert.equal(await initZkWasm(), wasm);
  `));
}

test("Node ESM initializes from an HTTP fetch URL", () => run(`
  import assert from "node:assert/strict";
  import { readFile } from "node:fs/promises";
  import { createServer } from "node:http";
  import { initZkWasm } from "@stellar-did-credit/zk-wasm";
  const bytes = await readFile("pkg/web/zk_wasm_bg.wasm");
  const server = createServer((req, res) => {
    res.writeHead(200, { "Content-Type": "application/wasm" }); res.end(bytes);
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  try {
    const wasm = await initZkWasm(new URL("http://127.0.0.1:" + server.address().port + "/proof.wasm"));
    assert.equal(typeof wasm.generate_score_proof, "function");
  } finally { server.closeAllConnections(); server.close(); }
`));

test("failed initialization can be retried", () => run(`
  import assert from "node:assert/strict";
  import { initZkWasm } from "@stellar-did-credit/zk-wasm";
  await assert.rejects(initZkWasm(new Uint8Array([0, 1])));
  assert.equal(typeof (await initZkWasm()).generate_score_proof, "function");
`));

test("CommonJS loads without WASM module flags", () => run(`
  const assert = require("node:assert/strict");
  const wasm = require("@stellar-did-credit/zk-wasm");
  assert.equal(typeof wasm.generate_score_proof, "function");
  wasm.initZkWasm().then(bindings => assert.equal(typeof bindings.generate_score_proof, "function"));
`, false));

test("CommonJS initializes from a Node buffer", () => run(`
  const assert = require("node:assert/strict");
  const { readFileSync } = require("node:fs");
  const wasm = require("@stellar-did-credit/zk-wasm");
  wasm.initZkWasm(readFileSync("pkg/web/zk_wasm_bg.wasm"))
    .then(bindings => assert.equal(typeof bindings.generate_score_proof, "function"));
`, false));
