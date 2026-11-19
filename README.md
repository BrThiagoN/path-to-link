# Path-to-Link

Path-to-Link is a Rust local file server that turns an allowed filesystem path into a temporary HTTP link. It is intended for developer tools and AI agents that can consume URLs but cannot read local paths directly.

The file stays on the host machine. Path-to-Link serves it on demand and does not upload or duplicate it. By default, the HTTP server binds to loopback. A link has its own secret access token, can expire automatically, and can be revoked.

## Requirements

- Rust stable and Cargo
- Windows, macOS, or Linux

## Project goals

- Several shares can be active at the same time; creating a new share never replaces another one.
- An MCP server exposes `share_file`, `list_shares`, and `revoke_share`.
- A CLI supports starting the HTTP service and managing shares.
- Only files below configured allowed roots can be shared. Canonical path checks resolve symbolic links before authorizing a file.
- Share expiration and explicit revocation stop future requests immediately.
- Localhost is the default. Binding to a LAN address must be an explicit choice.
- The HTTP endpoint streams the original file and supports normal media clients such as FFmpeg and CapCut.

## Architecture

The executable provides three interfaces over one in-memory share registry:

1. An Axum/Tokio HTTP server streams files from disk.
2. An MCP stdio server lets an agent create, list, and revoke shares. Its normal mode starts the HTTP listener alongside MCP.
3. CLI commands manage a separately running HTTP server.

Share records are temporary and live in memory. Restarting the server revokes its active shares. A local MCP client and the file-consuming API must be able to reach the returned HTTP address; `127.0.0.1` is only reachable from the same host/network namespace.

## Development

```powershell
cargo check
```

See [CLAUDE.md](CLAUDE.md) for project-specific implementation rules.
