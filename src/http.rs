//! Optional `bitreq` integration: key-config fetch and an async request builder
//! that encapsulate / send / decapsulate in one call.

use crate::{parse_key_config, Error, KeyConfig, OhttpClient, Response, Url};

/// How to reach the gateway's key endpoint when refetching after a rotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyFetch {
    /// Tunnel the GET through the relay with HTTP `CONNECT`, so the gateway
    /// only ever sees the relay's IP. What [`OhttpClient::from_gateway`] uses.
    /// Requires a relay that offers `CONNECT` bootstrap.
    ViaRelay,
    /// GET the gateway directly. Reveals the client's IP to the gateway, which
    /// is what routing through a relay exists to prevent — pick this only when
    /// the relay does not tunnel, or when the gateway already knows the client.
    Direct,
}

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
    ///
    /// `target` is the origin used to resolve per-request paths (see
    /// [`Self::new`]).
    pub async fn from_gateway(
        relay: Url,
        target: Url,
        gateway_key_url: &str,
    ) -> Result<Self, Error> {
        let key_config = fetch_key_config_via_relay(gateway_key_url, &relay).await?;
        Ok(Self::new(relay, target, key_config)
            .with_key_refresh(gateway_key_url, KeyFetch::ViaRelay))
    }

    /// Let this client recover from key rotations by refetching
    /// `gateway_key_url` over `route`.
    ///
    /// [`Self::from_gateway`] sets this up already. Use it on a client built
    /// with [`Self::new`], or to override the route — notably
    /// [`KeyFetch::Direct`] when the relay does not offer `CONNECT` bootstrap.
    pub fn with_key_refresh(mut self, gateway_key_url: impl Into<String>, route: KeyFetch) -> Self {
        self.key_refresh = Some((gateway_key_url.into(), route));
        self
    }

    /// Refetch the gateway's key config and adopt it, returning whether it
    /// actually changed.
    ///
    /// Recovers from a rotation, after which the gateway rejects requests
    /// sealed to the key it retired. [`RequestBuilder::send`] calls this for
    /// you; call it directly only if you drive [`OhttpClient::encapsulate`]
    /// yourself.
    ///
    /// Returns [`Error::NoGatewayKeyUrl`] if neither [`Self::from_gateway`] nor
    /// [`Self::with_key_refresh`] gave the client a key endpoint to refetch.
    pub async fn refresh_key_config(&self) -> Result<bool, Error> {
        let (url, route) = self.key_refresh.as_ref().ok_or(Error::NoGatewayKeyUrl)?;
        let key_config = match route {
            KeyFetch::ViaRelay => fetch_key_config_via_relay(url, &self.relay).await?,
            KeyFetch::Direct => fetch_key_config(url).await?,
        };
        Ok(self.set_key_config(key_config))
    }

    /// Start building an inner request with the given method and path on the
    /// client's target origin.
    ///
    /// [`RequestBuilder::send`] encapsulates, sends via `bitreq`, and
    /// decapsulates in one call. `path` must stay on the target origin (see
    /// [`OhttpClient::encapsulate`]).
    pub fn request(
        &self,
        method: impl Into<String>,
        path: impl Into<String>,
    ) -> RequestBuilder<'_> {
        RequestBuilder {
            client: self,
            method: method.into(),
            path: path.into(),
            headers: Vec::new(),
            params: Vec::new(),
            body: None,
        }
    }

    /// Shorthand for [`request("GET", path)`](Self::request).
    pub fn get(&self, path: impl Into<String>) -> RequestBuilder<'_> {
        self.request("GET", path)
    }

    /// Shorthand for [`request("HEAD", path)`](Self::request).
    pub fn head(&self, path: impl Into<String>) -> RequestBuilder<'_> {
        self.request("HEAD", path)
    }

    /// Shorthand for [`request("POST", path)`](Self::request).
    pub fn post(&self, path: impl Into<String>) -> RequestBuilder<'_> {
        self.request("POST", path)
    }

    /// Shorthand for [`request("PUT", path)`](Self::request).
    pub fn put(&self, path: impl Into<String>) -> RequestBuilder<'_> {
        self.request("PUT", path)
    }

    /// Shorthand for [`request("DELETE", path)`](Self::request).
    pub fn delete(&self, path: impl Into<String>) -> RequestBuilder<'_> {
        self.request("DELETE", path)
    }

    /// Shorthand for [`request("CONNECT", path)`](Self::request).
    pub fn connect(&self, path: impl Into<String>) -> RequestBuilder<'_> {
        self.request("CONNECT", path)
    }

    /// Shorthand for [`request("OPTIONS", path)`](Self::request).
    pub fn options(&self, path: impl Into<String>) -> RequestBuilder<'_> {
        self.request("OPTIONS", path)
    }

    /// Shorthand for [`request("TRACE", path)`](Self::request).
    pub fn trace(&self, path: impl Into<String>) -> RequestBuilder<'_> {
        self.request("TRACE", path)
    }

    /// Shorthand for [`request("PATCH", path)`](Self::request).
    pub fn patch(&self, path: impl Into<String>) -> RequestBuilder<'_> {
        self.request("PATCH", path)
    }
}

