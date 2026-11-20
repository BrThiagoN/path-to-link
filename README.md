# Path-to-Link

Path-to-Link is a Rust local file server that turns an allowed filesystem path into a temporary HTTP link. It is intended for developer tools and AI agents that can consume URLs but cannot read local paths directly.

The file stays on the host machine. Path-to-Link serves it on demand and does not upload or duplicate it. By default, the HTTP server binds to loopback. A link has its own secret access token, can expire automatically, and can be revoked.

## Requirements

- Rust stable and Cargo
- Windows, macOS, or Linux

## Build

```powershell
cargo build --release
```

## MCP mode

The MCP command starts the stdio server and an HTTP listener on an available loopback port. The listener runs in the same process as MCP, so it stays available while the agent session is active.

Example MCP client configuration (adapt this JSON structure to the client's settings format):

```json
{
  "mcpServers": {
    "path-to-link": {
      "command": "C:\\path\\to\\path-to-link.exe",
      "args": [
        "mcp",
        "--allow-root",
        "C:\\Users\\user\\Shared"
      ]
    }
  }
}
```

The MCP tools are:

- `share_file`: accept an absolute file path and optional lifetime; return a URL and share ID.
- `list_shares`: list active links and remaining lifetimes.
- `revoke_share`: immediately disable a link by ID.

Every share has its own ID and access token. Multiple shares stay active concurrently. The default lifetime is 900 seconds; the maximum is 86400 seconds. Keep the MCP process alive until the client has finished fetching the file.

For any MCP workflow that needs a file URL, call `share_file` with the local path and pass the returned URL to the consuming tool. If both processes can reach the same host and network namespace, the default loopback URL works without publishing the file to the internet. If the consumer runs elsewhere, bind the HTTP server to a reachable interface and provide a matching `--public-base-url`; the file must be reachable from that client's network.

## Standalone HTTP mode

Use this mode when CLI commands should manage one long-running server. The control API requires a bearer token; set it in the environment rather than putting it in the command line.

```powershell
$env:PATH_TO_LINK_CONTROL_TOKEN = (& .\target\release\path-to-link.exe token)
& .\target\release\path-to-link.exe serve --allow-root "C:\Shared"
```

In another PowerShell session, set the same `PATH_TO_LINK_CONTROL_TOKEN`, then use:

```powershell
& .\target\release\path-to-link.exe share "C:\Shared\example.pdf" --expires-in 3600
& .\target\release\path-to-link.exe list
& .\target\release\path-to-link.exe revoke <share-id>
```

`PATH_TO_LINK_URL` selects another server address; the default is `http://127.0.0.1:38751`.

To allow a client on the LAN, bind to a specific interface or `0.0.0.0` and provide `--public-base-url` with the address clients can reach. The control API remains protected by its control token; file URLs carry a separate per-share bearer token. Use LAN mode only on a network where those URLs can be protected.

## Project goals

- Several shares can be active at the same time; creating a new share never replaces another one.
- An MCP server exposes `share_file`, `list_shares`, and `revoke_share`.
- A CLI supports starting the HTTP service and managing shares.
- Only files below configured allowed roots can be shared. Canonical path checks resolve symbolic links before authorizing a file.
- Share expiration and explicit revocation stop future requests immediately.
- Localhost is the default. Binding to a LAN address must be an explicit choice.
- The HTTP endpoint streams the original file over standard HTTP and supports range requests for clients that need streaming or partial reads.

## Architecture

The executable provides three interfaces over one in-memory share registry:

1. An Axum/Tokio HTTP server streams files from disk.
2. An MCP stdio server lets an agent create, list, and revoke shares. Its normal mode starts the HTTP listener alongside MCP.
3. CLI commands manage a separately running HTTP server.

Share records are temporary and live in memory. Restarting the server revokes its active shares. A local MCP client and the file-consuming API must be able to reach the returned HTTP address; `127.0.0.1` is only reachable from the same host/network namespace.

## Development

```powershell
cargo fmt --all
cargo check
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for implementation guidance.
