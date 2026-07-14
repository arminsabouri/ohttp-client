//! Full end-to-end test: key config fetched over HTTP from a gateway, the
//! encapsulated request POSTed (via `bitreq`) to a real `ohttp-relay`, which
//! forwards it to the gateway, which forwards the inner request to a target
//! on a different origin than the gateway.

use ohttp_client::{OhttpClient, Url, parse_key_config};

mod harness;

#[test]
fn e2e_through_relay_and_gateway() {
    let harness = harness::TestHarness::start();

    // Bootstrap: fetch the key config from the gateway ourselves (sans-IO).
    let keys_res = bitreq::get(harness.gateway_url()).send().unwrap();
    assert_eq!(keys_res.status_code, 200);
    assert_eq!(
        keys_res.headers.get("content-type").map(String::as_str),
        Some("application/ohttp-keys")
    );

    let target_url = format!("{}/echo", harness.target_url());
    let client = OhttpClient::new(
        Url::parse(harness.relay_url()).unwrap(),
        Url::parse(&target_url).unwrap(),
        parse_key_config(keys_res.as_bytes()).unwrap(),
    );

    // Encapsulate, send the outer request to the relay ourselves, decapsulate.
    let (req, ctx) = client
        .encapsulate(
            "POST",
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

/// Same round trip, but the client sends the outer request itself via the
/// `bitreq` feature's async request builder. Key fetching still goes through
/// the crate's sans-IO `parse_key_config` fed by our own bitreq GET here.
#[cfg(feature = "bitreq")]
#[test]
fn e2e_send_with_bitreq_feature() {
    let harness = harness::TestHarness::start();

    let keys_res = bitreq::get(harness.gateway_url()).send().unwrap();
    assert_eq!(keys_res.status_code, 200);

    let client = OhttpClient::new(
        Url::parse(harness.relay_url()).unwrap(),
        Url::parse(&format!("{}/echo", harness.target_url())).unwrap(),
        parse_key_config(keys_res.as_bytes()).unwrap(),
    );

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let response = runtime
        .block_on(
            client
                .post()
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
    let harness = harness::TestHarness::start();
    let runtime = tokio::runtime::Runtime::new().unwrap();

    let key_config = runtime
        .block_on(ohttp_client::fetch_key_config(harness.gateway_url()))
        .unwrap();
    let client = OhttpClient::new(
        Url::parse(harness.relay_url()).unwrap(),
        Url::parse(&format!("{}/echo", harness.target_url())).unwrap(),
        key_config,
    );

    let response = runtime
        .block_on(
            client
                .post()
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
    let harness = harness::TestHarness::start();
    let runtime = tokio::runtime::Runtime::new().unwrap();

    let client = runtime
        .block_on(OhttpClient::from_gateway(
            Url::parse(harness.connect_proxy_url()).unwrap(),
            Url::parse(&format!("{}/echo", harness.target_url())).unwrap(),
            harness.gateway_url(),
        ))
        .unwrap();

    // Prove the tunneled fetch produced a working key config by using it for
    // a normal encapsulate/decapsulate round trip against the gateway.
    let (req, ctx) = client
        .encapsulate(
            "POST",
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
