mod daemon;

use std::{net::SocketAddr, path::PathBuf, time::Duration};

use anyhow::{Context, Result, bail};
use axum::Router;
use clap::{Args, Parser, Subcommand};
use path_to_link::{
    AppState, CreateShareRequest, ShareInfo, canonicalize_roots, generate_secret, http_api,
    mcp_api::PathToLinkMcp,
};
use rmcp::{ServiceExt, transport::stdio};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;
use url::Url;

#[derive(Debug, Parser)]
#[command(name = "ptl", version, about = "Share temporary links to local files")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Start the local server, optionally sharing a file.
    Run(RunArgs),
    /// Revoke a file link or stop the local server.
    Close(CloseArgs),
    /// Show the local server and its active links.
    Status,
    /// Change a setting on the running local server.
    Change {
        #[command(subcommand)]
        setting: Change,
    },
    /// Run the MCP server for an MCP client configuration.
    Mcp(McpArgs),
    /// Internal process used to run the local server in the background.
    #[command(name = "__serve", hide = true)]
    InternalServe,
}

#[derive(Debug, Subcommand)]
enum Change {
    /// Move the local server to another available port.
    Port,
}

#[derive(Debug, Args)]
struct RunArgs {
    /// File to share after starting the local server.
    path: Option<PathBuf>,
    /// Link lifetime in seconds (defaults to 900).
    #[arg(long, requires = "path", value_parser = clap::value_parser!(u64).range(1..=path_to_link::MAX_EXPIRATION_SECONDS))]
    expires_in: Option<u64>,
}

#[derive(Debug, Args)]
struct CloseArgs {
    /// File path to revoke, or `all` to stop the local server.
    path: PathBuf,
}

#[derive(Debug, Args)]
struct McpArgs {
    /// Address to bind. Port 0 selects an available local port.
    #[arg(long, default_value = "127.0.0.1:0")]
    bind: SocketAddr,
    /// A directory whose files may be shared. May be specified more than once.
    #[arg(long = "allow-root", required = true)]
    allowed_roots: Vec<PathBuf>,
    /// Base URL clients use to reach this server. Required for non-loopback binds.
    #[arg(long)]
    public_base_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SharesResponse {
    shares: Vec<ShareInfo>,
}

#[derive(Debug, Serialize)]
struct RevokeByPathRequest {
    path: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    match Cli::parse().command {
        Command::Run(args) => run_command(args).await,
        Command::Close(args) => close_command(args).await,
        Command::Status => status_command().await,
        Command::Change {
            setting: Change::Port,
        } => change_port_command().await,
        Command::Mcp(args) => run_mcp(args).await,
        Command::InternalServe => daemon::serve().await,
    }
}

async fn change_port_command() -> Result<()> {
    let api = daemon::connect(false)
        .await?
        .context("Path-to-Link is stopped. Start it with `ptl run`.")?;
    let response = api
        .client
        .post(format!("{}/api/port", api.base_url))
        .bearer_auth(&api.control_token)
        .send()
        .await
        .context("Could not change the local server port")?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        bail!(
            "This server does not support changing ports. Restart it with `ptl close all` and `ptl run`."
        );
    }
    let body = response.text().await?;
    if !status.is_success() {
        let message: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
        let reason = message["error"]
            .as_str()
            .unwrap_or("The server rejected the request");
        bail!("Could not change the local server port: {reason}");
    }
    println!("Path-to-Link is now running at {body}");
    println!("Run `ptl status` to see the updated links.");
    Ok(())
}

async fn run_command(args: RunArgs) -> Result<()> {
    let path = match args.path {
        Some(path) => {
            let path = tokio::fs::canonicalize(&path)
                .await
                .with_context(|| format!("Could not access {}", path.display()))?;
            if !tokio::fs::metadata(&path).await?.is_file() {
                bail!("Expected a file: {}", path.display());
            }
            Some(path)
        }
        None => None,
    };
    let api = daemon::connect(true)
        .await?
        .context("Local server did not start")?;
    let Some(path) = path else {
        println!("Path-to-Link is running at {}", api.base_url);
        return Ok(());
    };
    if api.legacy {
        bail!("Run `ptl close all` to stop the previous version, then retry sharing the file.");
    }

    let share: ShareInfo = api
        .client
        .post(format!("{}/api/shares", api.base_url))
        .bearer_auth(&api.control_token)
        .json(&CreateShareRequest {
            path: path
                .into_os_string()
                .into_string()
                .map_err(|_| anyhow::anyhow!("The file path is not valid UTF-8"))?,
            expires_in: args.expires_in,
        })
        .send()
        .await
        .context("Could not create a link")?
        .error_for_status()
        .context("The server rejected the share")?
        .json()
        .await
        .context("Invalid share response")?;
    println!("{}", share.url);
    println!("Valid for {} seconds", share.expires_in_seconds);
    Ok(())
}

