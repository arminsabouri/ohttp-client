//! `wasm-bindgen` surface for browser hosts that do their own `fetch`.
//!
//! Enable with `--features wasm` (implies `wasm_js`).

use wasm_bindgen::prelude::*;

use crate::{parse_key_config, OhttpClient, Response, ResponseContext, Url};

fn js_err(err: impl std::fmt::Display) -> JsError {
    JsError::new(&err.to_string())
}

/// Browser-facing OHTTP client. Encapsulate here; send the outer request with
/// `fetch` (or equivalent), then [`Encapsulated::decapsulate`].
///
/// Construct with a target origin; pass a path on that origin to
/// [`encapsulate`](WasmOhttpClient::encapsulate) per request.
///
/// # Key rotation
///
/// A gateway that has rotated its keys cannot decapsulate requests sealed to
/// the old one and answers 400 (or 401). Because the host does its own `fetch`,
/// it sees that status directly:
///
/// ```js
/// let res = await fetch(enc.url, { method: 'POST', headers, body: enc.body });
/// if (res.status === 400 || res.status === 401) {
///   const keys = await fetch(gatewayKeyUrl).then(r => r.arrayBuffer());
///   // `false` means the gateway still advertises the same key — the 400 was
///   // something else, so do not resend.
///   if (client.setKeyConfig(new Uint8Array(keys))) {
///     enc = client.encapsulate(method, path).build();  // resend once
///     res = await fetch(enc.url, { method: 'POST', headers, body: enc.body });
///   }
/// }
/// ```
///
/// The old [`Encapsulated`] is spent either way — build a fresh one to retry.
#[wasm_bindgen(js_name = OhttpClient)]
pub struct WasmOhttpClient {
    inner: OhttpClient,
}

#[wasm_bindgen(js_class = OhttpClient)]
impl WasmOhttpClient {
    #[wasm_bindgen(constructor)]
    pub fn new(relay: &str, target: &str, key_config: &[u8]) -> Result<WasmOhttpClient, JsError> {
        let relay = Url::parse(relay).map_err(js_err)?;
        let target = Url::parse(target).map_err(js_err)?;
        let key_config = parse_key_config(key_config).map_err(js_err)?;
        Ok(Self {
            inner: OhttpClient::new(relay, target, key_config),
        })
    }

    /// Pad every encapsulated request's BHTTP plaintext to exactly `n` bytes.
    pub fn known_length(mut self, n: usize) -> WasmOhttpClient {
        self.inner = self.inner.known_length(n);
        self
    }

    /// Adopt a freshly fetched `application/ohttp-keys` body, returning whether
    /// the gateway's key actually changed.
    ///
    /// Call this after a 400/401 from the relay; see the class docs. Builders
    /// created earlier share this client's config, so they pick up the new key
    /// too — but only at `build()`, so anything already encapsulated is stale.
    #[wasm_bindgen(js_name = setKeyConfig)]
    pub fn set_key_config(&self, key_config: &[u8]) -> Result<bool, JsError> {
        let key_config = parse_key_config(key_config).map_err(js_err)?;
        Ok(self.inner.set_key_config(key_config))
    }

    /// Start building an encapsulated request to `path` on the target origin.
    pub fn encapsulate(&self, method: &str, path: &str) -> EncapsulateBuilder {
        EncapsulateBuilder {
            client: self.inner.clone(),
            method: method.to_owned(),
            path: path.to_owned(),
            headers: Vec::new(),
            query: Vec::new(),
            body: None,
        }
    }
}

/// Builder for a single encapsulated request.
#[wasm_bindgen]
pub struct EncapsulateBuilder {
    client: OhttpClient,
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    query: Vec<(String, String)>,
    body: Option<Vec<u8>>,
}

#[wasm_bindgen]
impl EncapsulateBuilder {
    pub fn header(mut self, name: &str, value: &str) -> EncapsulateBuilder {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    pub fn param(mut self, key: &str, value: &str) -> EncapsulateBuilder {
        self.query.push((key.to_owned(), value.to_owned()));
        self
    }

    pub fn body(mut self, body: Vec<u8>) -> EncapsulateBuilder {
        self.body = Some(body);
        self
    }

    pub fn build(self) -> Result<Encapsulated, JsError> {
        let headers: Vec<(&str, &str)> = self
            .headers
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        let query: Vec<(&str, &str)> = self
            .query
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let (req, ctx) = self
            .client
            .encapsulate(
                &self.method,
                &self.path,
                &headers,
                &query,
                self.body.as_deref(),
            )
            .map_err(js_err)?;
        Ok(Encapsulated {
            url: req.url.to_string(),
            content_type: req.content_type.to_owned(),
            body: req.body,
            context: ctx,
        })
    }
}

/// Outer request plus the one-shot decapsulation context.
#[wasm_bindgen]
pub struct Encapsulated {
    url: String,
    content_type: String,
    body: Vec<u8>,
    context: ResponseContext,
}

#[wasm_bindgen]
impl Encapsulated {
    #[wasm_bindgen(getter)]
    pub fn url(&self) -> String {
        self.url.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn content_type(&self) -> String {
        self.content_type.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn body(&self) -> Vec<u8> {
        self.body.clone()
    }

    /// Decapsulate the raw body of the relay's response.
    pub fn decapsulate(self, response_body: &[u8]) -> Result<WasmResponse, JsError> {
        self.context
            .decapsulate(response_body)
            .map(WasmResponse)
            .map_err(js_err)
    }
}

/// Decapsulated inner HTTP response.
#[wasm_bindgen(js_name = OhttpResponse)]
pub struct WasmResponse(Response);

#[wasm_bindgen(js_class = OhttpResponse)]
impl WasmResponse {
    #[wasm_bindgen(getter)]
    pub fn status(&self) -> u16 {
        self.0.status()
    }

    #[wasm_bindgen(getter)]
    pub fn body(&self) -> Vec<u8> {
        self.0.body().to_vec()
    }

    /// Value of the first header with the given name (case-insensitive).
    pub fn header(&self, name: &str) -> Option<Vec<u8>> {
        self.0.header(name).map(|v| v.to_vec())
    }
}
