# ohttp-client

> ⚠️ **Work in progress.** APIs may change.

A minimal [Oblivious HTTP (RFC 9458)](https://www.rfc-editor.org/rfc/rfc9458)
client. It handles the parts every OHTTP client needs: BHTTP inner message
construction and parsing, request encapsulation / response decapsulation, and
[key config parsing (RFC 9540)](https://www.rfc-editor.org/rfc/rfc9540), so you
can tunnel requests through a relay to a gateway without exposing them to either.

## Sans-IO by default

By default the crate does **no network IO**. You encapsulate a request, send the
outer request with whatever HTTP client you like, then feed the raw response
bytes back in to decapsulate:

```rust
use ohttp_client::{OhttpClient, Url, parse_key_config};

let client = OhttpClient::new(
    Url::parse("https://relay.example/")?,
    Url::parse("https://target.example/")?,
    parse_key_config(&key_bytes)?, // GET the gateway's key endpoint yourself
);

let (req, ctx) = client.encapsulate("POST", "/resource",
    &[("content-type", "text/plain")], &[], Some(b"hello"))?;

let outer_response_body = send(&req);        // your HTTP client
let response = ctx.decapsulate(&outer_response_body)?;
```

This keeps the crate portable. It works anywhere, including in the browser.

## Feature flags

- **`bitreq`**: opt into doing the IO for you via
  [`bitreq`](https://crates.io/crates/bitreq). `OhttpClient::from_gateway`
  fetches the key config (tunneled through the relay) and a request builder
  handles encapsulate / send / decapsulate:
  `client.post("/resource").body("hello").send().await?`.
- **`wasm`**: [`wasm-bindgen`](https://rustwasm.github.io/wasm-bindgen/)
  exports for browser hosts, plus the JS RNG backend. Keep using the sans-IO
  flow from JS: encapsulate, `fetch` the outer request yourself, then
  decapsulate. See [`js/`](js/) for the wrapper and an example.

## WebAssembly

The crate builds for `wasm32-unknown-unknown`. Enable `wasm` for `wasm-bindgen`
exports, or `wasm_js` alone if you bind the Rust API without `wasm-bindgen`.
WASI targets need no extra feature.

## Building and testing

Common tasks are wrapped in the [`justfile`](justfile):

```sh
just test          # cargo test --all-features
just check-wasm    # verify the wasm32 build
just build-wasm    # build the web package into js/pkg/
just test-js       # build wasm and run the JS e2e
just check         # full suite: fmt, clippy, tests, wasm, js e2e, audit
```

Or directly with cargo:

```sh
cargo test --all-features
cargo check --target wasm32-unknown-unknown --features wasm
```

## License

See [LICENSE](LICENSE).
