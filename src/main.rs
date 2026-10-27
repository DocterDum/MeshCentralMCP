mod audit;
mod http;
mod mesh;
mod server;

use anyhow::{Context, Result};
use mesh::{Credentials, Mesh};
use rmcp::ServiceExt;
use server::{Config, MeshServer};
use std::sync::Arc;

fn env(name: &str) -> Result<String> {
    std::env::var(name).with_context(|| format!("{name} is not set (see .env.example)"))
}

/// Value following `--flag`, if the flag is present.
fn flag_value(args: &[String], flag: &str) -> Result<Option<String>> {
    match args.iter().position(|a| a == flag) {
        None => Ok(None),
        Some(i) => args
            .get(i + 1)
            .cloned()
            .map(Some)
            .with_context(|| format!("{flag} needs a value")),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let _ = dotenvy::dotenv();

    let cfg = Arc::new(Config {
        audit_path: std::env::var("MESH_AUDIT_LOG").unwrap_or_else(|_| "audit.jsonl".into()),
    });
    let mesh = Arc::new(Mesh::new(
        env("MESH_SERVER_URL")?,
        Credentials { user: env("MESH_TOKEN_USER")?, pass: env("MESH_TOKEN_PASS")? },
    ));

    // `--http <addr>` or `--https <addr>` serves Streamable HTTP; the default is stdio.
    let args: Vec<String> = std::env::args().collect();
    let http = flag_value(&args, "--http")?;
    let https = flag_value(&args, "--https")?;
    anyhow::ensure!(http.is_none() || https.is_none(), "use either --http or --https, not both");
    if let Some(addr) = https.clone().or(http) {
        let addr = addr.parse().context("invalid listen address, e.g. 127.0.0.1:8808")?;
        let opts = http::Options {
            addr,
            tls: https.is_some(),
            tls_cert: flag_value(&args, "--tls-cert")?,
            tls_key: flag_value(&args, "--tls-key")?,
            new_token: args.iter().any(|a| a == "--new-token"),
        };
        return http::serve(opts, mesh, cfg).await;
    }

    let svc = MeshServer::new(mesh, cfg)
        .serve(rmcp::transport::stdio())
        .await
        .context("failed to start MCP server on stdio")?;
    svc.waiting().await?;
    Ok(())
}
