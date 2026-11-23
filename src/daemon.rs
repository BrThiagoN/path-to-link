use std::{
    env,
    fs::{self, File, OpenOptions, TryLockError},
    io::{ErrorKind, Write},
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use axum::Router;
use path_to_link::{AppState, generate_secret, http_api};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use tokio::{
    net::TcpListener,
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
};
use url::Url;

const LEGACY_PORT: u16 = 38751;
const START_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Serialize, Deserialize)]
struct Config {
    control_token: String,
    // Older versions only stored the token and used a fixed port.
    endpoint: Option<Endpoint>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Endpoint {
    port: u16,
    instance: String,
    pid: u32,
}

pub struct LocalApi {
    pub client: Client,
    pub base_url: String,
    pub control_token: String,
    pub legacy: bool,
}

enum Daemon {
    Stopped,
    Starting,
    Running(LocalApi),
}

pub async fn connect(start: bool) -> Result<Option<LocalApi>> {
    let state_path = state_path()?;
    let client = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(2))
        .build()?;
    let mut launched = false;
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        match inspect(&state_path, &client).await? {
            Daemon::Running(api) => return Ok(Some(api)),
            Daemon::Stopped if !start => return Ok(None),
            Daemon::Stopped if !launched => {
                spawn()?;
                launched = true;
            }
            Daemon::Starting | Daemon::Stopped => {}
        }
        if Instant::now() >= deadline {
            bail!(
                "The local server did not become ready. Stop any remaining ptl background process before retrying. State file: {}",
                state_path.display()
            );
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn inspect(path: &Path, client: &Client) -> Result<Daemon> {
    let managed = match open_lock(path) {
        Ok(lock) => match lock.try_lock() {
            Ok(()) => false,
            Err(TryLockError::WouldBlock) => true,
            Err(TryLockError::Error(error)) => {
                return Err(error).context("Could not check server lock");
            }
        },
        Err(error) if error.kind() == ErrorKind::NotFound => false,
        Err(error) => return Err(error).context("Could not open server lock"),
    };
    let Some(config) = read_config(path)? else {
        return Ok(if managed {
            Daemon::Starting
        } else {
            Daemon::Stopped
        });
    };
    if managed != config.endpoint.is_some() {
        return Ok(if managed {
            Daemon::Starting
        } else {
            Daemon::Stopped
        });
    }

    let port = config
        .endpoint
        .as_ref()
        .map_or(LEGACY_PORT, |endpoint| endpoint.port);
    let base_url = format!("http://127.0.0.1:{port}");
    if let Some(endpoint) = &config.endpoint {
        let response = client.get(format!("{base_url}/api/instance")).send().await;
        let Ok(response) = response else {
            return Ok(Daemon::Starting);
        };
        if !response.status().is_success() || response.text().await? != endpoint.instance {
            bail!(
                "Server identity does not match {}. Stop the ptl background process (PID {}) before retrying.",
                path.display(),
                endpoint.pid
            );
        }
    } else {
        let Ok(response) = client.get(format!("{base_url}/health")).send().await else {
            return Ok(Daemon::Stopped);
        };
        let health = response
            .json::<serde_json::Value>()
            .await
            .unwrap_or_default();
        if health["service"] != "path-to-link" || health["status"] != "ok" {
            return Ok(Daemon::Stopped);
        }
    }
    let response = client
        .get(format!("{base_url}/api/shares"))
        .bearer_auth(&config.control_token)
        .send()
        .await;
    let response = match response {
        Ok(response) => response,
        Err(error) if managed && (error.is_connect() || error.is_request()) => {
            return Ok(Daemon::Starting);
        }
        Err(error) => return Err(error).context("Could not connect to the local control API"),
    };
    if response.status() == StatusCode::UNAUTHORIZED {
        bail!(
            "The local server rejected the control token in {}. Restore the matching state file or stop the ptl background process before retrying.",
            path.display()
        );
    }
    response
        .error_for_status()
        .context("Local control API returned an error")?;
    Ok(Daemon::Running(LocalApi {
        client: client.clone(),
        base_url,
        control_token: config.control_token,
        legacy: config.endpoint.is_none(),
    }))
}

#[cfg(unix)]
fn spawn() -> Result<()> {
    use std::{
        os::unix::process::CommandExt,
        process::{Command, Stdio},
    };

    let mut command = Command::new(env::current_exe().context("Could not locate ptl")?);
    command
        .arg("__serve")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .context("Could not start the background server")?;
    Ok(())
}

#[cfg(windows)]
fn spawn() -> Result<()> {
    use std::{os::windows::ffi::OsStrExt, ptr::null};

    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{
            CREATE_NEW_PROCESS_GROUP, CreateProcessW, DETACHED_PROCESS, PROCESS_INFORMATION,
            STARTUPINFOW,
        },
    };

    let executable: Vec<u16> = env::current_exe()
        .context("Could not locate ptl")?
        .as_os_str()
        .encode_wide()
        .chain([0])
        .collect();
    let mut args: Vec<u16> = "ptl __serve\0".encode_utf16().collect();
    let startup = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut process = PROCESS_INFORMATION::default();

    // Do not inherit the CLI's pipes when the caller captures stdout or stderr.
    // SAFETY: The strings are NUL terminated, the command line is writable, and
    // the input and output structs stay valid for the call.
    let created = unsafe {
        CreateProcessW(
            executable.as_ptr(),
            args.as_mut_ptr(),
            null(),
            null(),
            0,
            DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP,
            null(),
            null(),
            &startup,
            &mut process,
        )
    };
    if created == 0 {
        return Err(std::io::Error::last_os_error())
            .context("Could not start the background server");
    }
    // SAFETY: A successful CreateProcessW returns these two owned handles.
    unsafe {
        CloseHandle(process.hProcess);
        CloseHandle(process.hThread);
    }
    Ok(())
}

pub async fn serve() -> Result<()> {
    let path = state_path()?;
    let parent = path.parent().context("Invalid server state path")?;
    fs::create_dir_all(parent).context("Could not create the server state directory")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    let lock = open_lock(&path)?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Ok(()),
        Err(TryLockError::Error(error)) => {
            return Err(error).context("Could not acquire server lock");
        }
    }

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let address = listener.local_addr()?;
    let instance = generate_secret();
    let control_token = generate_secret();
    let state = AppState::managed(Url::parse(&format!("http://{address}/"))?, &control_token);
    let config = Config {
        control_token,
        endpoint: Some(Endpoint {
            port: address.port(),
            instance: instance.clone(),
            pid: std::process::id(),
        }),
    };
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (port_tx, port_rx) = mpsc::channel(1);
    let router = http_api::managed_router(state.clone(), shutdown_tx, instance, port_tx);
    save_config(&path, &config)?;

