# MeshCentralMCP

An MCP server that lets an AI assistant (Claude) troubleshoot devices managed by [MeshCentral](https://github.com/Ylianst/MeshCentral), with a human approving every action.

**Status:** early development, nothing usable yet.

## Goals

- Read-only by default. Command, terminal and file tools are opt-in.
- Every command needs a plain-language reason, shown in the approval prompt and written to MeshCentral's device event log.
- Scoped access: one dedicated MeshCentral account (login token) per customer or device group, never admin.
- Device output is treated as untrusted data: sanitised, length-capped and clearly marked.
- Local audit log of every call, with secrets redacted.

## Planned tools

| Tier | Tools |
|---|---|
| Read | list devices and groups, device info, event log |
| Execute | run command (CMD / PowerShell / shell), terminal sessions |
| Files | list, read, upload, download |

## Security notes

The server can only do what its MeshCentral account allows, so that account is the real security boundary. Prompt injection from device content (logs, files, web pages) is reduced by design but not eliminated: read each approval prompt.

## License

[AGPL-3.0](LICENSE)
