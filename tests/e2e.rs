//! Full end-to-end test: key config fetched over HTTP from a gateway, the
//! encapsulated request POSTed (via `bitreq`) to a real `ohttp-relay`, which
//! forwards it to the gateway, which forwards the inner request to a target
//! on a different origin than the gateway.

use ohttp_client::harness::TestHarness;
use ohttp_client::{parse_key_config, OhttpClient, Url};

#[test]
fn e2e_through_relay_and_gateway() {
    let harness = TestHarness::start();

    // Bootstrap: fetch the key config from the gateway ourselves (sans-IO).
    let keys_res = bitreq::get(harness.gateway_url()).send().unwrap();
    assert_eq!(keys_res.status_code, 200);
    assert_eq!(
        keys_res.headers.get("content-type").map(String::as_str),
        Some("application/ohttp-keys")
    );

    let client = OhttpClient::new(
        Url::parse(harness.relay_url()).unwrap(),
        Url::parse(harness.target_url()).unwrap(),
        parse_key_config(keys_res.as_bytes()).unwrap(),
    );

    // Encapsulate, send the outer request to the relay ourselves, decapsulate.
    let (req, ctx) = client
        .encapsulate(
            "POST",
            "/echo",
            &[("content-type", "text/plain")],
            &[("x", "1")],
            Some(b"hello"),
        )
        .unwrap();
    let outer_res = bitreq::post(req.url.as_str())
        .with_header("content-type", req.content_type)
        .with_body(req.body.clone())
        .send()
        .unwrap();
    assert_eq!(
        outer_res.status_code,
        200,
        "relay says: {:?}",
        outer_res.as_str()
    );
    assert_eq!(
        outer_res.headers.get("content-type").map(String::as_str),
        Some("message/ohttp-res")
    );

    let response = ctx.decapsulate(outer_res.as_bytes()).unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.header("content-type"), Some(&b"text/plain"[..]));
    // The target echoes "<method> <path> <body>", proving the inner request
    // traversed relay -> gateway -> target intact.
    assert_eq!(response.body(), b"POST /echo?x=1 hello");
}

/// With a fixed BHTTP plaintext
/// length so trailing random padding is present. Proves the gateway's
/// `read_bhttp` ignores the pad and the rest of the stack still works.
#[test]
fn e2e_with_known_length_padding() {
    let harness = TestHarness::start();

    let keys_res = bitreq::get(harness.gateway_url()).send().unwrap();
    assert_eq!(keys_res.status_code, 200);
    let key_config = parse_key_config(keys_res.as_bytes()).unwrap();

    let known_length = 1024;
    let client = OhttpClient::new(
        Url::parse(harness.relay_url()).unwrap(),
        Url::parse(harness.target_url()).unwrap(),
        key_config.clone(),
    )
    .known_length(known_length);

    let (req, ctx) = client
        .encapsulate(
            "POST",
            "/echo",
            &[("content-type", "text/plain")],
            &[("x", "1")],
            Some(b"hello"),
        )
        .unwrap();

    // Ciphertext should be larger than an unpadded encapsulation of the same
    // request (same AEAD overhead, longer plaintext).
    let (unpadded, _) = OhttpClient::new(
        Url::parse(harness.relay_url()).unwrap(),
        Url::parse(harness.target_url()).unwrap(),
        key_config,
    )
    .encapsulate(
        "POST",
        "/echo",
        &[("content-type", "text/plain")],
        &[("x", "1")],
        Some(b"hello"),
    )
    .unwrap();
    assert!(
        req.body.len() > unpadded.body.len(),
        "padded {} vs unpadded {}",
        req.body.len(),
        unpadded.body.len()
    );

    let outer_res = bitreq::post(req.url.as_str())
        .with_header("content-type", req.content_type)
        .with_body(req.body)
        .send()
        .unwrap();
    assert_eq!(
        outer_res.status_code,
        200,
        "relay says: {:?}",
        outer_res.as_str()
    );

    let response = ctx.decapsulate(outer_res.as_bytes()).unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.header("content-type"), Some(&b"text/plain"[..]));
    assert_eq!(response.body(), b"POST /echo?x=1 hello");
}

/// Same round trip, but the client sends the outer request itself via the
/// `bitreq` feature's async request builder. Key fetching still goes through
/// the crate's sans-IO `parse_key_config` fed by our own bitreq GET here.
#[cfg(feature = "bitreq")]
#[test]
fn e2e_send_with_bitreq_feature() {
    let harness = TestHarness::start();

    let keys_res = bitreq::get(harness.gateway_url()).send().unwrap();
    assert_eq!(keys_res.status_code, 200);

    let client = OhttpClient::new(
        Url::parse(harness.relay_url()).unwrap(),
        Url::parse(harness.target_url()).unwrap(),
        parse_key_config(keys_res.as_bytes()).unwrap(),
    );

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let response = runtime
        .block_on(
            client
                .post("/echo")
                .param("x", "1")
                .header("content-type", "text/plain")
                .body("hello")
                .send(),
        )
        .unwrap();

    assert_eq!(response.status(), 200);
    assert_eq!(response.body(), b"POST /echo?x=1 hello");
}

