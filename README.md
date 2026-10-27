# MeshCentralMCP

An MCP server that lets an AI assistant (Claude) troubleshoot devices managed by [MeshCentral](https://github.com/Ylianst/MeshCentral). Commands are approved one by one in the MCP client (for example Claude Code's tool prompt).

Written in Rust. Talks to MeshCentral over `wss://<server>/control.ashx`, the same channel its web UI uses, logged in with a login token. Nothing is exposed on customer PCs: they keep using their existing agent connection.

## Tools

| Tool | What it does |
|---|---|
| `mesh_list_groups` | List device groups |
| `mesh_list_devices` | List devices (filter by group, name, online) |
| `mesh_run_command` | Run a CMD / PowerShell / shell command and return its output |

The server sees whatever the MeshCentral account behind the login token can see. Every command is appended to `audit.jsonl`. Device names are not unique in MeshCentral, so an ambiguous name is rejected with the matching device ids.

## Setup

1. In MeshCentral, create a login token (My Account).
2. Copy `.env.example` to `.env` and fill it in.
3. Build: `cargo build --release` (needs Rust and, on Windows, the MSVC Build Tools).

### Claude Code / Claude desktop (stdio)

```json
{
  "mcpServers": {
    "meshcentral": {
      "command": "C:\\path\\to\\MeshCentralMCP\\target\\release\\meshcentral-mcp.exe"
    }
  }
}
```

The server reads `.env` from its working directory.

### Streamable HTTP

```
MCP_HTTP_TOKEN=<24+ random characters> meshcentral-mcp --http 127.0.0.1:8808
```

Serves `http://127.0.0.1:8808/mcp` and requires `Authorization: Bearer <token>`. It refuses to bind a non-loopback address unless `MCP_HTTP_ALLOW_REMOTE=1`, because it speaks plain HTTP: put a TLS reverse proxy in front for remote use. In this version the HTTP mode shares one MeshCentral login for all callers.

## Status

Working and tested against a live MeshCentral server: login, device listing, command execution with output, and both transports. Not yet implemented: terminals, file transfer, remote-desktop screenshots, per-caller auth in HTTP mode.

## License

[AGPL-3.0](LICENSE)