    let result = run_managed(listener, state, router, shutdown_rx, port_rx, &path, config).await;
    let cleanup = fs::remove_file(&path);
    drop(lock);
    result?;
    match cleanup {
        Err(error) if error.kind() != ErrorKind::NotFound => {
            Err(error).context("Could not remove local server state")
        }
        _ => Ok(()),
    }
}

type ChangePortReply = oneshot::Sender<Result<SocketAddr, String>>;

struct ActiveServer {
    stop: watch::Sender<bool>,
    task: JoinHandle<Result<()>>,
}

impl ActiveServer {
    fn start(listener: TcpListener, state: &AppState, router: &Router) -> Self {
        let (stop, shutdown_rx) = watch::channel(false);
        let task = tokio::spawn(super::run_server(
            listener,
            state.clone(),
            router.clone(),
            shutdown_rx,
        ));
        Self { stop, task }
    }
}

async fn run_managed(
    listener: TcpListener,
    state: AppState,
    router: Router,
    mut shutdown: watch::Receiver<bool>,
    mut port_changes: mpsc::Receiver<ChangePortReply>,
    path: &Path,
    mut config: Config,
) -> Result<()> {
    let mut active = ActiveServer::start(listener, &state, &router);
    loop {
        tokio::select! {
            biased;
            _ = shutdown.changed() => break,
            result = &mut active.task => return result.context("Local server task stopped")?,
            Some(reply) = port_changes.recv() => {
                match bind_next_port(path, &mut config).await {
                    Ok((listener, address)) => {
                        state.set_local_port(address.port()).await;
                        let old = std::mem::replace(&mut active, ActiveServer::start(listener, &state, &router));
                        let _ = old.stop.send(true);
                        let _ = reply.send(Ok(address));
                    }
                    Err(error) => {
                        tracing::warn!(%error, "Could not change the local server port");
                        let _ = reply.send(Err(error.to_string()));
                    }
                }
            }
        }
    }
    drop(port_changes);
    let _ = active.stop.send(true);
    active.task.await.context("Local server task stopped")?
}

async fn bind_next_port(path: &Path, config: &mut Config) -> Result<(TcpListener, SocketAddr)> {
    // The current listener stays bound while the OS assigns the new port.
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .context("Could not bind a new local port")?;
    let address = listener.local_addr()?;
    let mut next = config.clone();
    next.endpoint
        .as_mut()
        .context("Missing local server endpoint")?
        .port = address.port();
    save_config(path, &next)?;
    *config = next;
    Ok((listener, address))
}

fn state_path() -> Result<PathBuf> {
    #[cfg(windows)]
    let base = PathBuf::from(env::var_os("LOCALAPPDATA").context("LOCALAPPDATA is not set")?);
    #[cfg(not(windows))]
    let base = env::var_os("XDG_STATE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .context("Could not locate the user state directory")?;
    Ok(base.join("path-to-link").join("daemon.json"))
}

fn open_lock(path: &Path) -> std::io::Result<File> {
    // Keep this file in place. Deleting it would allow callers to lock different files.
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path.with_extension("lock"))
}

fn read_config(path: &Path) -> Result<Option<Config>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes).with_context(|| {
            format!("Invalid server state in {}. Restore it or stop the ptl background process before removing the file.", path.display())
        })?)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("Could not read {}", path.display())),
    }
}

fn save_config(path: &Path, config: &Config) -> Result<()> {
    let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> Result<()> {
        let mut file = options.open(&temp)?;
        file.write_all(&serde_json::to_vec(config)?)?;
        drop(file);
        fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.with_context(|| format!("Could not save state to {}", path.display()))
}
