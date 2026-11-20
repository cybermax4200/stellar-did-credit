# zk-worker-demo

Generates Groth16 score proofs in a Web Worker and shows that the page stays
responsive (a spinner and a tick counter run on the main thread while the proof
is computed).

```bash
# from packages/zk-wasm
pnpm build                                    # wasm (node + web) + TypeScript
npx tsc -p examples/zk-worker-demo            # compile main.ts -> main.js
npx http-server . -p 8080                     # any static server rooted at packages/zk-wasm
# open http://localhost:8080/examples/zk-worker-demo/
```

The demo imports `../../dist/index.js`, which spawns `dist/worker.js` as a module
worker; that in turn loads the web build in `pkg-web/`. Serve the whole
`packages/zk-wasm` directory so those relative paths resolve and `.wasm` is sent
as `application/wasm`.
