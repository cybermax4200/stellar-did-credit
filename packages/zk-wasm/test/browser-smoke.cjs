const assert = require('node:assert/strict');
const fs = require('node:fs');
const http = require('node:http');
const path = require('node:path');
const { spawn } = require('node:child_process');

const root = path.resolve('pkg-web');
const browser = process.env.CHROME_BIN || (process.platform === 'darwin'
  ? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' : 'google-chrome');
const bytes = new Uint8Array(32);
const args = [50, 50_000_000_000, 10, 50, 100, 20_000_000_000, 40, 30, 30,
  10, 1_700_000_000n, 1234567, false, 42, 600, bytes, bytes, 12345, bytes];
const expected = Buffer.from(require(path.resolve('pkg/zk_wasm_cjs.cjs')).generate_score_proof(...args)).toString('hex');
const html = `<!doctype html><script type="module">
import init, { generate_score_proof } from '/zk_wasm.js';
try {
  await init();
  const b = new Uint8Array(32);
  const proof = generate_score_proof(50, 50000000000, 10, 50, 100, 20000000000, 40, 30, 30,
    10, 1700000000n, 1234567, false, 42, 600, b, b, 12345, b);
  await fetch('/result', { method: 'POST', body: Array.from(proof, x => x.toString(16).padStart(2, '0')).join('') });
} catch (e) { await fetch('/error', { method: 'POST', body: String(e) }); }
</script>`;

const server = http.createServer((req, res) => {
  if (req.method === 'POST') {
    let body = '';
    req.on('data', chunk => { body += chunk; });
    req.on('end', () => {
      res.end('ok');
      try {
        assert.equal(req.url, '/result', body);
        assert.equal(body, expected, 'browser and Node proofs differ');
        console.log('Chromium generated the same proof as Node');
        finish(0);
      } catch (error) { console.error(error); finish(1); }
    });
  } else if (req.url === '/') {
    res.writeHead(200, { 'Content-Type': 'text/html' }); res.end(html);
  } else if (req.url === '/zk_wasm.js' || req.url === '/zk_wasm_bg.wasm') {
    res.writeHead(200, { 'Content-Type': req.url.endsWith('.wasm') ? 'application/wasm' : 'text/javascript' });
    res.end(fs.readFileSync(path.join(root, req.url.slice(1))));
  } else { res.writeHead(404); res.end(); }
});
let child;
const timer = setTimeout(() => { console.error('Chromium smoke test timed out'); finish(1); }, 60000);
function finish(code) {
  clearTimeout(timer);
  if (child) child.kill();
  server.close();
  process.exitCode = code;
}
server.listen(0, '127.0.0.1', () => {
  child = spawn(browser, ['--headless', '--no-sandbox', '--disable-gpu', `http://127.0.0.1:${server.address().port}/`], { stdio: 'ignore' });
  child.on('error', error => { console.error(error); finish(1); });
  child.on('exit', code => { if (code && process.exitCode === undefined) { console.error(`Chromium exited: ${code}`); finish(1); } });
});
