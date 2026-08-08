#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("ohttp: {0}")]
    Ohttp(#[from] ohttp::Error),
    #[error("bhttp: {0}")]
    Bhttp(#[from] bhttp::Error),
    #[error("url: {0}")]
    Url(#[from] url::ParseError),
    #[error("getrandom: {0}")]
    GetRandom(#[from] getrandom::Error),
    #[error("bhttp payload ({needed} bytes) exceeds known length ({known_length})")]
    KnownLengthTooSmall { needed: usize, known_length: usize },
    #[error("no key config found in response")]
    NoKeyConfig,
    #[error("inner message is not a response")]
    NotAResponse,
    #[error("path must stay on the client's target origin")]
    PathEscapesOrigin,
    #[cfg(feature = "bitreq")]
    #[error("bitreq: {0}")]
    Bitreq(#[from] bitreq::Error),
    #[cfg(feature = "bitreq")]
    #[error("relay returned unexpected status: {0}")]
    UnexpectedStatus(i32),
    #[cfg(feature = "bitreq")]
    #[error("relay url has no host")]
    NoRelayHost,
    #[cfg(feature = "bitreq")]
    #[error("client has no gateway key url; use `from_gateway` or `with_key_refresh`")]
    NoGatewayKeyUrl,
}

impl Error {
    /// Whether this failure is consistent with the gateway having rotated its
    /// keys since the client's config was fetched.
    ///
    /// A gateway that cannot decapsulate a request answers 4xx (RFC 9458
    /// §4.6), and holding a retired key id is the usual reason. Only 400 and
    /// 401 count: a relay's own 404 or 429 says nothing about keys, and
    /// refetching on those would waste a round-trip on every rate limit.
    ///
    /// Recover by refetching the gateway's key endpoint and passing the result
    /// to [`OhttpClient::set_key_config`](crate::OhttpClient::set_key_config),
    /// retrying only if it reports a change.
    pub fn is_possibly_stale_key(&self) -> bool {
        #[cfg(feature = "bitreq")]
        {
            matches!(self, Error::UnexpectedStatus(400 | 401))
        }
        #[cfg(not(feature = "bitreq"))]
        {
            false
        }
    }
}
