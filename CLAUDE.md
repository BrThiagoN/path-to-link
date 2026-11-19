# Path-to-Link contributor notes

## Purpose

Build a local Rust service that exposes approved local files through temporary HTTP links for tools such as CapCut MCP, Codex, OpenCode, and Claude Code.

## Architecture

- Use Tokio and Axum for concurrent HTTP requests.
- Use Tower HTTP file serving so clients can stream media and request byte ranges.
- Use `rmcp` over stdio for the MCP interface.
- Keep MCP, CLI, and HTTP operations connected to the same share registry when they run in one process.
- Keep the default bind address on `127.0.0.1`.

## Security and lifecycle

- Require at least one configured allowed root. Canonicalize roots and requested paths; reject non-files and paths outside every allowed root. This must reject symlink targets outside the allowlist.
- Store each share as an independent registry entry. Multiple links must stay active concurrently.
- Give each share an unpredictable ID and a separate access token. Never log access tokens.
- Validate expiry when serving a request, and also clean expired records in the background. Revocation must take effect immediately.
- Protect management operations with a control token. A non-loopback bind requires an explicit advertised base URL and secure credentials.
- Do not copy or upload source files. Stream the original file from disk.
- Keep MCP protocol output on stdout and diagnostic logs on stderr.

## Local guidance

- `.agents/` is intentionally ignored by Git and may contain machine-local agent notes. Read files there when present, but do not add them to commits.
- No existing `.agents/` instructions were present when this project was initialized.
- Prefer `cargo fmt` and `cargo check` during implementation. Do not run the test suite unless requested.
