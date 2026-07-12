//! Minimal sans-IO [Oblivious HTTP](https://www.rfc-editor.org/rfc/rfc9458) client.
//!
//! This crate handles the boilerplate every OHTTP client needs — BHTTP inner
//! message construction and parsing, encapsulation/decapsulation, and key
//! config parsing — without performing any network IO itself. You send the
//! outer request with whatever HTTP client you like and feed the raw response
//! bytes back in.
//!
//! ```no_run
//! use ohttp_client::{OhttpClient, Url};
//!
//! # fn send(req: &ohttp_client::OhttpRequest) -> Vec<u8> { unimplemented!() }
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // 1. GET the gateway's key endpoint yourself, then build a client.
//! let key_bytes: Vec<u8> = /* GET https://gateway.example/ohttp-keys */
//! #    vec![];
//! let client = OhttpClient::builder()
//!     .relay(Url::parse("https://relay.example/")?)
//!     .target(Url::parse("https://target.example/resource")?)
//!     .encoded_key_config(&key_bytes)?
//!     .build()?;
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
    #[error("builder missing required field: {0}")]
    MissingField(&'static str),
    #[error("inner message is not a response")]
    NotAResponse,
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

/// An OHTTP client bound to a relay, a target, and a gateway key config.
///
/// Build one with [`OhttpClient::builder`], then call [`encapsulate`] per
/// request. The target may differ from the gateway; the gateway URL itself is
/// never needed here — only its key config.
///
/// [`encapsulate`]: OhttpClient::encapsulate
#[derive(Debug, Clone)]
pub struct OhttpClient {
    key_config: KeyConfig,
    relay: Url,
    target: Url,
}

impl OhttpClient {
    pub fn builder() -> Builder {
        Builder::default()
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

/// Builder for [`OhttpClient`]. Relay, target, and key config are required.
#[derive(Debug, Default)]
pub struct Builder {
    relay: Option<Url>,
    target: Option<Url>,
    key_config: Option<KeyConfig>,
}

impl Builder {
    /// URL of the OHTTP relay the outer request is POSTed to.
    pub fn relay(mut self, url: Url) -> Self {
        self.relay = Some(url);
        self
    }

    /// URL of the resource the inner request is addressed to.
    pub fn target(mut self, url: Url) -> Self {
        self.target = Some(url);
        self
    }

    /// An already-parsed gateway key config.
    pub fn key_config(mut self, config: KeyConfig) -> Self {
        self.key_config = Some(config);
        self
    }

    /// Raw bytes of a gateway key endpoint response (`application/ohttp-keys`).
    pub fn encoded_key_config(mut self, bytes: &[u8]) -> Result<Self, Error> {
        self.key_config = Some(parse_key_config(bytes)?);
        Ok(self)
    }

    pub fn build(self) -> Result<OhttpClient, Error> {
        init();
        Ok(OhttpClient {
            relay: self.relay.ok_or(Error::MissingField("relay"))?,
            target: self.target.ok_or(Error::MissingField("target"))?,
            key_config: self.key_config.ok_or(Error::MissingField("key_config"))?,
        })
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
        OhttpClient::builder()
            .relay(Url::parse("https://relay.example/").unwrap())
            .target(Url::parse("https://target.example:8443/api/v1/thing?x=1").unwrap())
            .key_config(config)
            .build()
            .unwrap()
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

    #[test]
    fn builder_missing_fields() {
        let err = OhttpClient::builder().build().unwrap_err();
        assert!(matches!(err, Error::MissingField("relay")));
    }
}
