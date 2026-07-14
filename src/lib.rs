//! Minimal sans-IO [Oblivious HTTP](https://www.rfc-editor.org/rfc/rfc9458) client.
//!
//! This crate handles the boilerplate every OHTTP client needs — BHTTP inner
//! message construction and parsing, encapsulation/decapsulation, and key
//! config parsing — without performing any network IO itself. You send the
//! outer request with whatever HTTP client you like and feed the raw response
//! bytes back in.
//!
//! ```no_run
//! use ohttp_client::{OhttpClient, Url, parse_key_config};
//!
//! # fn send(req: &ohttp_client::OhttpRequest) -> Vec<u8> { unimplemented!() }
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // 1. GET the gateway's key endpoint yourself, then build a client.
//! let key_bytes: Vec<u8> = /* GET https://gateway.example/ohttp-keys */
//! #    vec![];
//! let client = OhttpClient::new(
//!     Url::parse("https://relay.example/")?,
//!     Url::parse("https://target.example/resource")?,
//!     parse_key_config(&key_bytes)?,
//! );
//!
//! // 2. Encapsulate; POST `req.body` to `req.url` with `req.content_type`.
//! let (req, ctx) = client.encapsulate("POST", &[("content-type", "text/plain")], Some(b"hello"))?;
//! let outer_response_body = send(&req);
//!
//! // 3. Decapsulate the raw response body to get the inner response.
//! let response = ctx.decapsulate(&outer_response_body)?;
//! assert_eq!(response.status(), 200);
//! # Ok(())
//! # }
//! ```
//!
//! With the optional `bitreq` feature, `bitreq` can also do the IO for you:
//! [`OhttpClient::from_gateway`] fetches the key config (tunneled through the
//! relay) and builds the client, and a request builder does encapsulate/send/
//! decapsulate:
//! `client.post().header("content-type", "text/plain").body("hello").send().await?`.

use std::io::Cursor;
use std::sync::Once;

use bhttp::{Message, Mode};

pub use ohttp::KeyConfig;
pub use url::Url;

