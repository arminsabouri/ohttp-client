# OHTTP Client

A minimal [Oblivious HTTP (RFC 9458)](https://www.rfc-editor.org/rfc/rfc9458)
client. It handles the parts every OHTTP client needs: BHTTP inner message
construction and parsing, request encapsulation / response decapsulation, and
[key config parsing (RFC 9540)](https://www.rfc-editor.org/rfc/rfc9540), so you
can tunnel requests through a relay to a gateway without exposing them to either.

This library is a thin wrapper over the [rust crate](https://crates.io/crates/ohttp-client) using WASM bindings.
It supports the X25519, P-256, and X-Wing KEMs.

## Prerequisites

- Rust toolchain with the `wasm32-unknown-unknown` target
- [`wasm-pack`](https://rustwasm.github.io/wasm-pack/)
- Node.js 18+

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-pack
```

## Build
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
