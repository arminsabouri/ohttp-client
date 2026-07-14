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

mod error;
pub use error::Error;

#[cfg(feature = "bitreq")]
mod http;
#[cfg(feature = "bitreq")]
pub use http::{RequestBuilder, fetch_key_config, fetch_key_config_via_relay};

/// Media type of an encapsulated request, per RFC 9458.
pub const OHTTP_REQ_CONTENT_TYPE: &str = "message/ohttp-req";

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
