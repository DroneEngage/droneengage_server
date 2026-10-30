//! TLS and WebSocket plumbing shared by the listeners and the outgoing S2S
//! links.
//!
//! Outgoing links follow the JavaScript server's choices:
//!   - storage server, UDP relays, parent (super) server: certificates are
//!     not verified (`rejectUnauthorized: false`);
//!   - AUTH: verified against Mozilla's root store (what Node.js ships)
//!     plus NODE_EXTRA_CA_CERTS, unless allow_fake_SSL is true
//!     (NODE_TLS_REJECT_UNAUTHORIZED = 0).

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{ring, CryptoProvider};
use rustls::{DigitallySignedStruct, SignatureScheme};
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio_rustls::{TlsAcceptor, TlsConnector};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

/// ws's default maxPayload (100 MiB).
pub const MAX_PAYLOAD: usize = 100 * 1024 * 1024;

pub fn ws_config() -> WebSocketConfig {
    WebSocketConfig::default().max_message_size(Some(MAX_PAYLOAD)).max_frame_size(Some(MAX_PAYLOAD))
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(ring::default_provider())
}

/// A TLS acceptor for a key/cert pair (PEM).
pub fn tls_acceptor(p_key: &Path, p_cert: &Path) -> Result<TlsAcceptor, String> {
    let v_certs = CertificateDer::pem_file_iter(p_cert)
        .and_then(|it| it.collect::<Result<Vec<_>, _>>())
        .map_err(|e| format!("invalid SSL cert file {}: {}", p_cert.display(), e))?;
    let v_key =
        PrivateKeyDer::from_pem_file(p_key).map_err(|e| format!("invalid SSL key file {}: {}", p_key.display(), e))?;
    let v_server = rustls::ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(v_certs, v_key)
        .map_err(|e| format!("SSL certificate/key rejected: {}", e))?;
    Ok(TlsAcceptor::from(Arc::new(v_server)))
}

/// Accepts any server certificate (rejectUnauthorized: false).
#[derive(Debug)]
struct AcceptAnyCert(Arc<CryptoProvider>);

impl ServerCertVerifier for AcceptAnyCert {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// How an outgoing wss link checks the server certificate.
#[derive(Debug, Clone)]
pub enum TlsVerify {
    /// rejectUnauthorized: false
    None,
    /// Node's default store plus an optional extra CA file
    Roots { extra_ca: Option<String> },
}

fn client_config(p_verify: &TlsVerify) -> Result<rustls::ClientConfig, String> {
    let v_builder = rustls::ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?;
    Ok(match p_verify {
        TlsVerify::None => v_builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAnyCert(provider())))
            .with_no_client_auth(),
        TlsVerify::Roots { extra_ca } => {
            let mut v_roots = rustls::RootCertStore::empty();
            v_roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            if let Some(v_file) = extra_ca {
                match CertificateDer::pem_file_iter(v_file).and_then(|it| it.collect::<Result<Vec<_>, _>>()) {
                    Ok(v_certs) => {
                        for c in v_certs {
                            let _ = v_roots.add(c);
                        }
                    }
                    Err(e) => println!("Warning: Ignoring extra certs from `{}`, load failed: {}", v_file, e),
                }
            }
            v_builder.with_root_certificates(v_roots).with_no_client_auth()
        }
    })
}

pub type ClientStream = Box<dyn AsyncReadWrite>;

pub trait AsyncReadWrite: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> AsyncReadWrite for T {}

pub type ClientWs = WebSocketStream<ClientStream>;

/// The host part of a ws URL ("::1" -> "[::1]").
fn url_host(p_host: &str) -> String {
    if p_host.contains(':') && !p_host.starts_with('[') {
        format!("[{}]", p_host)
    } else {
        p_host.to_string()
    }
}

/// Opens `ws[s]://host:port`.
pub async fn ws_connect(p_host: &str, p_port: &str, p_tls: Option<&TlsVerify>) -> Result<ClientWs, String> {
    ws_connect_path(p_host, p_port, "", p_tls).await
}

/// Opens `ws[s]://host:port<path>` (path with its query string, or "").
pub async fn ws_connect_path(
    p_host: &str,
    p_port: &str,
    p_path: &str,
    p_tls: Option<&TlsVerify>,
) -> Result<ClientWs, String> {
    let v_host = p_host.trim_start_matches('[').trim_end_matches(']').to_string();
    let v_port: u16 = p_port.trim().parse().map_err(|_| format!("Invalid URL: {}:{}", p_host, p_port))?;
    let v_tcp = TcpStream::connect((v_host.as_str(), v_port)).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::ConnectionRefused {
            format!("connect ECONNREFUSED {}:{}", v_host, v_port)
        } else {
            e.to_string()
        }
    })?;
    let _ = v_tcp.set_nodelay(true);
    let (v_stream, v_scheme): (ClientStream, &str) = match p_tls {
        None => (Box::new(v_tcp), "ws"),
        Some(v_verify) => {
            let v_connector = TlsConnector::from(Arc::new(client_config(v_verify)?));
            let v_name = ServerName::try_from(v_host.clone()).map_err(|e| e.to_string())?;
            let v_tls = v_connector.connect(v_name, v_tcp).await.map_err(|e| e.to_string())?;
            (Box::new(v_tls), "wss")
        }
    };
    let v_url = format!("{}://{}:{}{}", v_scheme, url_host(&v_host), v_port, p_path);
    let (v_ws, _) = tokio_tungstenite::client_async_with_config(v_url, v_stream, Some(ws_config()))
        .await
        .map_err(|e| e.to_string())?;
    Ok(v_ws)
}

/// Splits a WebSocket into a send channel (served by a writer task) and the
/// read half. The writer ends after sending a close frame or on error.
pub fn split_with_writer<S>(p_ws: WebSocketStream<S>) -> (UnboundedSender<Message>, SplitStream<WebSocketStream<S>>)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (v_sink, v_stream) = p_ws.split();
    let (v_tx, mut v_rx) = unbounded_channel::<Message>();
    tokio::spawn(async move {
        let mut v_sink: SplitSink<WebSocketStream<S>, Message> = v_sink;
        while let Some(m) = v_rx.recv().await {
            let v_close = matches!(m, Message::Close(_));
            if v_sink.send(m).await.is_err() || v_close {
                break;
            }
        }
    });
    (v_tx, v_stream)
}

/// The bytes of a data frame (text or binary), or None for control frames.
pub fn frame_bytes(p_msg: &Message) -> Option<&[u8]> {
    match p_msg {
        Message::Text(t) => Some(t.as_bytes()),
        Message::Binary(b) => Some(b),
        _ => None,
    }
}

/// JavaScript template interpolation of a config value for URLs/logs.
pub fn cfg_str(p_value: Option<&serde_json::Value>) -> String {
    crate::util::js_key(p_value)
}

/// True when a read error is just the peer going away (Node's ws emits only
/// 'close' then, no 'error').
pub fn is_abrupt_close(p_err: &tokio_tungstenite::tungstenite::Error) -> bool {
    use tokio_tungstenite::tungstenite::error::ProtocolError;
    use tokio_tungstenite::tungstenite::Error as E;
    matches!(
        p_err,
        E::ConnectionClosed | E::AlreadyClosed | E::Io(_) | E::Protocol(ProtocolError::ResetWithoutClosingHandshake)
    )
}
