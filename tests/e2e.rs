//! End to end: the whole server (start()) on an ephemeral TLS port, driven
//! by WebSocket clients — logins (de_auth frame, legacy query string),
//! routing, kick-out, the auth frame timeout and the HTTP fallback.

mod common;

use andruavserver::net::{ws_connect_path, ClientWs, TlsVerify};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;

async fn boot(p_extra: Value) -> (Arc<andruavserver::Server>, u16) {
    let mut v_cfg = json!({
        "server_ip": "127.0.0.1", "server_port": 0, "public_host": "comm.test.local",
        "ssl_key_file": "./ssl_local/ssl_airgap/domain.key", "ssl_cert_file": "./ssl_local/ssl_airgap/domain.crt",
        "local_server_enabled": true, "ignore_auth_server": true, "enable_storage_server": false, "ignoreLog": true
    });
    if let (Value::Object(b), Value::Object(o)) = (&mut v_cfg, p_extra) {
        b.extend(o);
    }
    let v_srv = common::server(v_cfg);
    let v_started = andruavserver::start(&v_srv).await.unwrap();
    (v_srv, v_started.chat.port())
}

async fn open(p_port: u16, p_path: &str) -> ClientWs {
    ws_connect_path("127.0.0.1", &p_port.to_string(), p_path, Some(&TlsVerify::None)).await.unwrap()
}

/// The next text message as JSON, or "CLOSE <code>" / "END".
async fn next(p_ws: &mut ClientWs) -> Value {
    loop {
        match tokio::time::timeout(Duration::from_secs(10), p_ws.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => return serde_json::from_str(t.as_str()).unwrap(),
            Ok(Some(Ok(Message::Binary(b)))) => return json!({ "binary": common::header(&b) }),
            Ok(Some(Ok(Message::Close(f)))) => {
                return json!(format!("CLOSE {}", f.map(|f| u16::from(f.code)).unwrap_or(1005)))
            }
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(_))) | Ok(None) => return json!("END"),
            Err(_) => return json!("TIMEOUT"),
        }
    }
}

async fn send(p_ws: &mut ClientWs, p_msg: Value) {
    p_ws.send(Message::text(p_msg.to_string())).await.unwrap();
}

async fn login_frame(p_port: u16, p_name: &str, p_actor: &str) -> ClientWs {
    let mut v_ws = open(p_port, "").await;
    send(&mut v_ws, json!({ "ty": "s", "mt": "de_auth", "f": "k", "s": p_name, "at": p_actor })).await;
    let v_welcome = next(&mut v_ws).await;
    assert_eq!(v_welcome["mt"], 9007);
    assert!(v_welcome["ms"]["s"].as_str().unwrap().starts_with("OK:connected:tcp:127.0.0.1:"));
    assert_eq!(next(&mut v_ws).await, json!({ "ty": "s", "mt": "de_auth_ack", "r": "ok" }));
    v_ws
}

#[tokio::test]
async fn frame_and_query_logins_route_and_ping() {
    let (_srv, v_port) = boot(json!({})).await;
    let mut v_gcs = open(v_port, "/?f=k1&s=G1&at=g").await;
    assert_eq!(next(&mut v_gcs).await["mt"], 9007);
    let mut v_drone = login_frame(v_port, "D1", "d").await;
    send(&mut v_drone, json!({ "ty": "g", "mt": 1002, "ms": { "lat": 1.0 } })).await;
    assert_eq!(next(&mut v_gcs).await, json!({ "ty": "g", "mt": 1002, "ms": { "lat": 1.0 }, "p": 4294967295u64 }));
    send(&mut v_gcs, json!({ "ty": "i", "tg": "D1", "mt": 1005 })).await;
    assert_eq!(next(&mut v_drone).await["mt"], 1005);
    send(&mut v_drone, json!({ "ty": "s", "mt": 9005, "ms": {} })).await;
    let v_pong = next(&mut v_drone).await;
    assert_eq!(v_pong["ms"]["s"], "OK:pong");
    assert!(v_pong["ms"]["ts"].is_number());
}

