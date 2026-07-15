#!/usr/bin/env node
/**
 * JS e2e: spawn the Rust test harness, encapsulate via wasm, fetch via
 * {@link send}, assert the echoed response.
 */

import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { init, OhttpClient, send } from './index.js';

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, '..');
const wasmPath = join(here, 'pkg', 'ohttp_client_bg.wasm');

await init({ module_or_path: readFileSync(wasmPath) });

const harness = spawn(
  'cargo',
  ['run', '--quiet', '--features', 'harness', '--bin', 'ohttp-test-harness'],
  { cwd: root, stdio: ['ignore', 'pipe', 'inherit'] },
);

const urls = await new Promise((resolve, reject) => {
  const rl = createInterface({ input: harness.stdout });
  const timer = setTimeout(() => reject(new Error('harness timed out')), 120_000);
  rl.on('line', (line) => {
    const prefix = 'OHTTP_HARNESS\t';
    if (!line.startsWith(prefix)) return;
    clearTimeout(timer);
    rl.close();
    try {
      resolve(JSON.parse(line.slice(prefix.length)));
    } catch (err) {
      reject(err);
    }
  });
  harness.once('exit', (code) => {
    clearTimeout(timer);
    reject(new Error(`harness exited early (code ${code})`));
  });
  harness.once('error', reject);
});

try {
  const keysRes = await fetch(urls.gateway_url);
  if (!keysRes.ok) throw new Error(`key fetch failed: ${keysRes.status}`);
  const keys = new Uint8Array(await keysRes.arrayBuffer());

  const client = new OhttpClient(urls.relay_url, `${urls.target_url}/echo`, keys);
  const encapsulated = client
    .encapsulate('POST')
    .header('content-type', 'text/plain')
    .param('x', '1')
    .body(new TextEncoder().encode('hello'))
    .build();

  const response = await send(encapsulated);
  const body = new TextDecoder().decode(response.body);

  if (response.status !== 200) throw new Error(`status ${response.status}`);
  if (body !== 'POST /echo?x=1 hello') throw new Error(`body: ${JSON.stringify(body)}`);

  console.log('ok');
} finally {
  harness.kill('SIGTERM');
}
