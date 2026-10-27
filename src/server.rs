//! MCP tool surface. Transport-agnostic: main.rs serves this over stdio or HTTP.

use crate::mesh::{Group, Mesh, Node, Shell};
use crate::{audit, safety};
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::{schemars, tool, tool_handler, tool_router, ErrorData, ServerHandler};
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
pub struct Config {
    pub audit_path: String,
}

#[derive(Clone)]
pub struct MeshServer {
    mesh: Arc<Mesh>,
    cfg: Arc<Config>,
    tool_router: ToolRouter<Self>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct ListDevicesArgs {
    /// Only devices in this device group (exact name, case-insensitive).
    group: Option<String>,
    /// Only devices whose name contains this text (case-insensitive).
    name_contains: Option<String>,
    /// Only devices whose agent is currently connected.
    online_only: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct RunCommandArgs {
    /// Device name or full node id. Names must match exactly one device.
    device: String,
    /// The command to run.
    command: String,
    /// "auto" (default), "cmd", "powershell" or "sh".
    shell: Option<String>,
    /// Seconds to wait for output (default 60).
    timeout_secs: Option<u64>,
}

fn err(msg: impl Into<String>) -> ErrorData {
    ErrorData::invalid_params(msg.into(), None)
}

fn internal(e: impl std::fmt::Display) -> ErrorData {
    ErrorData::internal_error(e.to_string(), None)
}

#[tool_router]
impl MeshServer {
    pub fn new(mesh: Arc<Mesh>, cfg: Arc<Config>) -> Self {
        Self { mesh, cfg, tool_router: Self::tool_router() }
    }

    #[tool(description = "List MeshCentral device groups.", annotations(read_only_hint = true))]
    async fn mesh_list_groups(&self) -> Result<CallToolResult, ErrorData> {
        let (groups, nodes) = self.mesh.inventory().await.map_err(internal)?;
        let mut out = String::new();
        for g in &groups {
            let n = nodes.iter().filter(|n| n.group_id == g.id).count();
            out.push_str(&format!("{}  ({} devices)\n", g.name, n));
        }
        Ok(CallToolResult::success(vec![ContentBlock::text(out)]))
    }

    #[tool(
        description = "List managed devices with group, online status and OS. Device names are not unique across groups.",
        annotations(read_only_hint = true)
    )]
    async fn mesh_list_devices(
        &self,
        Parameters(a): Parameters<ListDevicesArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let (groups, nodes) = self.mesh.inventory().await.map_err(internal)?;
        let gname = |id: &str| groups.iter().find(|g| g.id == id).map(|g| g.name.as_str()).unwrap_or("?");
        let mut rows: Vec<String> = nodes
            .iter()
            .filter(|n| a.group.as_deref().is_none_or(|g| gname(&n.group_id).eq_ignore_ascii_case(g)))
            .filter(|n| {
                a.name_contains
                    .as_deref()
                    .is_none_or(|s| n.name.to_lowercase().contains(&s.to_lowercase()))
            })
            .filter(|n| !a.online_only.unwrap_or(false) || n.online)
            .map(|n| {
                format!(
                    "{} | group: {} | {} | {}",
                    n.name,
                    gname(&n.group_id),
                    if n.online { "online" } else { "offline" },
                    n.os
                )
            })
            .collect();
        rows.sort();
        let text = if rows.is_empty() { "no matching devices".to_string() } else { rows.join("\n") };
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    #[tool(
        description = "Run a command on a managed device and return its output. Every call is written to an audit log.",
        annotations(read_only_hint = false, destructive_hint = true, open_world_hint = true)
    )]
    async fn mesh_run_command(
        &self,
        Parameters(a): Parameters<RunCommandArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let shell = Shell::parse(a.shell.as_deref().unwrap_or("auto")).map_err(|e| err(e.to_string()))?;
        let wait = Duration::from_secs(a.timeout_secs.unwrap_or(60).max(1));

        let (groups, nodes) = self.mesh.inventory().await.map_err(internal)?;
        let want = a.device.to_lowercase();
        let by_id: Vec<&Node> = nodes.iter().filter(|n| n.id == a.device).collect();
        let matches = if by_id.is_empty() {
            nodes.iter().filter(|n| n.name.to_lowercase() == want).collect()
        } else {
            by_id
        };
        let node = match matches.as_slice() {
            [one] => *one,
            [] => return Err(err(format!("no device named '{}'", a.device))),
            many => return Err(err(ambiguous(&a.device, many, &groups))),
        };

        let started = std::time::Instant::now();
        let result = self.mesh.run(node.id.clone(), shell, a.command.clone(), wait).await;
        audit::log(
            &self.cfg.audit_path,
            serde_json::json!({
                "tool": "mesh_run_command", "device": node.name, "node_id": node.id,
                "shell": format!("{shell:?}"), "command": a.command,
                "ok": result.is_ok(), "ms": started.elapsed().as_millis() as u64,
                "output_bytes": result.as_ref().map(|s| s.len()).unwrap_or(0),
            }),
        );
        let out = result.map_err(internal)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(safety::wrap_output(&out))]))
    }
}

fn ambiguous(name: &str, many: &[&Node], groups: &[Group]) -> String {
    let list: Vec<String> = many
        .iter()
        .map(|n| {
            let g = groups.iter().find(|g| g.id == n.group_id).map(|g| g.name.as_str()).unwrap_or("?");
            format!("{} (group {g}) id={}", n.name, n.id)
        })
        .collect();
    format!("'{name}' matches several devices; retry with the full id:\n{}", list.join("\n"))
}

#[tool_handler(
    router = self.tool_router,
    instructions = "Troubleshoot devices managed by MeshCentral."
)]
impl ServerHandler for MeshServer {}