async fn close_command(args: CloseArgs) -> Result<()> {
    let Some(api) = daemon::connect(false).await? else {
        println!("Path-to-Link is stopped.");
        return Ok(());
    };
    if args.path.as_os_str() == "all" {
        api.client
            .post(format!("{}/api/shutdown", api.base_url))
            .bearer_auth(&api.control_token)
            .send()
            .await
            .context("Could not stop the local server")?
            .error_for_status()
            .context("The server rejected the shutdown request")?;
        println!(
            "Path-to-Link is shutting down. Its links are no longer available for new requests."
        );
        return Ok(());
    }

    let mut path = std::path::absolute(args.path)?;
    if api.legacy {
        path = path_to_link::canonicalize_for_revoke(&path)
            .await
            .unwrap_or(path);
    }
    let path = path
        .into_os_string()
        .into_string()
        .map_err(|_| anyhow::anyhow!("The file path is not valid UTF-8"))?;
    let response = api
        .client
        .delete(format!("{}/api/shares/by-path", api.base_url))
        .bearer_auth(&api.control_token)
        .json(&RevokeByPathRequest { path: path.clone() })
        .send()
        .await
        .context("Could not revoke the link")?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        println!("No active link for {}", display_path(&path));
        return Ok(());
    }
    response
        .error_for_status()
        .context("The server rejected the revocation")?;
    println!("Link revoked: {}", display_path(&path));
    Ok(())
}

async fn status_command() -> Result<()> {
    let Some(api) = daemon::connect(false).await? else {
        println!("Path-to-Link is stopped.");
        return Ok(());
    };
    let shares: SharesResponse = api
        .client
        .get(format!("{}/api/shares", api.base_url))
        .bearer_auth(&api.control_token)
        .send()
        .await
        .context("Could not list links")?
        .error_for_status()
        .context("The server rejected the request")?
        .json()
        .await?;

    println!("Path-to-Link is running at {}", api.base_url);
    if shares.shares.is_empty() {
        println!("No active links.");
    }
    for share in shares.shares {
        println!("\n{}", display_path(&share.path));
        println!("  {}", share.url);
        println!("  expires in {} seconds", share.expires_in_seconds);
    }
    Ok(())
}

fn display_path(path: &str) -> std::borrow::Cow<'_, str> {
    #[cfg(windows)]
    {
        if let Some(path) = path.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{path}").into();
        }
        if let Some(path) = path.strip_prefix(r"\\?\") {
            if path.as_bytes().get(1) == Some(&b':') {
                return path.into();
            }
        }
    }
    path.into()
}

async fn run_mcp(args: McpArgs) -> Result<()> {
    let listener = TcpListener::bind(args.bind)
        .await
        .with_context(|| format!("could not bind to {}", args.bind))?;
    let address = listener.local_addr()?;
    let roots = canonicalize_roots(&args.allowed_roots).map_err(anyhow::Error::msg)?;
    let base_url = resolve_base_url(address, args.public_base_url.as_deref())?;
    let state = AppState::new(roots, base_url.clone(), &generate_secret());

    let http_state = state.clone();
    let http_task = tokio::spawn(async move {
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let router = http_api::router(http_state.clone(), shutdown_tx);
        if let Err(error) = run_server(listener, http_state, router, shutdown_rx).await {
            tracing::error!(%error, "Path-to-Link HTTP server stopped");
        }
    });

    tracing::info!(%address, base_url = %base_url, "Path-to-Link MCP and HTTP servers started");
    let service = PathToLinkMcp::new(state)
        .serve(stdio())
        .await
        .context("failed to start MCP stdio transport")?;
    let result = service
        .waiting()
        .await
        .context("MCP server stopped unexpectedly")
        .map(|_| ());
    http_task.abort();
    result
}

async fn run_server(
    listener: TcpListener,
    cleanup_state: AppState,
    router: Router,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    let cleanup_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            let removed = cleanup_state.prune_expired().await;
            if removed > 0 {
                tracing::debug!(removed, "expired shares removed");
            }
        }
    });
    let shutdown = async move {
        tokio::select! {
            _ = shutdown_rx.changed() => {},
            _ = async {
                if tokio::signal::ctrl_c().await.is_err() {
                    std::future::pending::<()>().await;
                }
            } => {},
        }
    };
    let result = axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await
        .context("HTTP server failed");
    cleanup_task.abort();
    result
}

fn resolve_base_url(address: SocketAddr, configured: Option<&str>) -> Result<Url> {
    let base_url = if let Some(configured) = configured {
        let url = Url::parse(configured).context("--public-base-url must be an absolute URL")?;
        if url.scheme() != "http"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || (url.path() != "/" && !url.path().is_empty())
        {
            bail!("--public-base-url must be an HTTP origin such as http://127.0.0.1:38751");
        }
        url
    } else if address.ip().is_loopback() {
        Url::parse(&format!("http://{address}/"))?
    } else {
        bail!("--public-base-url is required when binding outside loopback");
    };
    Ok(base_url)
}
