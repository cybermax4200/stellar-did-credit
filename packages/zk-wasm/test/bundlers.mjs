import assert from "node:assert/strict";
import { readFile, rm } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import vm from "node:vm";
import { build } from "vite";
import webpack from "webpack";

const root = fileURLToPath(new URL("../", import.meta.url));
const entry = fileURLToPath(new URL("browser-entry.js", import.meta.url));
const output = new URL("../.smoke/", import.meta.url);
await rm(output, { recursive: true, force: true });

try {
  await build({
    root,
    configFile: false,
    build: {
      outDir: ".smoke/vite",
      lib: { entry, name: "ZkWasmSmoke", formats: ["iife"], fileName: () => "bundle.js" },
      minify: false,
    },
  });
  await new Promise((resolve, reject) => {
    const compiler = webpack({
      mode: "production",
      target: "web",
      entry,
      output: {
        path: fileURLToPath(new URL("webpack/", output)),
        filename: "bundle.js",
        publicPath: "",
      },
      optimization: { minimize: false },
    });
    compiler.run((error, stats) => compiler.close((closeError) => {
      if (error || closeError) return reject(error || closeError);
      if (stats.hasErrors() || stats.hasWarnings()) return reject(new Error(stats.toString()));
      resolve();
    }));
  });
  for (const bundler of ["vite", "webpack"]) {
    const directory = new URL(`${bundler}/`, output);
    // Execute the real browser bundle with web globals and a fetchable asset.
    // There is no process, require, or filesystem API inside this context.
    const context = vm.createContext({
      URL, WebAssembly, Response, Request, TextEncoder, TextDecoder,
      Uint8Array, ArrayBuffer, console,
      location: { href: "https://smoke.example/bundle.js" },
      document: { currentScript: { src: "https://smoke.example/bundle.js", tagName: "SCRIPT" }, baseURI: "https://smoke.example/" },
      fetch: async (url) => {
        if (new URL(url, "https://smoke.example/").protocol === "data:") return fetch(url);
        const path = new URL(url, "https://smoke.example/").pathname.slice(1);
        const bytes = await readFile(new URL(path, directory));
        return new Response(bytes, { headers: { "Content-Type": "application/wasm" } });
      },
      atob: (value) => Buffer.from(value, "base64").toString("binary"),
    });
    context.self = context;
    context.window = context;
    vm.runInContext(await readFile(new URL("bundle.js", directory), "utf8"), context);
    assert.equal(await context.zkWasmSmoke, true);
    console.log(`${bundler}: browser bundle initialized WASM successfully`);
  }
} finally {
  await rm(output, { recursive: true, force: true });
}
