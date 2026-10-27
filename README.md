# MeshCentralMCP

An MCP server that lets an AI assistant (Claude) troubleshoot devices managed by [MeshCentral](https://github.com/Ylianst/MeshCentral), with a human approving every command.

Written in Rust. Talks to MeshCentral over `wss://<server>/control.ashx`, the same channel its web UI uses, logged in with a login token. Nothing is exposed on customer PCs: they keep using their existing agent connection.

## Tools

| Tool | Profile | What it does |
|---|---|---|
| `mesh_list_groups` | readonly | List device groups |
| `mesh_list_devices` | readonly | List devices (filter by group, name, online) |
| `mesh_run_command` | support | Run a CMD / PowerShell / shell command and return its output |

## Safety model

- **Read-only by default.** `mesh_run_command` is not even registered unless `MESH_PROFILE=support`.
- **A human approves each command.** The tool is marked non-read-only and destructive, so MCP clients such as Claude Code prompt before every call, showing the exact command.
- **A reason is mandatory** for every command and is written to the audit log.
- **Scoped visibility.** `MESH_GROUPS` limits an instance to named device groups. Run one instance and one MeshCentral account per customer.
- **Device output is untrusted.** Control and ANSI characters are stripped, output is capped at 64 KiB, and it is wrapped in markers telling the model to treat it as data.
- **Speed bumps, not a boundary.** Commands that look like they stop the Mesh Agent or power the device off are refused (restarts are allowed). This is a simple text filter and can be bypassed. The real boundary is the MeshCentral account's rights plus the human approving each call.
- **Audit log.** Every command is appended to `audit.jsonl` (device, reason, command, outcome, duration).
- **Duplicate names.** Device names are not unique in MeshCentral, so an ambiguous name is rejected with the matching ids.

Prompt injection from device content (logs, files, web pages) is reduced by design but not eliminated: read each approval prompt.

## Setup

1. In MeshCentral, create a **dedicated user** with rights only on the device groups you want, then create a login token for it (My Account).
2. Copy `.env.example` to `.env` and fill it in.
3. Build: `cargo build --release` (needs Rust and, on Windows, the MSVC Build Tools).

### Claude Code / Claude desktop (stdio)

```json
{
  "mcpServers": {
    "meshcentral": {
      "command": "C:\\path\\to\\MeshCentralMCP\\target\\release\\meshcentral-mcp.exe",
      "env": { "MESH_PROFILE": "support" }
    }
  }
}
```

The server reads `.env` from its working directory.

### Streamable HTTP

```
MCP_HTTP_TOKEN=<24+ random characters> meshcentral-mcp --http 127.0.0.1:8808
```

Serves `http://127.0.0.1:8808/mcp` and requires `Authorization: Bearer <token>`. It refuses to bind a non-loopback address unless `MCP_HTTP_ALLOW_REMOTE=1`, because it speaks plain HTTP: put a TLS reverse proxy in front for remote use. In this version the HTTP mode shares one MeshCentral login for all callers; per-user pass-through of login tokens is future work.

## Status

Working and tested against a live MeshCentral server: login, device listing, command execution with output, the safety filter, and both transports. Not yet implemented: terminals, file transfer, remote-desktop screenshots, per-caller auth in HTTP mode.

## License

[AGPL-3.0](LICENSE)
