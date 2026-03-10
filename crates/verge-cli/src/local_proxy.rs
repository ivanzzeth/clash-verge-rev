//! Local SOCKS5/HTTP proxy that forwards traffic via an upstream (e.g. Shadowsocks).
//! Replaces GOST for expose: one process per node, listens on SOCKS5 + HTTP CONNECT.

use anyhow::Result;
use shadowsocks::{
    config::ServerConfig,
    context::SharedContext,
    relay::tcprelay::proxy_stream::client::ProxyClientStream,
};
use socks5_impl::protocol::Address as Socks5Address;
use socks5_impl::server::auth::NoAuth;
use socks5_impl::server::{ClientConnection, IncomingConnection, Server};
use socks5_impl::protocol::Reply;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, copy_bidirectional};
use tokio::net::TcpListener;

/// Upstream proxy configuration. Only Shadowsocks is implemented for now.
pub enum Upstream {
    Shadowsocks {
        config: ServerConfig,
        context: SharedContext,
    },
}

impl Upstream {
    /// Parse upstream from URI (e.g. ss://...). Returns error if unsupported or invalid.
    pub fn from_uri(uri: &str) -> Result<Self> {
        let uri_lower = uri.trim().to_lowercase();
        if uri_lower.starts_with("ss://") {
            let config = ServerConfig::from_url(uri)
                .map_err(|e| anyhow::anyhow!("invalid shadowsocks URL: {}", e))?;
            let ctx = shadowsocks::context::Context::new_shared(
                shadowsocks::config::ServerType::Local,
            );
            return Ok(Upstream::Shadowsocks {
                config,
                context: ctx,
            });
        }
        anyhow::bail!(
            "unsupported upstream scheme (only ss:// supported for now): {}",
            uri.split(':').next().unwrap_or("")
        )
    }
}

/// Connect to target (host, port) via the upstream. Returns a bidirectional stream.
async fn connect_via_upstream(
    upstream: &Upstream,
    target_host: &str,
    target_port: u16,
) -> Result<impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> {
    match upstream {
        Upstream::Shadowsocks { config, context } => {
            let addr = (target_host.to_string(), target_port);
            let stream = ProxyClientStream::connect(context.clone(), config, addr)
                .await
                .map_err(|e| anyhow::anyhow!("shadowsocks connect: {}", e))?;
            Ok(stream)
        }
    }
}

/// Handle one SOCKS5 connection: authenticate, get CONNECT target, connect via upstream, relay.
async fn handle_socks5_connection(
    upstream: Arc<Upstream>,
    incoming: IncomingConnection<()>,
) -> Result<()> {
    let (authenticated, _) = incoming
        .authenticate()
        .await
        .map_err(|e| anyhow::anyhow!("socks5 auth: {}", e))?;
    let connection = authenticated
        .wait_request()
        .await
        .map_err(|e| anyhow::anyhow!("socks5 request: {}", e))?;

    if let ClientConnection::Connect(connect, addr) = connection {
        let (host, port) = match &addr {
            Socks5Address::SocketAddress(sa) => (sa.ip().to_string(), sa.port()),
            Socks5Address::DomainAddress(domain, p) => (domain.clone(), *p),
        };
        let mut remote_stream = connect_via_upstream(upstream.as_ref(), &host, port).await?;
        let connect_ready = connect
            .reply(Reply::Succeeded, addr)
            .await
            .map_err(|e| anyhow::anyhow!("socks5 reply: {}", e))?;
        let mut client_stream = connect_ready;
        copy_bidirectional(&mut client_stream, &mut remote_stream)
            .await
            .map_err(|e| anyhow::anyhow!("relay: {}", e))?;
        return Ok(());
    }
    anyhow::bail!("only SOCKS5 CONNECT is supported")
}

/// Parse HTTP CONNECT request: first line "CONNECT host:port HTTP/1.x". Returns (host, port).
fn parse_http_connect_request(line: &str) -> Option<(String, u16)> {
    let line = line.trim();
    if !line.to_uppercase().starts_with("CONNECT ") {
        return None;
    }
    let rest = line["CONNECT ".len()..].trim();
    let (host_port, _) = rest.split_once(' ')?;
    let (host, port_str) = host_port.rsplit_once(':')?;
    let port = port_str.parse::<u16>().ok()?;
    Some((host.to_string(), port))
}

/// Handle one HTTP CONNECT connection: read request, connect via upstream, send 200, relay.
async fn handle_http_connect(
    upstream: Arc<Upstream>,
    client: tokio::net::TcpStream,
) -> Result<()> {
    let mut reader = BufReader::new(client);
    let mut first_line = String::new();
    reader.read_line(&mut first_line).await?;
    let (host, port) = match parse_http_connect_request(&first_line) {
        Some(t) => t,
        None => {
            let mut stream = reader.into_inner();
            let _ = stream
                .write_all(b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\n")
                .await;
            anyhow::bail!("invalid CONNECT request");
        }
    };
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        if line == "\r\n" || line == "\n" {
            break;
        }
    }
    let remote_stream = connect_via_upstream(upstream.as_ref(), &host, port).await?;
    let mut client = reader.into_inner();
    client
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await?;
    let (mut client_read, mut client_write) = client.into_split();
    let (mut remote_read, mut remote_write) = tokio::io::split(remote_stream);
    let _ = tokio::try_join!(
        async { tokio::io::copy(&mut client_read, &mut remote_write).await.map(|_| ()) },
        async { tokio::io::copy(&mut remote_read, &mut client_write).await.map(|_| ()) },
    );
    Ok(())
}

/// Run local proxy: listen on socks_addr (SOCKS5) and http_addr (HTTP CONNECT), forward via upstream_uri.
pub async fn run_proxy(
    socks_addr: SocketAddr,
    http_addr: SocketAddr,
    upstream_uri: &str,
) -> Result<()> {
    let upstream = Arc::new(Upstream::from_uri(upstream_uri)?);

    let server = Server::bind(socks_addr, Arc::new(NoAuth)).await?;

    let http_listener = TcpListener::bind(http_addr).await?;

    let upstream_socks = upstream.clone();
    let socks_handle = tokio::spawn(async move {
        loop {
            let (incoming, _) = match server.accept().await {
                Ok(x) => x,
                Err(e) => {
                    eprintln!("socks5 accept: {}", e);
                    continue;
                }
            };
            let up = upstream_socks.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_socks5_connection(up, incoming).await {
                    eprintln!("socks5 handler: {}", e);
                }
            });
        }
    });

    let upstream_http = upstream.clone();
    let http_handle = tokio::spawn(async move {
        loop {
            let (client, _) = match http_listener.accept().await {
                Ok(x) => x,
                Err(e) => {
                    eprintln!("http accept: {}", e);
                    continue;
                }
            };
            let up = upstream_http.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_http_connect(up, client).await {
                    eprintln!("http handler: {}", e);
                }
            });
        }
    });

    tokio::select! {
        _ = socks_handle => {}
        _ = http_handle => {}
    }
    Ok(())
}