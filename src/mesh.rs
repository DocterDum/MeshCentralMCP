//! MeshCentral control-channel client (wss://host/control.ashx).
//!
//! Protocol facts come from MeshCentral's own `meshctrl.js` and were checked
//! against a live server: auth is the `x-meshauth` header
//! (base64(user),base64(pass)); `runcommands` replies arrive as
//! `{action:"msg", type:"runcommands", responseid, result}`.

use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Clone)]
pub struct Credentials {
    pub user: String,
    pub pass: String,
}

#[derive(Clone, Debug)]
pub struct Group {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct Node {
    pub id: String,
    pub name: String,
    pub group_id: String,
    pub online: bool,
    pub os: String,
}

#[derive(Clone, Copy, Debug)]
pub enum Shell {
    Auto = 0,
    Cmd = 1,
    PowerShell = 2,
    Sh = 3,
}

impl Shell {
    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "auto" | "" => Shell::Auto,
            "cmd" => Shell::Cmd,
            "powershell" | "ps" => Shell::PowerShell,
            "sh" | "bash" | "linux" => Shell::Sh,
            other => bail!("unknown shell '{other}' (use auto, cmd, powershell or sh)"),
        })
    }
}

/// Turn `https://host[/prefix]` into `wss://host[/prefix]/control.ashx`.
pub fn control_url(server: &str) -> Result<String> {
    let s = server.trim().trim_end_matches('/');
    let base = if let Some(r) = s.strip_prefix("https://") {
        format!("wss://{r}")
    } else if let Some(r) = s.strip_prefix("http://") {
        format!("ws://{r}")
    } else if s.starts_with("wss://") || s.starts_with("ws://") {
        s.to_string()
    } else {
        bail!("MESH_SERVER_URL must start with https:// or wss://");
    };
    Ok(format!("{base}/control.ashx"))
}

struct Conn {
    ws: Ws,
    counter: u64,
}

impl Conn {
    async fn open(server: &str, creds: &Credentials) -> Result<Self> {
        let url = control_url(server)?;
        let mut req = url.as_str().into_client_request()?;
        let hdr = format!("{},{}", B64.encode(&creds.user), B64.encode(&creds.pass));
        req.headers_mut()
            .insert("x-meshauth", HeaderValue::from_str(&hdr)?);
        let (ws, _) = timeout(Duration::from_secs(15), connect_async(req))
            .await
            .context("connect timed out")?
            .context("websocket handshake failed (bad URL, TLS error, or login rejected)")?;
        Ok(Self { ws, counter: 0 })
    }

    async fn send(&mut self, v: &Value) -> Result<()> {
        self.ws.send(Message::Text(v.to_string().into())).await?;
        Ok(())
    }

    async fn next_json(&mut self, wait: Duration) -> Result<Option<Value>> {
        loop {
            match timeout(wait, self.ws.next()).await {
                Err(_) | Ok(None) => return Ok(None),
                Ok(Some(Err(e))) => return Err(anyhow!(e)),
                Ok(Some(Ok(Message::Text(t)))) => {
                    if let Ok(v) = serde_json::from_str::<Value>(&t) {
                        return Ok(Some(v));
                    }
                }
                Ok(Some(Ok(Message::Close(_)))) => bail!("server closed the connection"),
                Ok(Some(Ok(_))) => {}
            }
        }
    }

    async fn inventory(&mut self) -> Result<(Vec<Group>, Vec<Node>)> {
        self.counter += 1;
        let rid = format!("inv{}", self.counter);
        self.send(&json!({ "action": "meshes" })).await?;
        self.send(&json!({ "action": "nodes", "responseid": rid })).await?;
        let (mut groups, mut nodes) = (None, None);
        let deadline = Instant::now() + Duration::from_secs(20);
        while groups.is_none() || nodes.is_none() {
            let left = deadline.saturating_duration_since(Instant::now());
            let Some(v) = self.next_json(left).await? else {
                bail!("timed out waiting for the device list");
            };
            match v["action"].as_str() {
                Some("meshes") => {
                    groups = Some(
                        v["meshes"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|m| Group {
                                id: m["_id"].as_str().unwrap_or_default().into(),
                                name: m["name"].as_str().unwrap_or_default().into(),
                            })
                            .collect::<Vec<_>>(),
                    );
                }
                Some("nodes") => {
                    let mut out = Vec::new();
                    for (gid, list) in v["nodes"].as_object().into_iter().flatten() {
                        for n in list.as_array().into_iter().flatten() {
                            out.push(Node {
                                id: n["_id"].as_str().unwrap_or_default().into(),
                                name: n["name"].as_str().unwrap_or_default().into(),
                                group_id: gid.clone(),
                                online: n["conn"].as_u64().unwrap_or(0) & 1 != 0,
                                os: n["osdesc"].as_str().unwrap_or_default().into(),
                            });
                        }
                    }
                    nodes = Some(out);
                }
                _ => {}
            }
        }
        Ok((groups.unwrap(), nodes.unwrap()))
    }

    async fn run(&mut self, node: &str, shell: Shell, cmd: &str, wait: Duration) -> Result<String> {
        self.counter += 1;
        let rid = format!("run{}", self.counter);
        self.send(&json!({
            "action": "runcommands", "nodeids": [node], "type": shell as u32,
            "cmds": cmd, "runAsUser": 0, "reply": true, "responseid": rid
        }))
        .await?;
        let deadline = Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let Some(v) = self.next_json(left).await? else {
                bail!(
                    "no reply within {}s. The command may still be running on the device",
                    wait.as_secs()
                );
            };
            if v["action"] == "msg" && v["type"] == "runcommands" && v["responseid"] == rid.as_str() {
                return Ok(v["result"].as_str().unwrap_or_default().to_string());
            }
        }
    }
}

/// Shared handle: one websocket, serialised requests, lazy reconnect.
pub struct Mesh {
    server: String,
    creds: Credentials,
    conn: Mutex<Option<Conn>>,
}

impl Mesh {
    pub fn new(server: String, creds: Credentials) -> Self {
        Self { server, creds, conn: Mutex::new(None) }
    }

    async fn with_conn<T>(
        &self,
        f: impl for<'a> FnOnce(&'a mut Conn) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<T>> + Send + 'a>>,
    ) -> Result<T> {
        let mut guard = self.conn.lock().await;
        if guard.is_none() {
            *guard = Some(Conn::open(&self.server, &self.creds).await?);
        }
        let res = f(guard.as_mut().unwrap()).await;
        if res.is_err() {
            *guard = None; // drop a possibly-broken socket; next call reconnects
        }
        res
    }

    pub async fn inventory(&self) -> Result<(Vec<Group>, Vec<Node>)> {
        self.with_conn(|c| Box::pin(c.inventory())).await
    }

    pub async fn run(&self, node: String, shell: Shell, cmd: String, wait: Duration) -> Result<String> {
        self.with_conn(move |c| Box::pin(async move { c.run(&node, shell, &cmd, wait).await }))
            .await
    }
}
