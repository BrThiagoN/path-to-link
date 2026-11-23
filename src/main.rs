use std::{
    env, fs,
    net::SocketAddr,
    path::PathBuf,
    process::{Command as ProcessCommand, Stdio},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use axum::Router;
use clap::{Args, Parser, Subcommand};
use path_to_link::{
    AppState, CreateShareRequest, ShareInfo, canonicalize_roots, generate_secret, http_api,
    mcp_api::PathToLinkMcp, valid_control_token,
};
use reqwest::Client;
use rmcp::{ServiceExt, transport::stdio};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;
use url::Url;

const LOCAL_SERVER_ADDRESS: &str = "127.0.0.1:38751";
const LOCAL_SERVER_URL: &str = "http://127.0.0.1:38751";
const CONTROL_TOKEN_ENV: &str = "PATH_TO_LINK_CONTROL_TOKEN";
const DEFAULT_EXPIRATION_SECONDS: u64 = 900;

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
    /// Run the MCP server for an MCP client configuration.
    Mcp(McpArgs),
    /// Internal process used to run the local server in the background.
    #[command(name = "__serve", hide = true)]
    InternalServe,
}

#[derive(Debug, Args)]
struct RunArgs {
    /// File to share after starting the local server.
    path: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct CloseArgs {
    /// File path to revoke, or `all` to stop the local server.
    path: String,
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

#[derive(Debug, Serialize, Deserialize)]
struct DaemonConfig {
    control_token: String,
}

#[derive(Debug, Deserialize)]
struct HealthResponse {
    status: String,
    service: String,
}

#[derive(Debug, Deserialize)]
struct SharesResponse {
    shares: Vec<ShareInfo>,
}

#[derive(Debug, Serialize)]
struct AddRootRequest {
    path: String,
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
        Command::Mcp(args) => run_mcp(args).await,
        Command::InternalServe => run_managed_server().await,
    }
}

async fn run_command(args: RunArgs) -> Result<()> {
    let Some(path) = args.path else {
        ensure_local_server().await?;
        println!("Path-to-Link está rodando em {LOCAL_SERVER_URL}");
        return Ok(());
    };

    let canonical_path = tokio::fs::canonicalize(&path)
        .await
        .with_context(|| format!("não foi possível acessar o arquivo: {}", path.display()))?;
    let metadata = tokio::fs::metadata(&canonical_path).await?;
    if !metadata.is_file() {
        bail!(
            "o caminho precisa apontar para um arquivo: {}",
            path.display()
        );
    }
    let parent = canonical_path
        .parent()
        .context("não foi possível determinar a pasta do arquivo")?;
    let api = ensure_local_server().await?;

    api.client
        .post(format!("{}/api/roots", api.base_url))
        .bearer_auth(&api.control_token)
        .json(&AddRootRequest {
            path: parent.to_string_lossy().into_owned(),
        })
        .send()
        .await
        .context("não foi possível autorizar a pasta local")?
        .error_for_status()
        .context("o servidor recusou a pasta do arquivo")?;

    let response = api
        .client
        .post(format!("{}/api/shares", api.base_url))
        .bearer_auth(&api.control_token)
        .json(&CreateShareRequest {
            path: canonical_path.to_string_lossy().into_owned(),
            expires_in: Some(DEFAULT_EXPIRATION_SECONDS),
        })
        .send()
        .await
        .context("não foi possível criar o link")?;
    let share: ShareInfo = response
        .error_for_status()
        .context("o servidor recusou o compartilhamento")?
        .json()
        .await
        .context("o servidor retornou uma resposta inválida")?;
    println!("{}", share.url);
    println!("Válido por {} segundos", share.expires_in_seconds);
    Ok(())
}

async fn close_command(args: CloseArgs) -> Result<()> {
    if args.path.eq_ignore_ascii_case("all") {
        return close_all().await;
    }

    let path = PathBuf::from(args.path);
    let canonical_path = tokio::fs::canonicalize(&path)
        .await
        .with_context(|| format!("não foi possível acessar o caminho: {}", path.display()))?;
    let Some(api) = connected_local_server().await? else {
        println!("Path-to-Link não está rodando.");
        return Ok(());
    };

    let response = api
        .client
        .delete(format!("{}/api/shares/by-path", api.base_url))
        .bearer_auth(&api.control_token)
        .json(&RevokeByPathRequest {
            path: canonical_path.to_string_lossy().into_owned(),
        })
        .send()
        .await
        .context("não foi possível revogar o link")?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        println!("Não há link ativo para {}", canonical_path.display());
        return Ok(());
    }
    response
        .error_for_status()
        .context("o servidor recusou a revogação")?;
    println!("Link revogado: {}", canonical_path.display());
    Ok(())
}

async fn close_all() -> Result<()> {
    let Some(api) = connected_local_server().await? else {
        clear_daemon_config()?;
        println!("Path-to-Link não está rodando.");
        return Ok(());
    };
    api.client
        .post(format!("{}/api/shutdown", api.base_url))
        .bearer_auth(&api.control_token)
        .send()
        .await
        .context("não foi possível encerrar o servidor local")?
        .error_for_status()
        .context("o servidor recusou o encerramento")?;
    clear_daemon_config()?;
    println!("Path-to-Link foi encerrado. Todos os links foram invalidados.");
    Ok(())
}