/// Media type of an encapsulated request, per RFC 9458.
pub const OHTTP_REQ_CONTENT_TYPE: &str = "message/ohttp-req";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("ohttp: {0}")]
    Ohttp(#[from] ohttp::Error),
    #[error("bhttp: {0}")]
    Bhttp(#[from] bhttp::Error),
    #[error("no key config found in response")]
    NoKeyConfig,
    #[error("inner message is not a response")]
    NotAResponse,
    #[cfg(feature = "bitreq")]
    #[error("bitreq: {0}")]
    Bitreq(#[from] bitreq::Error),
    #[cfg(feature = "bitreq")]
    #[error("relay returned unexpected status: {0}")]
    UnexpectedStatus(i32),
    #[cfg(feature = "bitreq")]
    #[error("relay url has no host")]
    NoRelayHost,
}

/// Parse the body of a gateway key endpoint response
/// (`application/ohttp-keys`, RFC 9540) into the first usable [`KeyConfig`].
pub fn parse_key_config(bytes: &[u8]) -> Result<KeyConfig, Error> {
    init();
    KeyConfig::decode_list(bytes)?
        .into_iter()
        .next()
        .ok_or(Error::NoKeyConfig)
}

fn init() {
    static INIT: Once = Once::new();
    INIT.call_once(ohttp::init);
}

/// GET the gateway's key endpoint with `bitreq` and parse the result.
///
/// Available with the `bitreq` feature.
#[cfg(feature = "bitreq")]
pub async fn fetch_key_config(gateway_key_url: &str) -> Result<KeyConfig, Error> {
    let res = bitreq::get(gateway_key_url).send_async().await?;
    if res.status_code != 200 {
        return Err(Error::UnexpectedStatus(res.status_code));
    }
    parse_key_config(res.as_bytes())
}

/// GET the gateway's key endpoint tunneled through the relay via HTTP
/// `CONNECT`, rather than dialing the gateway directly.
///
/// A direct GET reveals the client's IP address to the gateway before any
/// encapsulated request is ever sent, defeating the IP-hiding purpose of
/// routing those requests through a relay. Tunneling the key fetch through
/// the relay too (as `ohttp-relay`'s `connect-bootstrap` feature supports)
/// means the gateway only ever sees the relay's IP. Available with the
/// `bitreq` feature.
#[cfg(feature = "bitreq")]
pub async fn fetch_key_config_via_relay(
    gateway_key_url: &str,
    relay_url: &Url,
) -> Result<KeyConfig, Error> {
    let host = relay_url.host_str().ok_or(Error::NoRelayHost)?;
    let port = relay_url
        .port_or_known_default()
        .ok_or(Error::NoRelayHost)?;
    let proxy = bitreq::Proxy::new_http(format!("{host}:{port}"))?;
    let res = bitreq::get(gateway_key_url)
        .with_proxy(proxy)
        .send_async()
        .await?;
    if res.status_code != 200 {
        return Err(Error::UnexpectedStatus(res.status_code));
    }
    parse_key_config(res.as_bytes())
}

/// An OHTTP client bound to a relay, a target, and a gateway key config.
///
/// Construct with [`OhttpClient::new`] when you already have a key config, or
/// with [`OhttpClient::from_gateway`] (requires the `bitreq` feature) to fetch
/// the key config through the relay. Then call [`encapsulate`] per request.
/// The target may differ from the gateway; after construction the gateway URL
/// itself is not needed — only its key config.
///
/// [`encapsulate`]: OhttpClient::encapsulate
#[derive(Debug, Clone)]
pub struct OhttpClient {
    key_config: KeyConfig,
    relay: Url,
    target: Url,
}

impl OhttpClient {
    /// Bind a client to a relay, target, and already-known gateway key config.
    pub fn new(relay: Url, target: Url, key_config: KeyConfig) -> Self {
        init();
        Self {
            relay,
            target,
            key_config,
        }
    }

    /// Encapsulate an inner request to the target.
    ///
    /// Returns the outer request to POST to the relay yourself, and the
    /// one-shot context that decapsulates the corresponding response body.
    pub fn encapsulate(
        &self,
        method: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
    ) -> Result<(OhttpRequest, ResponseContext), Error> {
        let authority = self.target[url::Position::BeforeHost..url::Position::AfterPort].as_bytes();
        let path = self.target[url::Position::BeforePath..].as_bytes();
        let mut inner = Message::request(
            method.as_bytes().to_vec(),
            self.target.scheme().as_bytes().to_vec(),
            authority.to_vec(),
            path.to_vec(),
        );
        for (name, value) in headers {
            inner.put_header(name.as_bytes(), value.as_bytes());
        }
        if let Some(body) = body {
            inner.write_content(body);
        }
        let mut bhttp_bytes = Vec::new();
        inner.write_bhttp(Mode::KnownLength, &mut bhttp_bytes)?;

        let (encapsulated, ctx) = ohttp::ClientRequest::from_config(&mut self.key_config.clone())?
            .encapsulate(&bhttp_bytes)?;
        Ok((
            OhttpRequest {
                url: self.relay.clone(),
                content_type: OHTTP_REQ_CONTENT_TYPE,
                body: encapsulated,
            },
            ResponseContext(ctx),
        ))
    }
}

#[cfg(feature = "bitreq")]
impl OhttpClient {
    /// Fetch the gateway key config through the relay (HTTP `CONNECT`) and
    /// build a client.
    ///
    /// Prefer this over dialing the gateway yourself and calling [`new`]: the
    /// gateway never sees the caller's IP, even for the bootstrap key fetch.
    /// See [`fetch_key_config_via_relay`].
    pub async fn from_gateway(
        relay: Url,
        target: Url,
        gateway_key_url: &str,
    ) -> Result<Self, Error> {
        let key_config = fetch_key_config_via_relay(gateway_key_url, &relay).await?;
        Ok(Self::new(relay, target, key_config))
    }

    /// Start building an inner request with the given method.
    ///
    /// Available with the `bitreq` feature; [`RequestBuilder::send`]
    /// encapsulates, sends via `bitreq`, and decapsulates in one call.
    pub fn request(&self, method: impl Into<String>) -> RequestBuilder<'_> {
        RequestBuilder {
            client: self,
            method: method.into(),
            headers: Vec::new(),
            body: None,
        }
    }

    /// Shorthand for [`request("GET")`](Self::request).
    pub fn get(&self) -> RequestBuilder<'_> {
        self.request("GET")
    }

    /// Shorthand for [`request("POST")`](Self::request).
    pub fn post(&self) -> RequestBuilder<'_> {
        self.request("POST")
    }
}

/// Builds an inner request against the client's target, then sends it through
/// the relay with `bitreq`. Created by [`OhttpClient::request`] and friends.
#[cfg(feature = "bitreq")]
#[must_use = "call `send()` to perform the request"]
pub struct RequestBuilder<'a> {
    client: &'a OhttpClient,
    method: String,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
}

#[cfg(feature = "bitreq")]
impl RequestBuilder<'_> {
    /// Add a header to the inner request.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Set the inner request body.
    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// Encapsulate the inner request, POST it to the relay, and decapsulate
    /// the inner response.
    pub async fn send(self) -> Result<Response, Error> {
        let headers: Vec<(&str, &str)> = self
            .headers
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        let (req, ctx) = self
            .client
            .encapsulate(&self.method, &headers, self.body.as_deref())?;
        let res = bitreq::post(req.url.as_str())
            .with_header("content-type", req.content_type)
            .with_body(req.body)
            .send_async()
            .await?;
        if res.status_code != 200 {
            return Err(Error::UnexpectedStatus(res.status_code));
        }
        ctx.decapsulate(res.as_bytes())
    }
}

