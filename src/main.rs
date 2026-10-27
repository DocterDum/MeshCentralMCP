mod audit;
mod mesh;
mod safety;
mod server;

use anyhow::{Context, Result};
use mesh::{Credentials, Mesh};
use rmcp::ServiceExt;
use server::{Config, MeshServer, Profile};
use std::sync::Arc;

fn env(name: &str) -> Result<String> {
    std::env::var(name).with_context(|| format!("{name} is not set (see .env.example)"))
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let _ = dotenvy::dotenv();

    let profile = match std::env::var("MESH_PROFILE").unwrap_or_default().as_str() {
        "support" => Profile::Support,
        "" | "readonly" => Profile::ReadOnly,
        other => anyhow::bail!("MESH_PROFILE must be 'readonly' or 'support', got '{other}'"),
    };
    let cfg = Arc::new(Config {
        profile,
        groups: std::env::var("MESH_GROUPS")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        audit_path: std::env::var("MESH_AUDIT_LOG").unwrap_or_else(|_| "audit.jsonl".into()),
    });
    let mesh = Arc::new(Mesh::new(
        env("MESH_SERVER_URL")?,
        Credentials { user: env("MESH_TOKEN_USER")?, pass: env("MESH_TOKEN_PASS")? },
    ));

    // stdout is the MCP channel in stdio mode, so log to stderr only.
    eprintln!(
        "meshcentral-mcp: profile={} groups={}",
        if profile == Profile::Support { "support" } else { "readonly" },
        if cfg.groups.is_empty() { "all".to_string() } else { cfg.groups.join(",") }
    );

    // `--http <addr>` serves Streamable HTTP; the default is stdio.
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--http") {
        let addr: std::net::SocketAddr = args
            .get(i + 1)
            .context("--http needs an address, e.g. 127.0.0.1:8808")?
            .parse()
            .context("invalid --http address")?;
        return serve_http(addr, mesh, cfg).await;
    }

    let svc = MeshServer::new(mesh, cfg)
        .serve(rmcp::transport::stdio())
        .await
        .context("failed to start MCP server on stdio")?;
    svc.waiting().await?;
    Ok(())
}

/// Constant-time byte comparison, so the bearer check doesn't leak length-prefix timing.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn serve_http(addr: std::net::SocketAddr, mesh: Arc<Mesh>, cfg: Arc<Config>) -> Result<()> {
    use axum::{extract::Request, http::StatusCode, middleware::Next, response::Response};
    use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
    use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};

    let token = env("MCP_HTTP_TOKEN")?;
    anyhow::ensure!(token.len() >= 24, "MCP_HTTP_TOKEN must be at least 24 characters");
    anyhow::ensure!(
        addr.ip().is_loopback() || std::env::var("MCP_HTTP_ALLOW_REMOTE").as_deref() == Ok("1"),
        "refusing to bind {addr}: this endpoint has command access to devices and speaks plain HTTP. \
         Bind to 127.0.0.1, or put a TLS reverse proxy in front and set MCP_HTTP_ALLOW_REMOTE=1"
    );

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
    eprintln!("meshcentral-mcp: serving Streamable HTTP on http://{addr}/mcp (bearer token required)");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
