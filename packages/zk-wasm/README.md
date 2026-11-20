# WASM score prover

Build with `pnpm --filter @stellar-did-credit/zk-wasm build`. The package exports
ESM for Node and browser bundlers, and CommonJS for `require()`. Node does not
need experimental WASM module flags; Vite and Webpack 5 need no WASM loaders.

```js
import { initZkWasm, generate_score_proof } from "@stellar-did-credit/zk-wasm";
await initZkWasm();
// generate_score_proof(...) is now ready.
```

The default helper loads the adjacent WASM file using a Node buffer or a browser
URL. You can also pass a `Uint8Array`/Node `Buffer`, `ArrayBuffer`, compiled
`WebAssembly.Module`, `Response`, or fetchable URL:

```js
const wasm = await initZkWasm(new URL("https://example.org/zk_wasm_bg.wasm"));
// wasm.generate_score_proof(...)
```

Initialization is shared by concurrent calls and can be retried after a failed
load. A successfully initialized ESM instance is reused on subsequent calls.
CommonJS retains synchronous `generate_score_proof(...)` compatibility through
the Node bindings; `await require("@stellar-did-credit/zk-wasm").initZkWasm()` is
also supported. Its explicit-input helper uses the ESM initialization path.

The CommonJS entry uses `.cjs` because the package is ESM (`type: module`).
Generated bindings live in `pkg/web` and `pkg/node`, so rebuilding does not
replace the source helpers in `js/`.

Run `pnpm --filter @stellar-did-credit/zk-wasm test` for Node import smoke tests
and `pnpm --filter @stellar-did-credit/zk-wasm test:bundlers` to build and execute
browser bundles produced by Vite and Webpack, including WASM initialization.
