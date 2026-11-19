<div align="center">

# 🔗 Path-to-Link

### Local files, temporary links, and full control.

Share an approved file with a local tool or AI agent over HTTP — without uploading or duplicating the file.

[![Latest release](https://img.shields.io/github/v/release/BrThiagoN/path-to-link?display_name=tag&label=release)](https://github.com/BrThiagoN/path-to-link/releases/latest)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platforms](https://img.shields.io/badge/platforms-Windows%20%7C%20macOS%20%7C%20Linux-7c3aed)](#installation)

[Installation](#installation) · [Quick start](#quick-start) · [MCP](#mcp-setup) · [Security](#security) · [Development](#development)

</div>

---

Path-to-Link serves files directly from your machine. You choose which folders are allowed, each link gets a private token and an expiry time, and you can revoke it whenever you want.

## ✨ What it does

| | |
|---|---|
| 📁 **Folder allowlist** | Shares only files inside roots you explicitly approve. |
| ⏱️ **Temporary links** | Links expire automatically; the default is 15 minutes. |
| 🔐 **Private access tokens** | Every link has its own high-entropy token. |
| 🚫 **Instant revocation** | Revoke a link at any time. |
| 🖥️ **Local by default** | The HTTP server binds to loopback unless you choose another interface. |
| 🤖 **MCP and CLI** | Use it from an MCP client or manage a standalone server from your terminal. |

## Installation

### Windows

Run this in PowerShell:

```powershell
irm https://raw.githubusercontent.com/BrThiagoN/path-to-link/main/install.ps1 | iex
```

The installer downloads the latest Windows build, checks its SHA-256 checksum, and adds Path-to-Link to your user PATH.

### macOS and Linux

Run this in a terminal:

```sh
curl -fsSL https://raw.githubusercontent.com/BrThiagoN/path-to-link/main/install.sh | sh
```

The installer detects your CPU, verifies the download checksum, installs to `~/.local/bin`, and adds that directory to your shell profile if needed. Open a new terminal after installation.

### Install from source

If you already have stable Rust and Cargo installed:

```sh
cargo install --git https://github.com/BrThiagoN/path-to-link
```

Check that it is ready:

```sh
path-to-link --help
```

> **No release available yet?** The installers download the latest published release. Until the first release is created, install from source or build locally with `cargo build --release`.

## Quick start

Start the MCP server and allow it to share files from one folder:

```sh
path-to-link mcp --allow-root "/path/to/shared-files"
```

For example, on Windows:

```powershell
path-to-link mcp --allow-root "C:/Users/you/Shared"
```

The MCP server starts an HTTP listener on an available loopback port and exposes three tools:

| Tool | Purpose |
|---|---|
| `share_file` | Create a temporary link for a file under an allowed root. |
| `list_shares` | See active links and their remaining lifetime. |
| `revoke_share` | Disable a link immediately. |

Each share is independent, so you can keep several links active at once. The default lifetime is 900 seconds; the maximum is 86,400 seconds.

## MCP setup

Add Path-to-Link to your MCP client's configuration. Replace the example path with a folder on your machine:

```json
{
  "mcpServers": {
    "path-to-link": {
      "command": "path-to-link",
      "args": [
        "mcp",
        "--allow-root",
        "C:/Users/you/Shared"
      ]
    }
  }
}
```

The exact configuration file varies by MCP client. If the client cannot find `path-to-link`, use the full path to the installed executable.

The client and the file-consuming tool must be able to reach the same machine and network namespace. The default `127.0.0.1` address works when both run on the same host.

## Standalone HTTP server

Use standalone mode when you want a long-running HTTP server and CLI commands that manage it.

Start the server in one terminal:

```powershell
$env:PATH_TO_LINK_CONTROL_TOKEN = (& path-to-link token)
path-to-link serve --allow-root "C:/Users/you/Shared"
```

Then create and manage links from another terminal:

```powershell
$env:PATH_TO_LINK_CONTROL_TOKEN = "paste-the-same-token-here"
path-to-link share "C:/Users/you/Shared/example.pdf" --expires-in 3600
path-to-link list
path-to-link revoke <share-id>
```

Keep the control token in an environment variable. Do not place it directly in a command that may be saved in shell history.

## Security

- Only files below a configured `--allow-root` can be shared. Path-to-Link resolves paths before checking them.
- The server binds to `127.0.0.1` by default. A non-loopback bind requires an explicit `--public-base-url`.
- A share URL contains its own access token. Anyone with the full URL can fetch that file until the link expires or is revoked. Treat the URL as a secret.
- Files are streamed from disk on demand; they are not copied into Path-to-Link or uploaded by the server.
- Share records live in memory. Restarting the process revokes all active links.

Use a non-loopback bind only when the consuming client must connect from another device, and only on a network where you can protect the URLs.

## Supported release builds

| Operating system | Architectures |
|---|---|
| Windows | x86-64, ARM64 |
| macOS | Intel (x86-64), Apple silicon (ARM64) |
| Linux | x86-64, ARM64 |

Release binaries are provided by the project and do not require Rust to be installed. The macOS and Linux installer uses `curl` or `wget`; the Windows installer uses PowerShell.

## Build a release

Maintainers can publish a version by pushing a tag such as `v0.1.0`:

```sh
git tag v0.1.0
git push origin v0.1.0
```

GitHub Actions builds binaries for the supported platforms, generates SHA-256 checksums, and attaches them to a GitHub Release.

## Development

Requirements: stable Rust and Cargo.

```sh
cargo fmt --all
cargo check --locked
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for project contribution notes.

## License

Path-to-Link is available under the [MIT License](LICENSE).
