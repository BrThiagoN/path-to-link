# Path-to-Link contributor notes

## Purpose

Build a local Rust service that exposes approved local files through temporary HTTP links for MCP clients and other tools that accept HTTP file URLs.

## Architecture

- Use Tokio and Axum for concurrent HTTP requests.
- Use Tower HTTP file serving so clients can stream files and request byte ranges.
- Use `rmcp` over stdio for the MCP interface.
- Keep MCP, CLI, and HTTP operations connected to the same share registry when they run in one process.
- Keep the default bind address on `127.0.0.1`.

## Security and lifecycle

- Require explicit allowed roots in MCP mode. In CLI mode, grant a link to the requested file. Canonicalize paths and check them again when serving files to prevent a symlink redirect outside the authorized scope.
- Keep one active link per canonical path. Multiple distinct paths can stay active concurrently.
- Give each share an unpredictable ID and a separate access token. Never log access tokens.
- Validate expiry when serving a request, and also clean expired records in the background. Revocation must take effect immediately.
- Protect management operations with a control token. A non-loopback bind requires an explicit advertised base URL and secure credentials.
- Do not copy or upload source files. Stream the original file from disk.
- Keep MCP protocol output on stdout and diagnostic logs on stderr.

## Local guidance

- `.agents/` is intentionally ignored by Git and may contain machine-local agent notes. Read files there when present, but do not add them to commits.
- No existing `.agents/` instructions were present when this project was initialized.
- Prefer `cargo fmt` and `cargo check` during implementation. Pull requests targeting `main` run the test suite on Windows and Linux.