/// The fully `bitreq`-powered flow: the crate itself fetches the gateway key
/// config *and* sends the outer request, both over `bitreq`.
///
/// Key fetch is direct (not via `CONNECT`) so we can bind the real OHTTP
/// relay for encapsulated requests. For the privacy-preserving bootstrap,
/// see `e2e_from_gateway`.
#[cfg(feature = "bitreq")]
#[test]
fn e2e_fetch_key_config_and_send_with_bitreq() {
    let harness = TestHarness::start();
    let runtime = tokio::runtime::Runtime::new().unwrap();

    let key_config = runtime
        .block_on(ohttp_client::fetch_key_config(harness.gateway_url()))
        .unwrap();
    let client = OhttpClient::new(
        Url::parse(harness.relay_url()).unwrap(),
        Url::parse(harness.target_url()).unwrap(),
        key_config,
    );

    let response = runtime
        .block_on(
            client
                .post("/echo")
                .param("x", "1")
                .header("content-type", "text/plain")
                .body("hello")
                .send(),
        )
        .unwrap();

    assert_eq!(response.status(), 200);
    assert_eq!(response.header("content-type"), Some(&b"text/plain"[..]));
    assert_eq!(response.body(), b"POST /echo?x=1 hello");
}

/// [`OhttpClient::from_gateway`] tunnels the key fetch through an HTTP
/// `CONNECT` proxy instead of dialing the gateway directly, so the gateway
/// never learns the client's IP even for that bootstrap request.
///
/// The harness's generic `CONNECT` proxy stands in for a relay here: real
/// relays such as `ohttp-relay` implement the same `CONNECT` tunneling (its
/// `connect-bootstrap` feature), typically gated behind a gateway opt-in
/// check that assumes an HTTPS gateway origin, which our plain-HTTP test
/// gateway can't satisfy. That gating is a relay-operator policy concern
/// orthogonal to what's being tested here: that `from_gateway` correctly
/// tunnels the key GET and parses what comes back.
#[cfg(feature = "bitreq")]
#[test]
fn e2e_from_gateway() {
    let harness = TestHarness::start();
    let runtime = tokio::runtime::Runtime::new().unwrap();

    let client = runtime
        .block_on(OhttpClient::from_gateway(
            Url::parse(harness.connect_proxy_url()).unwrap(),
            Url::parse(harness.target_url()).unwrap(),
            harness.gateway_url(),
        ))
        .unwrap();

    // Prove the tunneled fetch produced a working key config by using it for
    // a normal encapsulate/decapsulate round trip against the gateway.
    let (req, ctx) = client
        .encapsulate(
            "POST",
            "/echo",
            &[("content-type", "text/plain")],
            &[("x", "1")],
            Some(b"hello"),
        )
        .unwrap();
    let gateway_res = bitreq::post(harness.gateway_url())
        .with_header("content-type", req.content_type)
        .with_body(req.body)
        .send()
        .unwrap();
    assert_eq!(gateway_res.status_code, 200);

    let response = ctx.decapsulate(gateway_res.as_bytes()).unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.header("content-type"), Some(&b"text/plain"[..]));
    assert_eq!(response.body(), b"POST /echo?x=1 hello");
}

/// Same relay -> gateway -> target path, exercised through the `wasm-bindgen`
/// API surface (`WasmOhttpClient` / `Encapsulated`). The host still does HTTP
/// with `bitreq`; in a browser that would be `fetch`.
#[cfg(feature = "wasm")]
#[test]
fn e2e_wasm_bindgen_api() {
    use ohttp_client::WasmOhttpClient;

    let harness = TestHarness::start();

    let keys_res = bitreq::get(harness.gateway_url()).send().unwrap();
    assert_eq!(keys_res.status_code, 200);

    let client = WasmOhttpClient::new(
        harness.relay_url(),
        harness.target_url(),
        keys_res.as_bytes(),
    )
    .unwrap();

    let encapsulated = client
        .encapsulate("POST", "/echo")
        .header("content-type", "text/plain")
        .param("x", "1")
        .body(b"hello".to_vec())
        .build()
        .unwrap();

    let outer_res = bitreq::post(encapsulated.url())
        .with_header("content-type", encapsulated.content_type())
        .with_body(encapsulated.body())
        .send()
        .unwrap();
    assert_eq!(
        outer_res.status_code,
        200,
        "relay says: {:?}",
        outer_res.as_str()
    );

    let response = encapsulated.decapsulate(outer_res.as_bytes()).unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.header("content-type"),
        Some(b"text/plain".to_vec())
    );
    assert_eq!(response.body(), b"POST /echo?x=1 hello");
}

