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

/// `None` if the flag is absent, `Some(None)` if present without a value.
fn flag(args: &[String], name: &str) -> Option<Option<String>> {
    let i = args.iter().position(|a| a == name)?;
    Some(args.get(i + 1).filter(|v| !v.starts_with("--")).cloned())
}

/// Value of `--name <value>`, falling back to an environment variable.
fn setting(args: &[String], name: &str, var: &str) -> Result<Option<String>> {
    match flag(args, name) {
        Some(Some(v)) => Ok(Some(v)),
        Some(None) => anyhow::bail!("{name} needs a value"),
        None => Ok(std::env::var(var).ok().filter(|v| !v.is_empty())),
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

    // Transport: MCP_TRANSPORT in .env (stdio, http or https), or --http / --https.
    // Flags override the environment. The default is stdio.
    let args: Vec<String> = std::env::args().collect();
    let flag_http = flag(&args, "--http");
    let flag_https = flag(&args, "--https");
    anyhow::ensure!(flag_http.is_none() || flag_https.is_none(), "use either --http or --https, not both");
    let transport = if flag_https.is_some() {
        "https".to_string()
    } else if flag_http.is_some() {
        "http".to_string()
    } else {
        std::env::var("MCP_TRANSPORT").unwrap_or_else(|_| "stdio".into()).to_ascii_lowercase()
    };
    if transport != "stdio" {
        anyhow::ensure!(
            transport == "http" || transport == "https",
            "MCP_TRANSPORT must be stdio, http or https"
        );
        let listen = flag_https
            .or(flag_http)
            .flatten()
            .or_else(|| std::env::var("MCP_LISTEN").ok().filter(|v| !v.is_empty()))
            .unwrap_or_else(|| "0.0.0.0:8808".into());
        let opts = http::Options {
            addr: listen.parse().with_context(|| format!("invalid listen address '{listen}'"))?,
            tls: transport == "https",
            tls_cert: setting(&args, "--tls-cert", "MCP_TLS_CERT")?,
            tls_key: setting(&args, "--tls-key", "MCP_TLS_KEY")?,
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