/// The outer request to send to the relay with your own HTTP client:
/// `POST {url}` with `Content-Type: {content_type}` and `{body}` as the body.
#[derive(Debug, Clone)]
pub struct OhttpRequest {
    pub url: Url,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

/// One-shot context to decapsulate the response to a single encapsulated
/// request. Consumed by [`decapsulate`](ResponseContext::decapsulate).
pub struct ResponseContext(ohttp::ClientResponse);

impl ResponseContext {
    /// Decapsulate the raw body of the relay's response into the inner response.
    pub fn decapsulate(self, encapsulated: &[u8]) -> Result<Response, Error> {
        let bhttp_bytes = self.0.decapsulate(encapsulated)?;
        let inner = Message::read_bhttp(&mut Cursor::new(&bhttp_bytes[..]))?;
        let status = inner.control().status().ok_or(Error::NotAResponse)?.code();
        let headers = inner
            .header()
            .iter()
            .map(|f| (f.name().to_vec(), f.value().to_vec()))
            .collect();
        Ok(Response {
            status,
            headers,
            body: inner.content().to_vec(),
        })
    }
}

/// The decapsulated inner response from the target.
#[derive(Debug, Clone)]
pub struct Response {
    status: u16,
    headers: Vec<(Vec<u8>, Vec<u8>)>,
    body: Vec<u8>,
}

impl Response {
    pub fn status(&self) -> u16 {
        self.status
    }

    /// Value of the first header with the given name (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&[u8]> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name.as_bytes()))
            .map(|(_, v)| v.as_slice())
    }

    pub fn headers(&self) -> impl Iterator<Item = (&[u8], &[u8])> {
        self.headers
            .iter()
            .map(|(n, v)| (n.as_slice(), v.as_slice()))
    }

    pub fn body(&self) -> &[u8] {
        &self.body
    }

    pub fn into_body(self) -> Vec<u8> {
        self.body
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ohttp::SymmetricSuite;
    use ohttp::hpke::{Aead, Kdf, Kem};

    fn test_key_config() -> KeyConfig {
        init();
        KeyConfig::new(
            1,
            Kem::X25519Sha256,
            vec![SymmetricSuite::new(Kdf::HkdfSha256, Aead::ChaCha20Poly1305)],
        )
        .unwrap()
    }

    fn test_client(config: KeyConfig) -> OhttpClient {
        OhttpClient::new(
            Url::parse("https://relay.example/").unwrap(),
            Url::parse("https://target.example:8443/api/v1/thing?x=1").unwrap(),
            config,
        )
    }

    #[test]
    fn round_trip() {
        let server = ohttp::Server::new(test_key_config()).unwrap();
        let client = test_client(server.config().clone());

        let (req, ctx) = client
            .encapsulate("POST", &[("content-type", "text/plain")], Some(b"hello"))
            .unwrap();
        assert_eq!(req.url.as_str(), "https://relay.example/");
        assert_eq!(req.content_type, "message/ohttp-req");

        // Gateway side: decapsulate and inspect the inner request.
        let (inner_bytes, server_ctx) = server.decapsulate(&req.body).unwrap();
        let inner = Message::read_bhttp(&mut Cursor::new(&inner_bytes[..])).unwrap();
        assert_eq!(inner.control().method(), Some(&b"POST"[..]));
        assert_eq!(inner.control().scheme(), Some(&b"https"[..]));
        assert_eq!(
            inner.control().authority(),
            Some(&b"target.example:8443"[..])
        );
        assert_eq!(inner.control().path(), Some(&b"/api/v1/thing?x=1"[..]));
        assert_eq!(
            inner.header().get(b"content-type"),
            Some(&b"text/plain"[..])
        );
        assert_eq!(inner.content(), b"hello");

        // Gateway responds; client decapsulates the inner response.
        let mut inner_res = Message::response(bhttp::StatusCode::try_from(200u16).unwrap());
        inner_res.put_header("content-type", "application/json");
        inner_res.write_content(b"{\"ok\":true}");
        let mut res_bytes = Vec::new();
        inner_res
            .write_bhttp(Mode::KnownLength, &mut res_bytes)
            .unwrap();
        let enc_res = server_ctx.encapsulate(&res_bytes).unwrap();

        let response = ctx.decapsulate(&enc_res).unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(
            response.header("Content-Type"),
            Some(&b"application/json"[..])
        );
        assert_eq!(response.body(), b"{\"ok\":true}");
    }

    #[test]
    fn parse_key_config_list() {
        let config = test_key_config();
        let encoded = KeyConfig::encode_list(&[&config]).unwrap();
        let parsed = parse_key_config(&encoded).unwrap();
        assert_eq!(parsed.encode().unwrap(), config.encode().unwrap());

        assert!(matches!(parse_key_config(&[]), Err(Error::NoKeyConfig)));
    }
}
