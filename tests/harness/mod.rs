//! End-to-end test harness: a target HTTP server, an OHTTP gateway serving
//! its key config, an OHTTP relay ([`ohttp_relay`]) in front of it, and a
//! generic HTTP `CONNECT` proxy standing in for a relay's key-fetch tunnel.
//!
//! The target and gateway are minimal hand-rolled HTTP/1.1 servers on
//! std-thread [`TcpListener`]s; the relay runs on a background tokio runtime.
//! The `CONNECT` proxy is a second hand-rolled server: `ohttp-relay`'s own
//! CONNECT bootstrap gates on an HTTPS-only gateway opt-in probe that our
//! plain-HTTP test gateway can't satisfy, but `CONNECT` tunneling itself is a
//! generic HTTP mechanism, so a minimal standalone proxy exercises the same
//! client-side behavior. Everything shuts down gracefully when the
//! [`TestHarness`] is dropped.

use std::io::{BufRead, BufReader, Cursor, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

/// Gateway opt-in advertised to the relay's prober: ALPN-style list
/// containing the one purpose `ohttp_relay` requires before it forwards.
const ALLOWED_PURPOSES_BODY: &[u8] = b"\x00\x01\x2aTEST 454403bb-9f7b-4385-b31f-acd2dae20b7e";
const GATEWAY_PATH: &str = "/.well-known/ohttp-gateway";

pub struct TestHarness {
    relay_url: String,
    gateway_url: String,
    target_url: String,
    connect_proxy_url: String,
    shutdown: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    server_addrs: Vec<SocketAddr>,
    relay_rt: Option<tokio::runtime::Runtime>,
}

impl TestHarness {
    pub fn start() -> Self {
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut threads = Vec::new();
        let mut server_addrs = Vec::new();

        // Target: the resource the inner request is addressed to. It echoes
        // the request line and body so the test can assert what came through.
        let target_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let target_addr = target_listener.local_addr().unwrap();
        server_addrs.push(target_addr);
        threads.push(serve(target_listener, shutdown.clone(), |req| {
            let mut body = format!("{} {} ", req.method, req.path).into_bytes();
            body.extend_from_slice(&req.body);
            (
                200,
                vec![("content-type".into(), "text/plain".into())],
                body,
            )
        }));

        // Gateway: serves its key config, opts in to relaying, and
        // decapsulates requests, forwarding them to the inner target.
        let key_config = ohttp_client::KeyConfig::new(
            1,
            ohttp::hpke::Kem::X25519Sha256,
            vec![ohttp::SymmetricSuite::new(
                ohttp::hpke::Kdf::HkdfSha256,
                ohttp::hpke::Aead::ChaCha20Poly1305,
            )],
        )
        .unwrap();
        let ohttp_server = Arc::new(ohttp::Server::new(key_config).unwrap());
        let gateway_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let gateway_addr = gateway_listener.local_addr().unwrap();
        server_addrs.push(gateway_addr);
        threads.push(serve(gateway_listener, shutdown.clone(), move |req| {
            handle_gateway_request(&ohttp_server, req)
        }));

        // Relay: the real ohttp-relay crate, forwarding to the gateway.
        // `free_port` + bind is racy under parallel tests; retry on EADDRINUSE.
        let gateway_uri: ohttp_relay::GatewayUri =
            format!("http://127.0.0.1:{}", gateway_addr.port())
                .parse()
                .unwrap();
        let relay_rt = tokio::runtime::Runtime::new().unwrap();
        let relay_port = {
            let mut last_err = None;
            let mut bound = None;
            for _ in 0..16 {
                let port = free_port();
                match relay_rt.block_on(ohttp_relay::listen_tcp(port, gateway_uri.clone())) {
                    Ok(_handle) => {
                        bound = Some(port);
                        break;
                    }
                    Err(err) => last_err = Some(err),
                }
            }
            bound.unwrap_or_else(|| {
                panic!("failed to bind ohttp-relay after retries: {:?}", last_err)
            })
        };

        // Generic CONNECT proxy: stands in for a relay's key-fetch tunnel.
        let connect_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let connect_addr = connect_listener.local_addr().unwrap();
        server_addrs.push(connect_addr);
        threads.push(serve_connect_proxy(connect_listener, shutdown.clone()));

        TestHarness {
            relay_url: format!("http://127.0.0.1:{relay_port}/"),
            gateway_url: format!("http://127.0.0.1:{}{}", gateway_addr.port(), GATEWAY_PATH),
            target_url: format!("http://127.0.0.1:{}", target_addr.port()),
            connect_proxy_url: format!("http://127.0.0.1:{}/", connect_addr.port()),
            shutdown,
            threads,
            server_addrs,
            relay_rt: Some(relay_rt),
        }
    }

    /// URL the encapsulated request is POSTed to.
    pub fn relay_url(&self) -> &str {
        &self.relay_url
    }

    /// RFC 9540 gateway endpoint; GET it to fetch the key config.
    pub fn gateway_url(&self) -> &str {
        &self.gateway_url
    }

    /// Base URL of the target resource (distinct from the gateway).
    pub fn target_url(&self) -> &str {
        &self.target_url
    }

    /// URL of a generic HTTP `CONNECT` proxy, standing in for a relay's
    /// key-fetch tunnel.
    pub fn connect_proxy_url(&self) -> &str {
        &self.connect_proxy_url
    }
}

impl Drop for TestHarness {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        // Unblock each listener's accept() so the server threads can exit.
        for addr in &self.server_addrs {
            let _ = TcpStream::connect(addr);
        }
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
        if let Some(rt) = self.relay_rt.take() {
            rt.shutdown_background();
        }
    }
}

