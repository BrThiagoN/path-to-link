use std::{net::SocketAddr, path::PathBuf, time::Duration};

use anyhow::{Context, Result, bail};
use axum::Router;
use clap::{Args, Parser, Subcommand};
use path_to_link::{
    AppState, ShareInfo, canonicalize_roots, generate_secret, http_api, mcp_api::PathToLinkMcp,
    valid_control_token,
};
use reqwest::Client;
use rmcp::{ServiceExt, transport::stdio};
use serde::Serialize;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;
use url::Url;

#[derive(Debug, Parser)]
#[command(name = "path-to-link", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the authenticated HTTP service for CLI-managed shares.
    Serve(ServerArgs),
    /// Run MCP over stdio and start a local HTTP listener for its share links.
    Mcp(McpArgs),
    /// Create a share through a running `serve` process.
    Share(ShareArgs),
    /// List shares from a running `serve` process.
    List(ServerClientArgs),
    /// Revoke a share from a running `serve` process.
    Revoke(RevokeArgs),
    /// Generate a high-entropy control token for the HTTP management API.
    Token,
}

#[derive(Debug, Args)]
struct ServerArgs {
    /// Address to bind. Keep this on 127.0.0.1 unless LAN access is needed.
    #[arg(long, default_value = "127.0.0.1:38751")]
    bind: SocketAddr,
    /// A directory whose files may be shared. May be specified more than once.
    #[arg(long = "allow-root", required = true)]
    allowed_roots: Vec<PathBuf>,
    /// Control API bearer token; set PATH_TO_LINK_CONTROL_TOKEN instead of a CLI argument.
    #[arg(long, env = "PATH_TO_LINK_CONTROL_TOKEN")]
    control_token: Option<String>,
    /// Base URL clients use to reach this server. Required for non-loopback binds.
    #[arg(long)]
    public_base_url: Option<String>,
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

#[derive(Debug, Args)]
struct ServerClientArgs {
    /// URL of a running `serve` process.
    #[arg(
        long,
        env = "PATH_TO_LINK_URL",
        default_value = "http://127.0.0.1:38751"
    )]
    server_url: String,
    /// Control API token; set PATH_TO_LINK_CONTROL_TOKEN instead of a CLI argument.
    #[arg(long, env = "PATH_TO_LINK_CONTROL_TOKEN")]
    control_token: Option<String>,
}

#[derive(Debug, Args)]
struct ShareArgs {
    /// Absolute path of the file to share.
    path: PathBuf,
    /// Link lifetime in seconds (1 to 86400).
    #[arg(long, default_value_t = 900)]
    expires_in: u64,
    #[command(flatten)]
    server: ServerClientArgs,
}

#[derive(Debug, Args)]
struct RevokeArgs {
    /// ID returned when the share was created.
    id: String,
    #[command(flatten)]
    server: ServerClientArgs,
}

#[derive(Serialize)]
struct CliCreateShare {
    path: String,
    expires_in: u64,
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
        Command::Serve(args) => {
            run_http_server(
                args.bind,
                args.allowed_roots,
                args.public_base_url,
                require_control_token(args.control_token)?,
            )
            .await
        }
        Command::Mcp(args) => run_mcp(args).await,
        Command::Share(args) => create_share_via_api(args).await,
        Command::List(args) => list_shares_via_api(args).await,
        Command::Revoke(args) => revoke_via_api(args).await,
        Command::Token => {
            println!("{}", generate_secret());
            Ok(())
        }
    }
}

async fn run_http_server(
    bind: SocketAddr,
    allowed_roots: Vec<PathBuf>,
    public_base_url: Option<String>,
    control_token: String,
) -> Result<()> {
    let listener = TcpListener::bind(bind)
        .await
        .with_context(|| format!("could not bind to {bind}"))?;
    let address = listener.local_addr()?;
    let roots = canonicalize_roots(&allowed_roots).map_err(anyhow::Error::msg)?;
    let base_url = resolve_base_url(address, public_base_url.as_deref())?;
    let state = AppState::new(roots, base_url.clone(), &control_token);
    tracing::info!(%address, base_url = %base_url, "Path-to-Link HTTP server started");
    run_server(listener, state).await
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
        if let Err(error) = run_server(listener, http_state).await {
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

async fn run_server(listener: TcpListener, state: AppState) -> Result<()> {
    let cleanup_state = state.clone();
    let router: Router = http_api::router(state);
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
    let shutdown = async {
        let _ = tokio::signal::ctrl_c().await;
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

fn require_control_token(token: Option<String>) -> Result<String> {
    let token = token.unwrap_or_default();
    if !valid_control_token(&token) {
        bail!(
            "set PATH_TO_LINK_CONTROL_TOKEN to a token with at least 32 characters; `path-to-link token` generates one"
        );
    }
    Ok(token)
}

fn client_config(args: &ServerClientArgs) -> Result<(Client, String, String)> {
    let token = require_control_token(args.control_token.clone())?;
    let mut server_url =
        Url::parse(&args.server_url).context("--server-url must be an absolute URL")?;
    if server_url.scheme() != "http"
        || server_url.host_str().is_none()
        || server_url.query().is_some()
        || server_url.fragment().is_some()
    {
        bail!("--server-url must be an HTTP origin, such as http://127.0.0.1:38751");
    }
    server_url.set_path("");
    let base = server_url.as_str().trim_end_matches('/').to_string();
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("could not create HTTP client")?;
    Ok((client, base, token))
}

async fn create_share_via_api(args: ShareArgs) -> Result<()> {
    let (client, base, token) = client_config(&args.server)?;
    let request = CliCreateShare {
        path: args.path.to_string_lossy().into_owned(),
        expires_in: args.expires_in,
    };
    let response = client
        .post(format!("{base}/api/shares"))
        .bearer_auth(token)
        .json(&request)
        .send()
        .await
        .context("could not reach Path-to-Link server")?;
    let share: ShareInfo = response
        .error_for_status()
        .context("server rejected share request")?
        .json()
        .await
        .context("server returned an invalid share response")?;
    println!("{}", serde_json::to_string_pretty(&share)?);
    Ok(())
}

async fn list_shares_via_api(args: ServerClientArgs) -> Result<()> {
    let (client, base, token) = client_config(&args)?;
    let response = client
        .get(format!("{base}/api/shares"))
        .bearer_auth(token)
        .send()
        .await
        .context("could not reach Path-to-Link server")?;
    let body: serde_json::Value = response
        .error_for_status()
        .context("server rejected list request")?
        .json()
        .await
        .context("server returned invalid JSON")?;
    println!("{}", serde_json::to_string_pretty(&body)?);
    Ok(())
}

async fn revoke_via_api(args: RevokeArgs) -> Result<()> {
    let (client, base, token) = client_config(&args.server)?;
    let response = client
        .delete(format!("{base}/api/shares/{}", args.id))
        .bearer_auth(token)
        .send()
        .await
        .context("could not reach Path-to-Link server")?;
    let body: serde_json::Value = response
        .error_for_status()
        .context("server rejected revoke request")?
        .json()
        .await
        .context("server returned invalid JSON")?;
    println!("{}", serde_json::to_string_pretty(&body)?);
    Ok(())
}