/// A gateway that rotates with no overlap breaks every client still holding
/// the old key. With a refresh route configured, `send` absorbs that: the 400
/// triggers a refetch and one retry, and the caller sees only success.
#[cfg(feature = "bitreq")]
#[test]
fn e2e_rotation_is_transparent_to_send() {
    use ohttp_client::KeyFetch;

    let harness = TestHarness::start();
    let runtime = tokio::runtime::Runtime::new().unwrap();

    // `Direct` because the harness relay is the real `ohttp-relay`, which does
    // not offer CONNECT bootstrap here; `e2e_refresh_key_config_via_relay`
    // covers the tunneled route.
    let key_config = runtime
        .block_on(ohttp_client::fetch_key_config(harness.gateway_url()))
        .unwrap();
    let client = OhttpClient::new(
        Url::parse(harness.relay_url()).unwrap(),
        Url::parse(harness.target_url()).unwrap(),
        key_config.clone(),
    )
    .with_key_refresh(harness.gateway_url(), KeyFetch::Direct);

    let response = runtime
        .block_on(client.post("/echo").body("hello").send())
        .unwrap();
    assert_eq!(response.body(), b"POST /echo hello");

    harness.rotate_keys();

    let response = runtime
        .block_on(client.post("/echo").body("after").send())
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.body(), b"POST /echo after");

    // The retry succeeded because the client adopted the new key, not because
    // the gateway kept honoring the old one.
    assert_ne!(
        client.key_config().encode().unwrap(),
        key_config.encode().unwrap()
    );
}

/// A gateway rotating with an overlap window keeps decapsulating with the
/// retired key, so an in-flight client is never disrupted and never refetches.
#[cfg(feature = "bitreq")]
#[test]
fn e2e_rotation_with_overlap_is_not_disruptive() {
    use ohttp_client::KeyFetch;

    let harness = TestHarness::start();
    let runtime = tokio::runtime::Runtime::new().unwrap();

    let key_config = runtime
        .block_on(ohttp_client::fetch_key_config(harness.gateway_url()))
        .unwrap();
    let client = OhttpClient::new(
        Url::parse(harness.relay_url()).unwrap(),
        Url::parse(harness.target_url()).unwrap(),
        key_config.clone(),
    )
    .with_key_refresh(harness.gateway_url(), KeyFetch::Direct);

    harness.rotate_keys_overlap();

    let response = runtime
        .block_on(client.post("/echo").body("hello").send())
        .unwrap();
    assert_eq!(response.body(), b"POST /echo hello");

    // No 400, so no refetch: the client is still on the key it started with
    // even though the endpoint now advertises a newer one.
    assert_eq!(
        client.key_config().encode().unwrap(),
        key_config.encode().unwrap()
    );
}

/// Without a refresh route there is nothing to recover with, so the rotation
/// surfaces as an error the caller can recognize rather than a hang or panic.
#[cfg(feature = "bitreq")]
#[test]
fn e2e_rotation_without_refresh_route_surfaces_error() {
    let harness = TestHarness::start();
    let runtime = tokio::runtime::Runtime::new().unwrap();

    let key_config = runtime
        .block_on(ohttp_client::fetch_key_config(harness.gateway_url()))
        .unwrap();
    let client = OhttpClient::new(
        Url::parse(harness.relay_url()).unwrap(),
        Url::parse(harness.target_url()).unwrap(),
        key_config,
    );

    harness.rotate_keys();

    let err = runtime
        .block_on(client.post("/echo").body("hello").send())
        .unwrap_err();
    assert!(
        matches!(err, ohttp_client::Error::UnexpectedStatus(400)),
        "expected the gateway's 400 to reach the caller, got {err:?}"
    );
    assert!(err.is_possibly_stale_key());

    // Refreshing is exactly what this client cannot do.
    assert!(matches!(
        runtime.block_on(client.refresh_key_config()),
        Err(ohttp_client::Error::NoGatewayKeyUrl)
    ));
}

/// The tunneled refresh route: the key refetch goes through HTTP `CONNECT` so
/// the gateway never learns the client's IP, same as the bootstrap fetch.
#[cfg(feature = "bitreq")]
#[test]
fn e2e_refresh_key_config_via_relay() {
    let harness = TestHarness::start();
    let runtime = tokio::runtime::Runtime::new().unwrap();

    let client = runtime
        .block_on(OhttpClient::from_gateway(
            Url::parse(harness.connect_proxy_url()).unwrap(),
            Url::parse(harness.target_url()).unwrap(),
            harness.gateway_url(),
        ))
        .unwrap();
    let original = client.key_config().encode().unwrap();

    // Nothing rotated yet, so the refetch is a no-op — this is the signal
    // `send` uses to decide a 400 was not about the key.
    assert!(!runtime.block_on(client.refresh_key_config()).unwrap());
    assert_eq!(client.key_config().encode().unwrap(), original);

    harness.rotate_keys();
    assert!(runtime.block_on(client.refresh_key_config()).unwrap());
    assert_ne!(client.key_config().encode().unwrap(), original);
}
