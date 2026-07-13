//! Full end-to-end test: key config fetched over HTTP from a gateway, the
//! encapsulated request POSTed (via `bitreq`) to a real `ohttp-relay`, which
//! forwards it to the gateway, which forwards the inner request to a target
//! on a different origin than the gateway.

use ohttp_client::{OhttpClient, Url};

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

    let target_url = format!("{}/echo?x=1", harness.target_url());
    let client = OhttpClient::builder()
        .relay(Url::parse(harness.relay_url()).unwrap())
        .target(Url::parse(&target_url).unwrap())
        .encoded_key_config(keys_res.as_bytes())
        .unwrap()
        .build()
        .unwrap();

    // Encapsulate, send the outer request to the relay ourselves, decapsulate.
    let (req, ctx) = client
        .encapsulate("POST", &[("content-type", "text/plain")], Some(b"hello"))
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
/// `bitreq` feature's async `send`.
#[cfg(feature = "bitreq")]
#[test]
fn e2e_send_with_bitreq_feature() {
    let harness = harness::TestHarness::start();

    let keys_res = bitreq::get(harness.gateway_url()).send().unwrap();
    assert_eq!(keys_res.status_code, 200);

    let client = OhttpClient::builder()
        .relay(Url::parse(harness.relay_url()).unwrap())
        .target(Url::parse(&format!("{}/echo?x=1", harness.target_url())).unwrap())
        .encoded_key_config(keys_res.as_bytes())
        .unwrap()
        .build()
        .unwrap();

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let response = runtime
        .block_on(client.send("POST", &[("content-type", "text/plain")], Some(b"hello")))
        .unwrap();

    assert_eq!(response.status(), 200);
    assert_eq!(response.body(), b"POST /echo?x=1 hello");
}
