//! Streamable HTTP / HTTPS transport with a server-generated bearer token.

use crate::mesh::Mesh;
use crate::server::{Config, MeshServer};
use anyhow::{Context, Result};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

pub struct Options {
    pub addr: SocketAddr,
    pub tls: bool,
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
    pub new_token: bool,
}

/// Constant-time byte comparison, so the bearer check doesn't leak timing.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Identifies the token's origin (like `ghp_`), so it is recognisable in config
/// files and picked up by secret scanners.
const TOKEN_PREFIX: &str = "meshmcp_";

/// The token clients must present. Generated on first start (256 random bits)
/// and kept in a file so it survives restarts; `--new-token` replaces it.
fn load_or_create_token(new: bool) -> Result<(String, String)> {
    let path = std::env::var("MCP_HTTP_TOKEN_FILE").unwrap_or_else(|_| "mcp-http-token.txt".into());
    if !new {
        if let Ok(t) = std::fs::read_to_string(&path) {
            let t = t.trim().to_string();
            if t.starts_with(TOKEN_PREFIX) && t.len() >= TOKEN_PREFIX.len() + 32 {
                return Ok((t, path));
            }
        }
    }
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("no secure random source: {e}"))?;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let token = format!("{TOKEN_PREFIX}{hex}");
    std::fs::write(&path, &token).with_context(|| format!("writing {path}"))?;
    Ok((token, path))
}

/// Use the supplied certificate, or create (once) a self-signed one in `mcp-tls/`.
fn cert_paths(o: &Options) -> Result<(String, String)> {
    if let (Some(c), Some(k)) = (&o.tls_cert, &o.tls_key) {
        return Ok((c.clone(), k.clone()));
    }
    anyhow::ensure!(
        o.tls_cert.is_none() && o.tls_key.is_none(),
        "--tls-cert and --tls-key must be given together"
    );
    let (cert, key) = ("mcp-tls/cert.pem".to_string(), "mcp-tls/key.pem".to_string());
    if !Path::new(&cert).exists() || !Path::new(&key).exists() {
        let mut names = vec!["localhost".to_string(), "127.0.0.1".to_string(), "::1".to_string()];
        if let Ok(extra) = std::env::var("MCP_TLS_NAMES") {
            names.extend(extra.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()));
        }
        let ck = rcgen::generate_simple_self_signed(names).context("generating self-signed certificate")?;
        std::fs::create_dir_all("mcp-tls")?;
        std::fs::write(&cert, ck.cert.pem())?;
        std::fs::write(&key, ck.signing_key.serialize_pem())?;
        eprintln!("meshcentral-mcp: created a self-signed certificate in mcp-tls/ (clients must trust it)");
    }
    Ok((cert, key))
}

pub async fn serve(o: Options, mesh: Arc<Mesh>, cfg: Arc<Config>) -> Result<()> {
    use axum::{extract::Request, http::StatusCode, middleware::Next, response::Response};
    use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
    use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};

    if !o.tls && !o.addr.ip().is_loopback() {
        eprintln!(
            "meshcentral-mcp: warning: plain HTTP on {} sends the bearer token unencrypted; \
             use https, a TLS reverse proxy, or a loopback address",
            o.addr
        );
    }

    let (token, token_path) = load_or_create_token(o.new_token)?;
    let svc = StreamableHttpService::new(
        move || Ok(MeshServer::new(mesh.clone(), cfg.clone())),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );
    let expected = format!("Bearer {token}");
    let app = axum::Router::new().nest_service("/mcp", svc).layer(axum::middleware::from_fn(
        move |req: Request, next: Next| {
            let expected = expected.clone();
            async move {
                let ok = req
                    .headers()
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|v| ct_eq(v.as_bytes(), expected.as_bytes()));
                if ok {
                    Ok(next.run(req).await)
                } else {
                    Err::<Response, StatusCode>(StatusCode::UNAUTHORIZED)
                }
            }
        },
    ));

    let scheme = if o.tls { "https" } else { "http" };
    eprintln!("meshcentral-mcp: serving Streamable HTTP on {scheme}://{}/mcp", o.addr);
    eprintln!("meshcentral-mcp: clients send \"Authorization: Bearer <token>\"; the token is in {token_path}");

    if o.tls {
        let (cert, key) = cert_paths(&o)?;
        let config = axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert, &key)
            .await
            .with_context(|| format!("loading TLS certificate {cert} / key {key}"))?;
        axum_server::bind_rustls(o.addr, config).serve(app.into_make_service()).await?;
    } else {
        let listener = tokio::net::TcpListener::bind(o.addr).await?;
        axum::serve(listener, app).await?;
    }
    Ok(())
}