fn handle_gateway_request(
    server: &ohttp::Server,
    req: HttpRequest,
) -> (u16, Vec<(String, String)>, Vec<u8>) {
    match (req.method.as_str(), req.path.split('?').next().unwrap()) {
        ("GET", GATEWAY_PATH) if req.path.contains("allowed_purposes") => (
            200,
            vec![(
                "content-type".into(),
                "application/x-ohttp-allowed-purposes".into(),
            )],
            ALLOWED_PURPOSES_BODY.to_vec(),
        ),
        ("GET", GATEWAY_PATH) => (
            200,
            vec![("content-type".into(), "application/ohttp-keys".into())],
            ohttp_client::KeyConfig::encode_list(&[server.config()]).unwrap(),
        ),
        ("POST", GATEWAY_PATH) => {
            assert_eq!(req.header("content-type"), Some("message/ohttp-req"));
            let (bhttp_bytes, response_ctx) = server.decapsulate(&req.body).unwrap();
            let inner = bhttp::Message::read_bhttp(&mut Cursor::new(&bhttp_bytes[..])).unwrap();
            let inner_response = forward_to_target(&inner);

            let mut bhttp_res = Vec::new();
            inner_response
                .write_bhttp(bhttp::Mode::KnownLength, &mut bhttp_res)
                .unwrap();
            (
                200,
                vec![("content-type".into(), "message/ohttp-res".into())],
                response_ctx.encapsulate(&bhttp_res).unwrap(),
            )
        }
        _ => (404, vec![], b"Not Found".to_vec()),
    }
}

/// Send the decapsulated inner request to the authority it names and build
/// the inner BHTTP response from what comes back.
fn forward_to_target(inner: &bhttp::Message) -> bhttp::Message {
    let part = |v: Option<&[u8]>| String::from_utf8(v.unwrap().to_vec()).unwrap();
    let method = part(inner.control().method());
    let url = format!(
        "{}://{}{}",
        part(inner.control().scheme()),
        part(inner.control().authority()),
        part(inner.control().path()),
    );

    let mut forward = bitreq::Request::new(bitreq::Method::Custom(method), url);
    for field in inner.header().iter() {
        forward = forward.with_header(
            String::from_utf8_lossy(field.name()).into_owned(),
            String::from_utf8_lossy(field.value()).into_owned(),
        );
    }
    let res = forward.with_body(inner.content().to_vec()).send().unwrap();

    let status = bhttp::StatusCode::try_from(u16::try_from(res.status_code).unwrap()).unwrap();
    let mut message = bhttp::Message::response(status);
    for (name, value) in &res.headers {
        message.put_header(name.as_str(), value.as_str());
    }
    message.write_content(res.as_bytes());
    message
}

struct HttpRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Minimal HTTP/1.1 server loop: one connection at a time, one request per
/// connection.
fn serve(
    listener: TcpListener,
    shutdown: Arc<AtomicBool>,
    handler: impl Fn(HttpRequest) -> (u16, Vec<(String, String)>, Vec<u8>) + Send + 'static,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            if shutdown.load(Ordering::SeqCst) {
                break;
            }
            let Ok(mut stream) = stream else { continue };
            if let Some(req) = read_request(&mut stream) {
                let (status, headers, body) = handler(req);
                write_response(&mut stream, status, &headers, &body);
            }
        }
    })
}

fn read_request(stream: &mut TcpStream) -> Option<HttpRequest> {
    let mut reader = BufReader::new(stream);

    let mut request_line = String::new();
    reader.read_line(&mut request_line).ok()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();

    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).ok()?;
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(':')?;
        headers.push((name.trim().to_string(), value.trim().to_string()));
    }

    let req = HttpRequest {
        method,
        path,
        headers,
        body: Vec::new(),
    };
    let content_length: usize = req
        .header("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).ok()?;

    Some(HttpRequest { body, ..req })
}

fn write_response(stream: &mut TcpStream, status: u16, headers: &[(String, String)], body: &[u8]) {
    // `connection: close` keeps the relay's pooled hyper client from reusing
    // a connection this one-request-per-connection server has already closed.
    let mut response = format!(
        "HTTP/1.1 {status} OK\r\ncontent-length: {}\r\nconnection: close\r\n",
        body.len()
    );
    for (name, value) in headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str("\r\n");
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

/// A minimal HTTP `CONNECT` proxy: read the request line and headers, dial
/// the requested `host:port`, reply `200`, then pipe bytes bidirectionally.
fn serve_connect_proxy(listener: TcpListener, shutdown: Arc<AtomicBool>) -> JoinHandle<()> {
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            if shutdown.load(Ordering::SeqCst) {
                break;
            }
            let Ok(client) = stream else { continue };
            std::thread::spawn(move || {
                let _ = handle_connect(client);
            });
        }
    })
}

fn handle_connect(client: TcpStream) -> Option<()> {
    let mut reader = BufReader::new(client.try_clone().ok()?);

    let mut request_line = String::new();
    reader.read_line(&mut request_line).ok()?;
    let mut parts = request_line.split_whitespace();
    if parts.next()? != "CONNECT" {
        return None;
    }
    let target = parts.next()?.to_string();

    loop {
        let mut line = String::new();
        reader.read_line(&mut line).ok()?;
        if line.trim_end().is_empty() {
            break;
        }
    }

    let mut upstream = TcpStream::connect(&target).ok()?;
    let mut client = reader.into_inner();
    client
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .ok()?;

    let mut upstream_read = upstream.try_clone().ok()?;
    let mut client_read = client.try_clone().ok()?;
    let relay_up = std::thread::spawn(move || {
        let _ = std::io::copy(&mut client_read, &mut upstream);
    });
    let _ = std::io::copy(&mut upstream_read, &mut client);
    let _ = relay_up.join();
    Some(())
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}
