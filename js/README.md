# JS / wasm bindings

Thin `fetch` wrapper around the crate's `wasm-bindgen` exports. The compiled
wasm package lives in `pkg/` (generated; not checked in to version control).

## Prerequisites

- Rust toolchain with the `wasm32-unknown-unknown` target
- [`wasm-pack`](https://rustwasm.github.io/wasm-pack/)
- Node.js 18+

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-pack
```

## Build

From this directory:

```sh
npm run build
```

Or from the repo root:

```sh
just build-wasm
# equivalent:
wasm-pack build --target web --out-dir js/pkg --out-name ohttp_client -- --features wasm
```

That writes `js/pkg/` (`ohttp_client.js`, `ohttp_client_bg.wasm`, typings).

## Use

```js
import { init, OhttpClient, send } from './index.js';

// Browser: wasm-pack's default init can locate the .wasm next to the JS.
await init();

// Node: pass the wasm bytes explicitly.
// import { readFileSync } from 'node:fs';
// await init({ module_or_path: readFileSync('./pkg/ohttp_client_bg.wasm') });

const keys = new Uint8Array(await (await fetch(gatewayKeysUrl)).arrayBuffer());
// `targetUrl` is the origin; pass the path (or full URL) per request.
const client = new OhttpClient(relayUrl, targetUrl, keys);

const encapsulated = client
  .encapsulate('POST', '/resource')
  .header('content-type', 'text/plain')
  .body(new TextEncoder().encode('hello'))
  .build();

const response = await send(encapsulated);
console.log(response.status, new TextDecoder().decode(response.body));
```

`send` POSTs to the relay with `fetch` and decapsulates. For a custom HTTP
stack, call `fetch` yourself and then `encapsulated.decapsulate(bytes)`.

## Test

Needs a built `pkg/` and a Rust toolchain (spawns `ohttp-test-harness`):

```sh
npm run build && npm test
# or from repo root:
just test-js
```