#[tokio::test]
async fn a_newer_login_with_the_same_sender_kicks_the_old_socket_out() {
    let (_srv, v_port) = boot(json!({})).await;
    let mut v_gcs = open(v_port, "/?f=k&s=G&at=g").await;
    next(&mut v_gcs).await;
    let mut v_old = login_frame(v_port, "D", "d").await;
    let mut v_new = login_frame(v_port, "D", "d").await;
    assert_eq!(next(&mut v_old).await, json!("END"), "terminated without a close frame");
    send(&mut v_gcs, json!({ "ty": "i", "tg": "D", "mt": 7 })).await;
    assert_eq!(next(&mut v_new).await["mt"], 7);
}

#[tokio::test]
async fn bad_auth_frames_are_answered_and_closed() {
    let (_srv, v_port) = boot(json!({})).await;
    for (v_frame, v_reason) in [
        ("not json".to_string(), "bad auth frame format"),
        (json!({ "ty": "g", "mt": 1 }).to_string(), "expected de_auth frame"),
        (json!({ "ty": "s", "mt": "de_auth", "s": "X" }).to_string(), "validation error"),
    ] {
        let mut v_ws = open(v_port, "").await;
        v_ws.send(Message::text(v_frame)).await.unwrap();
        assert_eq!(next(&mut v_ws).await, json!({ "ty": "s", "mt": "de_auth_ack", "r": "fail", "em": v_reason }));
        assert_eq!(next(&mut v_ws).await, json!("CLOSE 1005"));
    }
}

#[tokio::test]
async fn legacy_credentials_can_be_turned_off() {
    let (_srv, v_port) = boot(json!({ "ws_auth_no_frame_old_compatibility": false })).await;
    let mut v_ws = open(v_port, "/?f=k&s=L&at=d").await;
    assert_eq!(next(&mut v_ws).await, json!("CLOSE 1005"));
}

#[tokio::test]
async fn a_socket_without_an_auth_frame_is_closed_after_8_s() {
    let (_srv, v_port) = boot(json!({})).await;
    let mut v_ws = open(v_port, "").await;
    assert_eq!(
        next(&mut v_ws).await,
        json!({ "ty": "s", "mt": "de_auth_ack", "r": "fail", "em": "auth frame timeout" })
    );
    assert_eq!(next(&mut v_ws).await, json!("CLOSE 1005"));
}

#[tokio::test]
async fn plain_http_gets_expresss_404() {
    let (_srv, v_port) = boot(json!({})).await;
    let v_tcp = tokio::net::TcpStream::connect(("127.0.0.1", v_port)).await.unwrap();
    let v_cfg = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoVerify(Arc::new(rustls::crypto::ring::default_provider()))))
        .with_no_client_auth();
    let mut v_tls = tokio_rustls::TlsConnector::from(Arc::new(v_cfg))
        .connect(rustls_pki_types::ServerName::try_from("localhost").unwrap(), v_tcp)
        .await
        .unwrap();
    v_tls.write_all(b"GET /a<b HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").await.unwrap();
    let mut v_resp = String::new();
    let _ = v_tls.read_to_string(&mut v_resp).await;
    assert!(v_resp.starts_with("HTTP/1.1 404 Not Found\r\n"), "{}", v_resp);
    assert!(v_resp.contains("Content-Security-Policy: default-src 'none'"));
    assert!(v_resp.ends_with("<pre>Cannot GET /a%3Cb</pre>\n</body>\n</html>\n"), "{}", v_resp);
}

#[derive(Debug)]
struct NoVerify(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _: &rustls_pki_types::CertificateDer<'_>,
        _: &[rustls_pki_types::CertificateDer<'_>],
        _: &rustls_pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls_pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        m: &[u8],
        c: &rustls_pki_types::CertificateDer<'_>,
        d: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(m, c, d, &self.0.signature_verification_algorithms)
    }
    fn verify_tls13_signature(
        &self,
        m: &[u8],
        c: &rustls_pki_types::CertificateDer<'_>,
        d: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(m, c, d, &self.0.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}
