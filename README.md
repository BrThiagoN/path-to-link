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

Path-to-Link serves files directly from your machine. Select files in the CLI or configure allowed folders for MCP. Each link gets a private token and an expiry time, and you can revoke it whenever you want.

## ✨ What it does

| | |
|---|---|
| 📁 **Folder allowlist** | Limits MCP sharing to roots you explicitly approve. |
| ⏱️ **Temporary links** | Links expire automatically; the default is 15 minutes. |
| 🔐 **Private access tokens** | Every link has its own high-entropy token. |
| 🚫 **Instant revocation** | Revoke a link at any time. |
| 🖥️ **Local by default** | The HTTP server binds to loopback unless you choose another interface. |
| 🤖 **MCP and CLI** | Share files from an MCP client or from your terminal with `ptl`. |

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
ptl help
```

> **No release available yet?** The installers download the latest published release. Until the first release is created, install from source or build locally with `cargo build --release`.

## Quick start

Start the local server in the background:

```sh
ptl run
```

Create a temporary link for a file (the server starts automatically if needed):

```powershell
ptl run "C:/Users/you/Shared/example.pdf"
```

List links, revoke one file's link, or stop the server and invalidate all links:

```powershell
ptl status
ptl close "C:/Users/you/Shared/example.pdf"
ptl close all
```

`ptl run` picks an available port on `127.0.0.1`. Each file has one active link at a time. Running the same path again reuses its link and renews its lifetime. The default is 15 minutes. Pass `--expires-in <seconds>` with the file path to set a different lifetime. `ptl close all` stops the background server and invalidates its links. For a file literally named `all`, use `ptl close ./all`.

Run `ptl change port` to move the running server to another available loopback port. Active shares keep their expiry times. Their URLs use the new port, so run `ptl status` to copy the updated links and replace any old URLs you shared.

If you used the previous standalone commands, `ptl run [file]` replaces `serve`, `share`, and `token`. Use `ptl status` in place of `list` and `ptl close <file>` in place of `revoke`. The installed command for source builds is now `ptl`.

## MCP setup

For MCP clients, start Path-to-Link over stdio and allow a folder:

```sh
ptl mcp --allow-root "/path/to/shared-files"
```

The MCP server starts an HTTP listener on an available loopback port and exposes three tools:

| Tool | Purpose |
|---|---|
| `share_file` | Create a temporary link for a file under an allowed root. |
| `list_shares` | See active links and their remaining lifetime. |
| `revoke_share` | Disable a link immediately. |

Each canonical file path has one active link. Repeated calls for that path reuse the link and renew its lifetime. The default lifetime is 900 seconds; the maximum is 86,400 seconds.

## Connect an MCP client

Add Path-to-Link to your MCP client's configuration. Replace the example path with a folder on your machine:

```json
{
  "mcpServers": {
    "ptl": {
      "command": "ptl",
      "args": [
        "mcp",
        "--allow-root",
        "C:/Users/you/Shared"
      ]
    }
  }
}
```

The exact configuration file varies by MCP client. If the client cannot find `ptl`, use the full path to the installed executable.

The client and the file-consuming tool must be able to reach the same machine and network namespace. The default `127.0.0.1` address works when both run on the same host.

## Security

- The CLI shares only the specific file requested with `ptl run <file>`. Other files in the same folder need their own share requests.
- The CLI stores the local control token and selected port in `%LOCALAPPDATA%\path-to-link\daemon.json` on Windows or `${XDG_STATE_HOME:-$HOME/.local/state}/path-to-link/daemon.json` on macOS and Linux. Keep this file private while the server is running. If the token no longer matches, restore the matching state file or stop the background `ptl` process before restarting.
- MCP mode shares only files below folders explicitly passed with `--allow-root`. Path-to-Link resolves paths before checking them.
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
