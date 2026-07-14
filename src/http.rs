//! Optional `bitreq` integration: key-config fetch and an async request builder
//! that encapsulate / send / decapsulate in one call.

use crate::{Error, KeyConfig, OhttpClient, Response, Url, parse_key_config};

/// GET the gateway's key endpoint with `bitreq` and parse the result.
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
/// means the gateway only ever sees the relay's IP.
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

impl OhttpClient {
    /// Fetch the gateway key config through the relay (HTTP `CONNECT`) and
    /// build a client.
    ///
    /// Prefer this over dialing the gateway yourself and calling [`Self::new`]:
    /// the gateway never sees the caller's IP, even for the bootstrap key
    /// fetch. See [`fetch_key_config_via_relay`].
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
    /// [`RequestBuilder::send`] encapsulates, sends via `bitreq`, and
    /// decapsulates in one call.
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
#[must_use = "call `send()` to perform the request"]
pub struct RequestBuilder<'a> {
    client: &'a OhttpClient,
    method: String,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
}

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