async fn status_command() -> Result<()> {
    let Some(api) = connected_local_server().await? else {
        println!("Path-to-Link não está rodando.");
        return Ok(());
    };
    let response = api
        .client
        .get(format!("{}/api/shares", api.base_url))
        .bearer_auth(&api.control_token)
        .send()
        .await
        .context("não foi possível consultar os links")?
        .error_for_status()
        .context("o servidor recusou a consulta")?;
    let shares: SharesResponse = response.json().await?;

    println!("Path-to-Link está rodando em {LOCAL_SERVER_URL}");
    if shares.shares.is_empty() {
        println!("Nenhum link ativo.");
        return Ok(());
    }
    for share in shares.shares {
        println!("\n{}", share.path);
        println!("  {}", share.url);
        println!("  expira em {} segundos", share.expires_in_seconds);
    }
    Ok(())
}

#[derive(Clone)]
struct LocalApi {
    client: Client,
    base_url: String,
    control_token: String,
}

async fn ensure_local_server() -> Result<LocalApi> {
    let client = create_http_client()?;
    if let Some(config) = read_daemon_config()? {
        let api = local_api(client.clone(), config.control_token);
        if server_is_usable(&client, &api.control_token).await {
            return Ok(api);
        }
    }

    let control_token = generate_secret();
    save_daemon_config(&DaemonConfig {
        control_token: control_token.clone(),
    })?;

    let executable = env::current_exe().context("não foi possível localizar o executável ptl")?;
    let mut command = ProcessCommand::new(executable);
    command
        .arg("__serve")
        .env(CONTROL_TOKEN_ENV, &control_token)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x00000008;
        command.creation_flags(DETACHED_PROCESS);
    }
    command
        .spawn()
        .context("não foi possível iniciar o servidor em segundo plano")?;

    for _ in 0..50 {
        if server_is_healthy(&client).await {
            return Ok(local_api(client, control_token));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    clear_daemon_config()?;
    bail!("o servidor não iniciou em {LOCAL_SERVER_URL}; verifique se a porta 38751 está livre")
}

async fn connected_local_server() -> Result<Option<LocalApi>> {
    let Some(config) = read_daemon_config()? else {
        return Ok(None);
    };
    let client = create_http_client()?;
    if !server_is_usable(&client, &config.control_token).await {
        clear_daemon_config()?;
        return Ok(None);
    }
    Ok(Some(local_api(client, config.control_token)))
}

fn local_api(client: Client, control_token: String) -> LocalApi {
    LocalApi {
        client,
        base_url: LOCAL_SERVER_URL.to_string(),
        control_token,
    }
}

fn create_http_client() -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .context("não foi possível criar o cliente HTTP")
}

async fn server_is_healthy(client: &Client) -> bool {
    let Ok(response) = client
        .get(format!("{LOCAL_SERVER_URL}/health"))
        .send()
        .await
    else {
        return false;
    };
    if !response.status().is_success() {
        return false;
    }
    response
        .json::<HealthResponse>()
        .await
        .is_ok_and(|health| health.status == "ok" && health.service == "path-to-link")
}

async fn server_is_usable(client: &Client, control_token: &str) -> bool {
    if !server_is_healthy(client).await {
        return false;
    }
    client
        .get(format!("{LOCAL_SERVER_URL}/api/shares"))
        .bearer_auth(control_token)
        .send()
        .await
        .is_ok_and(|response| response.status().is_success())
}

fn daemon_config_path() -> Result<PathBuf> {
    #[cfg(windows)]
    let base = env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .context("LOCALAPPDATA não está definido")?;
    #[cfg(not(windows))]
    let base = env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .context("não foi possível localizar a pasta de estado do usuário")?;
    Ok(base.join("path-to-link").join("daemon.json"))
}

fn read_daemon_config() -> Result<Option<DaemonConfig>> {
    let path = daemon_config_path()?;
    match fs::read(&path) {
        Ok(contents) => Ok(Some(
            serde_json::from_slice(&contents).context("arquivo de estado local inválido")?,
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => {
            Err(error).with_context(|| format!("não foi possível ler {}", path.display()))
        }
    }
}

fn save_daemon_config(config: &DaemonConfig) -> Result<()> {
    let path = daemon_config_path()?;
    let parent = path.parent().context("caminho de estado inválido")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("não foi possível criar {}", parent.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    let contents = serde_json::to_vec(config)?;
    fs::write(&path, contents)
        .with_context(|| format!("não foi possível gravar {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn clear_daemon_config() -> Result<()> {
    let path = daemon_config_path()?;
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("não foi possível remover {}", path.display()))
        }
    }
}

async fn run_managed_server() -> Result<()> {
    let token = env::var(CONTROL_TOKEN_ENV).context("token interno do servidor ausente")?;
    if !valid_control_token(&token) {
        bail!("token interno do servidor inválido");
    }
    let address: SocketAddr = LOCAL_SERVER_ADDRESS.parse()?;
    let listener = TcpListener::bind(address)
        .await
        .with_context(|| format!("não foi possível abrir {LOCAL_SERVER_URL}"))?;
    let base_url = Url::parse(&format!("http://{address}/"))?;
    let state = AppState::new(Vec::new(), base_url, &token);
    tracing::info!(%address, "Path-to-Link local server started");
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
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    let router: Router = http_api::router(state, shutdown_tx);
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
            _ = tokio::signal::ctrl_c() => {},
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
