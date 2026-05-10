use std::{
    fs,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

pub const GITHUB_CLIENT_ID: &str = "Ov23li8JAOc6Bms43PVH";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GithubAuthState {
    pub token: Option<String>,
    pub login: Option<String>,
    pub message: Option<String>,
}

impl GithubAuthState {
    pub fn is_authenticated(&self) -> bool {
        self.token
            .as_deref()
            .is_some_and(|token| !token.trim().is_empty())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AccessTokenResponse {
    pub access_token: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct GithubUserResponse {
    login: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredToken {
    token: String,
    login: Option<String>,
}

pub fn token_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("gitoui")
        .join("github_token.toml")
}

pub fn load_state() -> GithubAuthState {
    load_state_from_path(token_path())
}

pub fn load_state_from_path(path: PathBuf) -> GithubAuthState {
    let Ok(content) = fs::read_to_string(path) else {
        return GithubAuthState::default();
    };
    let Ok(stored) = toml::from_str::<StoredToken>(&content) else {
        return GithubAuthState::default();
    };
    GithubAuthState {
        token: Some(stored.token),
        login: stored.login,
        message: None,
    }
}

pub fn save_state(state: &GithubAuthState) -> Result<(), String> {
    let token = state.token.clone().ok_or("No GitHub token to save")?;
    save_state_to_path(token_path(), token, state.login.clone())
}

pub fn save_state_to_path(
    path: PathBuf,
    token: String,
    login: Option<String>,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create GitHub auth directory: {e}"))?;
    }
    let content = toml::to_string_pretty(&StoredToken { token, login })
        .map_err(|e| format!("Failed to serialize GitHub token: {e}"))?;
    fs::write(&path, content).map_err(|e| format!("Failed to write GitHub token: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

pub fn clear_state() -> Result<(), String> {
    let path = token_path();
    if path.exists() {
        fs::remove_file(path).map_err(|e| format!("Failed to delete GitHub token: {e}"))?;
    }
    Ok(())
}

pub fn request_device_code() -> Result<DeviceCodeResponse, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| format!("Failed to initialize GitHub client: {e}"))?;
    let response = client
        .post("https://github.com/login/device/code")
        .header("Accept", "application/json")
        .header("User-Agent", "gitoui")
        .form(&[("client_id", GITHUB_CLIENT_ID), ("scope", "read:user repo")])
        .send()
        .map_err(|e| format!("Failed to request GitHub device code: {e}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "GitHub device code request failed: {}",
            response.status()
        ));
    }
    let body = response
        .text()
        .map_err(|e| format!("Failed to read GitHub response: {e}"))?;
    serde_json::from_str(&body).map_err(|e| format!("Failed to parse GitHub device code: {e}"))
}

pub fn poll_for_token(device: &DeviceCodeResponse) -> Result<GithubAuthState, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| format!("Failed to initialize GitHub client: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(device.expires_in);
    let mut interval = Duration::from_secs(device.interval.unwrap_or(5).max(1));

    while Instant::now() < deadline {
        thread::sleep(interval);
        let response = client
            .post("https://github.com/login/oauth/access_token")
            .header("Accept", "application/json")
            .header("User-Agent", "gitoui")
            .form(&[
                ("client_id", GITHUB_CLIENT_ID),
                ("device_code", device.device_code.as_str()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .map_err(|e| format!("Failed to poll GitHub auth: {e}"))?;
        if !response.status().is_success() {
            return Err(format!("GitHub auth polling failed: {}", response.status()));
        }
        let body = response
            .text()
            .map_err(|e| format!("Failed to read GitHub auth response: {e}"))?;
        let parsed: AccessTokenResponse = serde_json::from_str(&body)
            .map_err(|e| format!("Failed to parse GitHub auth response: {e}"))?;
        if let Some(token) = parsed.access_token {
            let login = fetch_login(&token)?;
            let state = GithubAuthState {
                token: Some(token),
                login: Some(login),
                message: Some("GitHub authentication succeeded".into()),
            };
            save_state(&state)?;
            return Ok(state);
        }
        match parsed.error.as_deref() {
            Some("authorization_pending") => {}
            Some("slow_down") => interval += Duration::from_secs(5),
            Some("expired_token") => return Err("GitHub authentication expired".into()),
            Some("access_denied") => return Err("GitHub authentication denied".into()),
            Some(error) => {
                return Err(parsed
                    .error_description
                    .unwrap_or_else(|| format!("GitHub auth failed: {error}")));
            }
            None => return Err("GitHub auth response did not contain a token".into()),
        }
    }

    Err("GitHub authentication expired".into())
}

pub fn fetch_login(token: &str) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| format!("Failed to initialize GitHub client: {e}"))?;
    let response = client
        .get("https://api.github.com/user")
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "gitoui")
        .bearer_auth(token)
        .send()
        .map_err(|e| format!("Failed to fetch GitHub user: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("GitHub user request failed: {}", response.status()));
    }
    let body = response
        .text()
        .map_err(|e| format!("Failed to read GitHub user response: {e}"))?;
    let user: GithubUserResponse = serde_json::from_str(&body)
        .map_err(|e| format!("Failed to parse GitHub user response: {e}"))?;
    Ok(user.login)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_state_is_authenticated_only_with_non_empty_token() {
        assert!(!GithubAuthState::default().is_authenticated());
        assert!(!GithubAuthState {
            token: Some("   ".into()),
            login: None,
            message: None,
        }
        .is_authenticated());
        assert!(GithubAuthState {
            token: Some("token".into()),
            login: Some("octocat".into()),
            message: None,
        }
        .is_authenticated());
    }

    #[test]
    fn parses_device_code_response() {
        let response: DeviceCodeResponse = serde_json::from_str(
            r#"{
                "device_code":"device",
                "user_code":"ABCD-EFGH",
                "verification_uri":"https://github.com/login/device",
                "expires_in":900,
                "interval":5
            }"#,
        )
        .unwrap();

        assert_eq!(response.device_code, "device");
        assert_eq!(response.user_code, "ABCD-EFGH");
        assert_eq!(response.verification_uri, "https://github.com/login/device");
        assert_eq!(response.expires_in, 900);
        assert_eq!(response.interval, Some(5));
    }

    #[test]
    fn parses_access_token_response() {
        let response: AccessTokenResponse = serde_json::from_str(
            r#"{"access_token":"token","token_type":"bearer","scope":"read:user,repo"}"#,
        )
        .unwrap();

        assert_eq!(response.access_token.as_deref(), Some("token"));
        assert_eq!(response.error, None);
    }

    #[test]
    fn token_file_round_trips() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("github_token.toml");

        save_state_to_path(path.clone(), "token".into(), Some("octocat".into())).unwrap();
        let state = load_state_from_path(path);

        assert_eq!(state.token.as_deref(), Some("token"));
        assert_eq!(state.login.as_deref(), Some("octocat"));
        assert!(state.is_authenticated());
    }
}
