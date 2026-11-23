# Path-to-Link command reference

## CLI commands

| Command | What it does |
| --- | --- |
| `ptl help` | List available commands. |
| `ptl help <command>` | Show usage and options for a command, such as `ptl help run`. |
| `ptl --version` | Print the installed version. |
| `ptl run` | Start the server in the background on an available port at `127.0.0.1`. If it is already running, print its address. |
| `ptl run <file>` | Start the server if needed and print a temporary link for the file. |
| `ptl run <file> --expires-in <seconds>` | Share the file with a custom lifetime from 1 to 86,400 seconds. The default is 900 seconds (15 minutes). |
| `ptl status` | Show the server address, active links, and how long each link has left. |
| `ptl change port` | Move the running server to another available local port. Run `ptl status` to get updated URLs. |
| `ptl close <file>` | Revoke the link for one file. Use `ptl close ./all` for a file named `all`. |
| `ptl close all` | Stop the background server and invalidate all its links. |
| `ptl mcp --allow-root <directory>` | Start the MCP server over stdio and allow sharing files from the directory. Repeat `--allow-root` to allow more directories. |

## Common tasks

Share a file from the current directory for one hour, see its link, then revoke it.

```sh
ptl run "report draft.pdf" --expires-in 3600
ptl status
ptl close "report draft.pdf"
```

Quote paths with spaces. Sharing the same file again reuses its active link and renews its lifetime.

Move the running server to a new port and copy the updated links from `ptl status`.

```sh
ptl change port
ptl status
```

Active shares keep their expiry times. Replace any URLs you previously shared because they point to the old port.

## MCP tools

These tools are available from an MCP client connected to `ptl mcp`.

| Tool | Arguments | What it does |
| --- | --- | --- |
| `share_file` | `path`, optional `expires_in` | Share a file at an absolute path within an allowed directory, or renew its active link. |
| `list_shares` | None | List active links and their remaining lifetimes. |
| `revoke_share` | `id` | Revoke the link with the ID returned by `share_file`. |

See the [MCP setup in the README](../README.md#mcp-setup) for connection instructions and `ptl help mcp` for bind options.
