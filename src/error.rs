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