/// Builds an inner request against a path on the client's target origin, then
/// sends it through the relay with `bitreq`. Created by [`OhttpClient::request`]
/// and friends.
#[must_use = "call `send()` to perform the request"]
pub struct RequestBuilder<'a> {
    client: &'a OhttpClient,
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    params: Vec<(String, String)>,
    body: Option<Vec<u8>>,
}

impl RequestBuilder<'_> {
    /// Add a header to the inner request.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Add a query parameter to the inner request URL.
    ///
    /// The key and value are percent-encoded when the request is sent.
    /// Parameters are appended after any query already present on the
    /// resolved path/URL.
    pub fn param(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.params.push((key.into(), value.into()));
        self
    }

    /// Set the inner request body.
    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// Encapsulate the inner request, POST it to the relay, and decapsulate
    /// the inner response.
    ///
    /// If the gateway rejects the request in a way consistent with a key
    /// rotation ([`Error::is_possibly_stale_key`]), the client's key config is
    /// refetched and the request is sent once more — at most two attempts, no
    /// backoff. The retry is skipped, and the original error returned, when the
    /// refetch fails or comes back with the same key: a 400 that survives an
    /// unchanged key was never about the key.
    ///
    /// Two requests failing concurrently will each refetch. That is harmless
    /// (the fetch is an idempotent GET and the configs agree), so there is no
    /// single-flight guard.
    pub async fn send(self) -> Result<Response, Error> {
        let err = match self.attempt().await {
            Ok(response) => return Ok(response),
            Err(err) => err,
        };
        if !err.is_possibly_stale_key() {
            return Err(err);
        }
        // Report the request's failure, not the recovery's: a refetch that
        // errors or returns the same key leaves the original error the
        // meaningful one.
        match self.client.refresh_key_config().await {
            Ok(true) => self.attempt().await,
            Ok(false) | Err(_) => Err(err),
        }
    }

    async fn attempt(&self) -> Result<Response, Error> {
        let headers: Vec<(&str, &str)> = self
            .headers
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        let query: Vec<(&str, &str)> = self
            .params
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let (req, ctx) = self.client.encapsulate(
            &self.method,
            &self.path,
            &headers,
            &query,
            self.body.as_deref(),
        )?;
        let res = bitreq::post(req.url.as_str())
            .with_header("content-type", req.content_type)
            .with_body(req.body)
            .send_async()
            .await?;
        // TODO: other status codes?
        if res.status_code != 200 {
            return Err(Error::UnexpectedStatus(res.status_code));
        }
        ctx.decapsulate(res.as_bytes())
    }
}
