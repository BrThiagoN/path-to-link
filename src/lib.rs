use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::sync::RwLock;
use url::Url;
use uuid::Uuid;

pub mod http_api;
pub mod mcp_api;

pub const DEFAULT_EXPIRATION_SECONDS: u64 = 900;
pub const MAX_EXPIRATION_SECONDS: u64 = 86_400;

#[derive(Clone)]
pub struct AppState {
    inner: Arc<AppStateInner>,
}

struct AppStateInner {
    allowed_roots: Option<Vec<PathBuf>>,
    control_token_hash: [u8; 32],
    shares: RwLock<ShareRegistry>,
}

struct ShareRegistry {
    base_url: Url,
    records: HashMap<String, ShareRecord>,
}

#[derive(Clone)]
struct ShareRecord {
    path: PathBuf,
    info: ShareInfo,
    access_token_hash: [u8; 32],
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CreateShareRequest {
    pub path: String,
    #[serde(default)]
    pub expires_in: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ShareInfo {
    pub id: String,
    pub path: String,
    pub url: String,
    pub created_at_unix: u64,
    pub expires_at_unix: u64,
    pub expires_in_seconds: u64,
}

impl AppState {
    pub fn new(allowed_roots: Vec<PathBuf>, base_url: Url, control_token: &str) -> Self {
        Self::with_roots(Some(allowed_roots), base_url, control_token)
    }

    pub fn managed(base_url: Url, control_token: &str) -> Self {
        Self::with_roots(None, base_url, control_token)
    }

    fn with_roots(allowed_roots: Option<Vec<PathBuf>>, base_url: Url, control_token: &str) -> Self {
        Self {
            inner: Arc::new(AppStateInner {
                allowed_roots,
                control_token_hash: token_hash(control_token),
                shares: RwLock::new(ShareRegistry {
                    base_url,
                    records: HashMap::new(),
                }),
            }),
        }
    }

    pub fn is_control_token_valid(&self, token: &str) -> bool {
        token_hash(token)
            .ct_eq(&self.inner.control_token_hash)
            .into()
    }

    pub async fn set_local_port(&self, port: u16) {
        let mut shares = self.inner.shares.write().await;
        shares
            .base_url
            .set_port(Some(port))
            .expect("local HTTP URLs support ports");
        for record in shares.records.values_mut() {
            let mut url =
                Url::parse(&record.info.url).expect("share URLs are generated from the base URL");
            url.set_port(Some(port))
                .expect("local HTTP URLs support ports");
            record.info.url = url.into();
        }
    }

    pub async fn create_share(&self, request: CreateShareRequest) -> Result<ShareInfo, String> {
        let expires_in = request.expires_in.unwrap_or(DEFAULT_EXPIRATION_SECONDS);
        if !(1..=MAX_EXPIRATION_SECONDS).contains(&expires_in) {
            return Err(format!(
                "expires_in must be between 1 and {MAX_EXPIRATION_SECONDS} seconds"
            ));
        }

        let requested_path = PathBuf::from(&request.path);
        if !requested_path.is_absolute() && self.inner.allowed_roots.is_none() {
            return Err("managed shares require an absolute path".to_string());
        }
        let canonical_path = tokio::fs::canonicalize(&requested_path)
            .await
            .map_err(|_| "file does not exist or cannot be accessed".to_string())?;
        if self.inner.allowed_roots.is_none() && canonical_path != requested_path {
            return Err("file path changed before it could be shared".to_string());
        }
        if !self.is_allowed_path(&canonical_path) {
            return Err("file is outside the configured allowed roots".to_string());
        }

        let metadata = tokio::fs::metadata(&canonical_path)
            .await
            .map_err(|_| "file does not exist or cannot be accessed".to_string())?;
        if !metadata.is_file() {
            return Err("only regular files can be shared".to_string());
        }

        let now = unix_time_now();
        let expires_at = now.saturating_add(expires_in);
        let mut shares = self.inner.shares.write().await;
        shares
            .records
            .retain(|_, record| record.info.expires_at_unix > now);
        if let Some(record) = shares
            .records
            .values_mut()
            .find(|record| record.path == canonical_path)
        {
            record.info.expires_at_unix = expires_at;
            record.info.expires_in_seconds = expires_in;
            return Ok(record.info.clone());
        }

        let id = Uuid::new_v4().simple().to_string();
        let access_token = generate_secret();
        let mut link = shares.base_url.clone();
        link.path_segments_mut()
            .map_err(|_| "configured base URL cannot contain path segments".to_string())?
            .pop_if_empty()
            .push("f")
            .push(&id);
        link.query_pairs_mut().append_pair("token", &access_token);

        let info = ShareInfo {
            id: id.clone(),
            path: canonical_path.to_string_lossy().into_owned(),
            url: link.into(),
            created_at_unix: now,
            expires_at_unix: expires_at,
            expires_in_seconds: expires_in,
        };
        let record = ShareRecord {
            path: canonical_path,
            info: info.clone(),
            access_token_hash: token_hash(&access_token),
        };

        shares.records.insert(id, record);
        Ok(info)
    }

    pub async fn list_shares(&self) -> Vec<ShareInfo> {
        let now = unix_time_now();
        let mut shares = self.inner.shares.write().await;
        shares
            .records
            .retain(|_, record| record.info.expires_at_unix > now);

        let mut result: Vec<_> = shares
            .records
            .values()
            .map(|record| {
                let mut info = record.info.clone();
                info.expires_in_seconds = info.expires_at_unix.saturating_sub(now);
                info
            })
            .collect();
        result.sort_by(|a, b| a.expires_at_unix.cmp(&b.expires_at_unix));
        result
    }

    pub async fn revoke_share(&self, id: &str) -> bool {
        self.inner.shares.write().await.records.remove(id).is_some()
    }

    pub async fn revoke_share_by_path(&self, path: &Path) -> bool {
        let canonical_path = canonicalize_for_revoke(path).await;
        let mut shares = self.inner.shares.write().await;
        let now = unix_time_now();
        let mut revoked = false;
        shares.records.retain(|_, record| {
            if record.info.expires_at_unix <= now {
                return false;
            }
            let matches = record.path == path || canonical_path.as_ref() == Some(&record.path);
            revoked |= matches;
            !matches
        });
        revoked
    }

    pub async fn resolve_file(&self, id: &str, token: &str) -> Option<PathBuf> {
        let now = unix_time_now();
        let record = {
            let mut shares = self.inner.shares.write().await;
            let record = shares.records.get(id)?.clone();
            if record.info.expires_at_unix <= now {
                shares.records.remove(id);
                return None;
            }
            if !bool::from(token_hash(token).ct_eq(&record.access_token_hash)) {
                return None;
            }
            record
        };

        // Resolve again at request time so replacing a shared path with a symlink
        // cannot redirect a later request outside the allowed roots.
        let path = tokio::fs::canonicalize(&record.path).await.ok()?;
        if !self.is_allowed_path(&path)
            || (self.inner.allowed_roots.is_none() && path != record.path)
        {
            self.revoke_share(id).await;
            return None;
        }
        let metadata = tokio::fs::metadata(&path).await.ok()?;
        metadata.is_file().then_some(path)
    }

    pub async fn prune_expired(&self) -> usize {
        let now = unix_time_now();
        let mut shares = self.inner.shares.write().await;
        let previous_count = shares.records.len();
        shares
            .records
            .retain(|_, record| record.info.expires_at_unix > now);
        previous_count - shares.records.len()
    }

    fn is_allowed_path(&self, path: &Path) -> bool {
        self.inner
            .allowed_roots
            .as_ref()
            .is_none_or(|roots| roots.iter().any(|root| path.starts_with(root)))
    }
}

pub async fn canonicalize_for_revoke(path: &Path) -> Option<PathBuf> {
    let mut ancestor = path;
    let mut missing = Vec::new();
    loop {
        if let Ok(mut candidate) = tokio::fs::canonicalize(ancestor).await {
            candidate.extend(missing.into_iter().rev());
            return Some(candidate);
        }
        missing.push(ancestor.file_name()?);
        ancestor = ancestor.parent()?;
    }
}

pub fn canonicalize_roots(roots: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    if roots.is_empty() {
        return Err("at least one --allow-root directory is required".to_string());
    }

    let mut canonical_roots = Vec::with_capacity(roots.len());
    for root in roots {
        let canonical = std::fs::canonicalize(root)
            .map_err(|_| format!("allowed root does not exist: {}", root.display()))?;
        if !canonical.is_dir() {
            return Err(format!(
                "allowed root is not a directory: {}",
                root.display()
            ));
        }
        if !canonical_roots.contains(&canonical) {
            canonical_roots.push(canonical);
        }
    }
    Ok(canonical_roots)
}

pub fn generate_secret() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

pub fn valid_control_token(token: &str) -> bool {
    token.len() >= 32
}

fn token_hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

fn unix_time_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!("path-to-link-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("create temporary test root");
        root
    }

    fn test_state(roots: Vec<PathBuf>) -> AppState {
        AppState::new(
            roots,
            Url::parse("http://127.0.0.1:38751/").expect("valid test URL"),
            "test-control-token-with-enough-entropy",
        )
    }

    #[tokio::test]
    async fn repeated_share_for_path_reuses_link_and_renews_expiry() {
        let root = temporary_root();
        let file = root.join("sample.txt");
        tokio::fs::write(&file, "test file")
            .await
            .expect("write temporary file");
        let roots = canonicalize_roots(std::slice::from_ref(&root)).expect("canonicalize root");
        let state = test_state(roots);

        let first = state
            .create_share(CreateShareRequest {
                path: file.to_string_lossy().into_owned(),
                expires_in: Some(60),
            })
            .await
            .expect("create initial share");
        let equivalent_path = root.join(".").join("sample.txt");
        let second = state
            .create_share(CreateShareRequest {
                path: equivalent_path.to_string_lossy().into_owned(),
                expires_in: Some(600),
            })
            .await
            .expect("refresh existing share");

        assert_eq!(first.id, second.id);
        assert_eq!(first.url, second.url);
        assert_eq!(first.created_at_unix, second.created_at_unix);
        assert!(second.expires_at_unix > first.expires_at_unix);
        assert_eq!(second.expires_in_seconds, 600);
        assert_eq!(state.list_shares().await.len(), 1);

        tokio::fs::remove_dir_all(root)
            .await
            .expect("remove temporary root");
    }

    #[tokio::test]
    async fn managed_share_can_be_revoked_after_file_is_deleted() {
        let root = temporary_root();
        let file = root.join("sample.txt");
        tokio::fs::write(&file, "test file")
            .await
            .expect("write temporary file");
        let state = AppState::managed(
            Url::parse("http://127.0.0.1:38751/").unwrap(),
            &generate_secret(),
        );
        let canonical = tokio::fs::canonicalize(&file).await.unwrap();
        let share = state
            .create_share(CreateShareRequest {
                path: canonical.to_string_lossy().into_owned(),
                expires_in: None,
            })
            .await
            .expect("create share for selected file");

        state.set_local_port(38752).await;
        let [updated] = &state.list_shares().await[..] else {
            panic!("Expected one active share after changing ports");
        };
        let mut expected = Url::parse(&share.url).unwrap();
        expected.set_port(Some(38752)).unwrap();
        assert_eq!(updated.url, expected.as_str());
        assert_eq!(updated.expires_at_unix, share.expires_at_unix);

        tokio::fs::remove_file(&file)
            .await
            .expect("delete shared file");
        assert!(state.revoke_share_by_path(&file).await);
        assert!(state.list_shares().await.is_empty());

        tokio::fs::remove_dir_all(root)
            .await
            .expect("remove temporary root");
    }
}
