use std::{
    fs,
    path::PathBuf,
    process::{Command, Output, Stdio},
    time::Duration,
};

struct Workspace(PathBuf);

impl Workspace {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("ptl-cli-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ptl"));
        command.args(args).current_dir(&self.0);
        #[cfg(windows)]
        command.env("LOCALAPPDATA", &self.0);
        #[cfg(not(windows))]
        command.env("XDG_STATE_HOME", &self.0);
        command
    }

    fn run(&self, args: &[&str]) -> String {
        success(self.command(args).output().unwrap())
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = self.command(&["close", "all"]).output();
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[tokio::test]
async fn local_server_lifecycle() {
    let workspace = Workspace::new();
    let file = workspace.0.join("all");
    fs::write(&file, "test content").unwrap();
    let mut first_command = workspace.command(&["run", "all"]);
    let first = first_command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let second = workspace.run(&["run", "./all"]);
    let first = success(first.wait_with_output().unwrap());
    let link = first.lines().next().unwrap();
    assert_eq!(Some(link), second.lines().next());

    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    assert_eq!(
        client.get(link).send().await.unwrap().text().await.unwrap(),
        "test content"
    );
    let status = workspace.run(&["status"]);
    assert!(status.contains(link));
    assert!(!status.contains(r"\\?\"));

    let old_url = url::Url::parse(link).unwrap();
    let old_port = old_url.port().unwrap();
    let change_url = old_url.join("/api/port").unwrap();
    assert_eq!(client.post(change_url).send().await.unwrap().status(), 401);
    let change = workspace.run(&["change", "port"]);
    let new_base = change
        .lines()
        .next()
        .unwrap()
        .strip_prefix("Path-to-Link is now running at ")
        .unwrap();
    let new_port = url::Url::parse(new_base).unwrap().port().unwrap();
    assert_ne!(new_port, old_port);
    let mut new_url = old_url;
    new_url.set_port(Some(new_port)).unwrap();
    let link = new_url.as_str();
    let status = workspace.run(&["status"]);
    assert!(status.contains(new_base));
    assert!(status.contains(link));
    assert_eq!(
        client.get(link).send().await.unwrap().text().await.unwrap(),
        "test content"
    );

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, old_port))
        .await
        .is_ok()
    {
        assert!(
            std::time::Instant::now() < deadline,
            "Old port was not closed"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let config_path = workspace.0.join("path-to-link").join("daemon.json");
    let config = fs::read(&config_path).unwrap();
    let mut invalid: serde_json::Value = serde_json::from_slice(&config).unwrap();
    invalid["control_token"] = path_to_link::generate_secret().into();
    let invalid = serde_json::to_vec(&invalid).unwrap();
    fs::write(&config_path, &invalid).unwrap();
    let result = workspace.command(&["status"]).output();
    let preserved = fs::read(&config_path);
    fs::write(&config_path, &config).unwrap();
    let result = result.unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("rejected the control token"));
    assert_eq!(preserved.unwrap(), invalid);

    fs::remove_file(&file).unwrap();
    assert!(workspace.run(&["close", "./all"]).contains("Link revoked"));
    fs::write(&file, "test content").unwrap();
    assert_eq!(client.get(link).send().await.unwrap().status(), 404);
    let new_share = workspace.run(&["run", "all", "--expires-in", "60"]);
    assert!(new_share.contains("Valid for 60 seconds"));
    assert_ne!(Some(link), new_share.lines().next());

    workspace.run(&["close", "all"]);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while config_path.exists() {
        assert!(std::time::Instant::now() < deadline, "Server did not stop");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(workspace.run(&["status"]).contains("is stopped"));
}
