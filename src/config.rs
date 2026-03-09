use std::{
    env, fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use reqwest::Url;
use serde::{Deserialize, Serialize};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BackendKind {
    Fixture,
    CybozuHtml,
}

impl BackendKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Fixture => "fixture",
            Self::CybozuHtml => "cybozu-html",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub backend: BackendKind,
    pub fixture: Option<FixtureConfig>,
    #[serde(rename = "cybozu-html")]
    pub cybozu_html: Option<CybozuHtmlConfig>,
}

#[derive(Debug)]
pub struct LoadedConfig {
    pub path: PathBuf,
    pub config: AppConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialPair {
    pub username: String,
    pub password: String,
    pub source: CredentialSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialSource {
    Env {
        username_env: String,
        password_env: String,
    },
    Inline,
}

impl AppConfig {
    pub fn load(path: &Path) -> Result<Self> {
        ensure_private_config_permissions(path)?;
        let raw = fs::read_to_string(path)
            .with_context(|| format!("設定ファイルを読み込めません: {}", path.display()))?;
        let mut config: Self = parse_config(path, &raw)?;
        config.resolve_relative_paths(path);
        Ok(config)
    }

    pub fn load_with_resolution(explicit_path: Option<&Path>) -> Result<LoadedConfig> {
        let path = match explicit_path {
            Some(path) => path.to_path_buf(),
            None => discover_default_config_path()?,
        };
        let config = Self::load(&path)?;
        Ok(LoadedConfig { path, config })
    }

    fn resolve_relative_paths(&mut self, config_path: &Path) {
        let base_dir = config_path.parent().unwrap_or_else(|| Path::new("."));

        if let Some(fixture) = &mut self.fixture
            && !fixture.path.is_absolute()
        {
            fixture.path = normalize_path(&base_dir.join(&fixture.path));
        }

        if let Some(cybozu) = &mut self.cybozu_html
            && let Some(path) = &cybozu.session_cache_path
            && !path.is_absolute()
        {
            cybozu.session_cache_path = Some(normalize_path(&base_dir.join(path)));
        }
    }

    pub fn doctor_report(&self, config_path: &Path) -> DoctorReport {
        let mut checks = vec![DoctorCheck::ok(
            "config",
            format!("設定ファイルを読み込みました: {}", config_path.display()),
        )];

        let mut next_steps = Vec::new();

        match self.backend {
            BackendKind::Fixture => {
                if let Some(fixture) = &self.fixture {
                    checks.push(DoctorCheck::ok(
                        "backend",
                        format!(
                            "fixture バックエンドを使用します: {}",
                            fixture.path.display()
                        ),
                    ));
                } else {
                    checks.push(DoctorCheck::error(
                        "backend",
                        "[fixture] セクションがありません".to_string(),
                    ));
                }
            }
            BackendKind::CybozuHtml => {
                let Some(cybozu) = &self.cybozu_html else {
                    checks.push(DoctorCheck::error(
                        "backend",
                        "[cybozu-html] セクションがありません".to_string(),
                    ));
                    return DoctorReport::new(
                        config_path,
                        self.backend.as_str(),
                        checks,
                        next_steps,
                    );
                };

                match Url::parse(&cybozu.base_url) {
                    Ok(url) => {
                        let detail = if url.path().ends_with("/o/ag.cgi") {
                            format!("対象 URL を確認しました: {}", cybozu.base_url)
                        } else {
                            format!(
                                "URL は解釈できましたが、末尾が `/o/ag.cgi` ではありません: {}",
                                cybozu.base_url
                            )
                        };

                        if url.path().ends_with("/o/ag.cgi") {
                            checks.push(DoctorCheck::ok("base-url", detail));
                        } else {
                            checks.push(DoctorCheck::warn("base-url", detail));
                        }
                    }
                    Err(error) => checks.push(DoctorCheck::error(
                        "base-url",
                        format!("URL を解釈できません: {error}"),
                    )),
                }

                let inferred_login_url = infer_related_url(&cybozu.base_url, "/login");
                push_optional_url_check(
                    &mut checks,
                    "office-login-url",
                    "Cybozu ログイン画面 URL",
                    cybozu.office_login_url.as_deref(),
                    inferred_login_url.as_deref(),
                );

                let inferred_login_post_url =
                    infer_related_url(&cybozu.base_url, "/api/auth/redirect.do");
                push_optional_url_check(
                    &mut checks,
                    "office-login-post-url",
                    "Cybozu ログイン POST 先",
                    cybozu.office_login_post_url.as_deref(),
                    inferred_login_post_url.as_deref(),
                );
                checks.push(DoctorCheck::ok(
                    "session-cache",
                    format!(
                        "セッション Cookie キャッシュを使用します: {}",
                        cybozu.session_cache_path().display()
                    ),
                ));

                push_env_pair_checks(
                    &mut checks,
                    "basic-auth",
                    "Basic 認証",
                    cybozu.basic_username_env.as_deref(),
                    cybozu.basic_password_env.as_deref(),
                    cybozu.basic_username.as_deref(),
                    cybozu.basic_password.as_deref(),
                );
                push_env_pair_checks(
                    &mut checks,
                    "office-login",
                    "Cybozu ログイン",
                    cybozu.office_username_env.as_deref(),
                    cybozu.office_password_env.as_deref(),
                    cybozu.office_username.as_deref(),
                    cybozu.office_password.as_deref(),
                );

                checks.push(DoctorCheck::warn(
                    "html-contract",
                    "cybozu-html バックエンドは `events list`、通常予定の `events add` / `events clone`、通常予定と繰り返し予定の `events update` / `events delete` まで確認済みです。残りの拡張は参加者・設備・複数日・繰り返し clone です。".to_string(),
                ));

                next_steps.push(
                    "docs/03-development-flow.md の Phase 1 に従って画面契約を採取する".to_string(),
                );
                next_steps.push(
                    "採取したフォーム項目・hidden 値・遷移を docs/01-research.md に追記する"
                        .to_string(),
                );
                next_steps.push(
                    "採取結果をもとに src/backend/cybozu_html.rs の add/update/clone/delete 拡張を埋める".to_string(),
                );
            }
        }

        DoctorReport::new(config_path, self.backend.as_str(), checks, next_steps)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct FixtureConfig {
    pub path: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CybozuHtmlConfig {
    pub base_url: String,
    pub office_login_url: Option<String>,
    pub office_login_post_url: Option<String>,
    pub session_cache_path: Option<PathBuf>,
    pub basic_username_env: Option<String>,
    pub basic_password_env: Option<String>,
    pub basic_username: Option<String>,
    pub basic_password: Option<String>,
    pub office_username_env: Option<String>,
    pub office_password_env: Option<String>,
    pub office_username: Option<String>,
    pub office_password: Option<String>,
    pub user_agent: Option<String>,
}

impl CybozuHtmlConfig {
    pub fn session_cache_path(&self) -> PathBuf {
        self.session_cache_path
            .clone()
            .unwrap_or_else(default_session_cache_path)
    }

    pub fn resolve_basic_credentials(&self) -> Result<Option<CredentialPair>> {
        resolve_credential_pair(
            self.basic_username_env.as_deref(),
            self.basic_password_env.as_deref(),
            self.basic_username.as_deref(),
            self.basic_password.as_deref(),
        )
    }

    pub fn resolve_office_credentials(&self) -> Result<Option<CredentialPair>> {
        resolve_credential_pair(
            self.office_username_env.as_deref(),
            self.office_password_env.as_deref(),
            self.office_username.as_deref(),
            self.office_password.as_deref(),
        )
    }
}

#[derive(Debug, Serialize)]
pub struct DoctorReport {
    pub config_path: String,
    pub backend: String,
    pub ready: bool,
    pub checks: Vec<DoctorCheck>,
    pub next_steps: Vec<String>,
}

impl DoctorReport {
    fn new(
        config_path: &Path,
        backend: &str,
        checks: Vec<DoctorCheck>,
        next_steps: Vec<String>,
    ) -> Self {
        let ready = checks.iter().all(|check| check.level != "error");

        Self {
            config_path: config_path.display().to_string(),
            backend: backend.to_string(),
            ready,
            checks,
            next_steps,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct DoctorCheck {
    pub key: String,
    pub level: &'static str,
    pub detail: String,
}

impl DoctorCheck {
    fn ok(key: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            level: "ok",
            detail: detail.into(),
        }
    }

    fn warn(key: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            level: "warn",
            detail: detail.into(),
        }
    }

    fn error(key: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            level: "error",
            detail: detail.into(),
        }
    }
}

fn push_env_pair_checks(
    checks: &mut Vec<DoctorCheck>,
    key: &str,
    label: &str,
    username_env: Option<&str>,
    password_env: Option<&str>,
    inline_username: Option<&str>,
    inline_password: Option<&str>,
) {
    let inline_ready = matches!((inline_username, inline_password), (Some(_), Some(_)));

    match (username_env, password_env) {
        (Some(username_env), Some(password_env)) => {
            let username_exists = env::var_os(username_env).is_some();
            let password_exists = env::var_os(password_env).is_some();

            match (username_exists, password_exists) {
                (true, true) => checks.push(DoctorCheck::ok(
                    key,
                    format!(
                        "{label} は環境変数 {username_env} / {password_env} から利用可能です"
                    ),
                )),
                (false, false) if inline_ready => checks.push(DoctorCheck::warn(
                    key,
                    format!(
                        "{label} の環境変数 {username_env} / {password_env} は未設定ですが、設定ファイル内の資格情報を fallback として使えます"
                    ),
                )),
                (false, false) => checks.push(DoctorCheck::error(
                    key,
                    format!(
                        "{label} の環境変数 {username_env} / {password_env} が見つかりません"
                    ),
                )),
                _ if inline_ready => checks.push(DoctorCheck::warn(
                    key,
                    format!(
                        "{label} の環境変数 {username_env} / {password_env} は片方だけ設定されています。今回は設定ファイル内の資格情報を fallback として使えます"
                    ),
                )),
                _ => checks.push(DoctorCheck::error(
                    key,
                    format!(
                        "{label} の環境変数 {username_env} / {password_env} はどちらか一方しか設定されていません"
                    ),
                )),
            }
        }
        (Some(_), None) | (None, Some(_)) if inline_ready => checks.push(DoctorCheck::warn(
            key,
            format!(
                "{label} の環境変数名は片方だけ設定されています。今回は設定ファイル内の資格情報を fallback として使えます"
            ),
        )),
        (Some(_), None) | (None, Some(_)) => checks.push(DoctorCheck::error(
            key,
            format!("{label} はユーザー名とパスワードの環境変数名を両方設定してください"),
        )),
        (None, None) => match (inline_username, inline_password) {
            (Some(_), Some(_)) => checks.push(DoctorCheck::warn(
                key,
                format!(
                    "{label} は設定ファイル内の平文資格情報を使います。一時検証向けです"
                ),
            )),
            (Some(_), None) | (None, Some(_)) => checks.push(DoctorCheck::error(
                key,
                format!(
                    "{label} は設定ファイル内でもユーザー名とパスワードを両方設定してください"
                ),
            )),
            (None, None) => checks.push(DoctorCheck::warn(
                key,
                format!(
                    "{label} は未設定です。必要なら環境変数名または設定ファイル内の資格情報を両方設定してください"
                ),
            )),
        },
    }
}

fn push_optional_url_check(
    checks: &mut Vec<DoctorCheck>,
    key: &str,
    label: &str,
    configured_url: Option<&str>,
    inferred_url: Option<&str>,
) {
    match configured_url {
        Some(url) => match Url::parse(url) {
            Ok(_) => {
                let detail = match inferred_url {
                    Some(inferred) if inferred != url => {
                        format!("{label} を確認しました: {url} (推定値: {inferred})")
                    }
                    _ => format!("{label} を確認しました: {url}"),
                };
                checks.push(DoctorCheck::ok(key, detail));
            }
            Err(error) => checks.push(DoctorCheck::error(
                key,
                format!("{label} を解釈できません: {error}"),
            )),
        },
        None => {
            let detail = match inferred_url {
                Some(url) => format!(
                    "{label} は未設定です。観測上は {url} を使う見込みですが、設定に明示しておく方が安全です"
                ),
                None => format!(
                    "{label} は未設定です。base_url から推定できない場合は設定に明示してください"
                ),
            };
            checks.push(DoctorCheck::warn(key, detail));
        }
    }
}

fn resolve_credential_pair(
    username_env: Option<&str>,
    password_env: Option<&str>,
    inline_username: Option<&str>,
    inline_password: Option<&str>,
) -> Result<Option<CredentialPair>> {
    match (username_env, password_env) {
        (Some(username_env), Some(password_env)) => {
            let username = env::var(username_env).ok();
            let password = env::var(password_env).ok();

            match (username, password) {
                (Some(username), Some(password)) => Ok(Some(CredentialPair {
                    username,
                    password,
                    source: CredentialSource::Env {
                        username_env: username_env.to_string(),
                        password_env: password_env.to_string(),
                    },
                })),
                (Some(_), None) | (None, Some(_)) => {
                    if let (Some(inline_username), Some(inline_password)) =
                        (inline_username, inline_password)
                    {
                        Ok(Some(CredentialPair {
                            username: inline_username.to_string(),
                            password: inline_password.to_string(),
                            source: CredentialSource::Inline,
                        }))
                    } else {
                        bail!(
                            "環境変数 {username_env} / {password_env} はどちらか一方しか設定されていません"
                        )
                    }
                }
                (None, None) => match (inline_username, inline_password) {
                    (Some(inline_username), Some(inline_password)) => Ok(Some(CredentialPair {
                        username: inline_username.to_string(),
                        password: inline_password.to_string(),
                        source: CredentialSource::Inline,
                    })),
                    (Some(_), None) | (None, Some(_)) => {
                        bail!(
                            "設定ファイル内の資格情報はユーザー名とパスワードを両方指定してください"
                        )
                    }
                    (None, None) => Ok(None),
                },
            }
        }
        (Some(_), None) | (None, Some(_)) => match (inline_username, inline_password) {
            (Some(inline_username), Some(inline_password)) => Ok(Some(CredentialPair {
                username: inline_username.to_string(),
                password: inline_password.to_string(),
                source: CredentialSource::Inline,
            })),
            _ => bail!("環境変数名はユーザー名とパスワードを両方指定してください"),
        },
        (None, None) => match (inline_username, inline_password) {
            (Some(inline_username), Some(inline_password)) => Ok(Some(CredentialPair {
                username: inline_username.to_string(),
                password: inline_password.to_string(),
                source: CredentialSource::Inline,
            })),
            (Some(_), None) | (None, Some(_)) => {
                bail!("設定ファイル内の資格情報はユーザー名とパスワードを両方指定してください")
            }
            (None, None) => Ok(None),
        },
    }
}

fn infer_related_url(base_url: &str, path: &str) -> Option<String> {
    let mut url = Url::parse(base_url).ok()?;
    url.set_path(path);
    url.set_query(None);
    Some(url.to_string())
}

fn parse_config(path: &Path, raw: &str) -> Result<AppConfig> {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("yml") | Some("yaml") => serde_yaml::from_str(raw)
            .with_context(|| format!("設定ファイルを解釈できません: {}", path.display())),
        Some("toml") => toml::from_str(raw)
            .with_context(|| format!("設定ファイルを解釈できません: {}", path.display())),
        _ => bail!(
            "設定ファイル形式を判別できません: {} (.yml/.yaml/.toml を使ってください)",
            path.display()
        ),
    }
}

fn discover_default_config_path() -> Result<PathBuf> {
    let candidates = config_search_paths()?;

    for path in &candidates {
        if path.is_file() {
            return Ok(path.clone());
        }
    }

    let joined = candidates
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    bail!(
        "設定ファイルが見つかりませんでした。探索先: {joined}. `--config` を指定するか .cbzcal.yml / .cbzcal.toml を作成してください"
    )
}

fn config_search_paths() -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    paths.push(env::current_dir()?.join(".cbzcal.toml"));
    paths.push(env::current_dir()?.join(".cbzcal.yml"));

    if let Some(xdg_config_home) = xdg_config_home() {
        paths.push(xdg_config_home.join("cbzcal").join("config.toml"));
        paths.push(xdg_config_home.join("cbzcal").join("config.yml"));
    }

    if let Some(home_dir) = home_dir() {
        paths.push(home_dir.join(".cbzcal.toml"));
        paths.push(home_dir.join(".cbzcal.yml"));
    }

    Ok(paths)
}

fn xdg_config_home() -> Option<PathBuf> {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|home| home.join(".config")))
}

fn xdg_state_home() -> Option<PathBuf> {
    env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|home| home.join(".local").join("state")))
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}

fn default_session_cache_path() -> PathBuf {
    xdg_state_home()
        .or_else(home_dir)
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join("cbzcal")
        .join("session-cookies.json")
}

#[cfg(unix)]
fn ensure_private_config_permissions(path: &Path) -> Result<()> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("設定ファイルの情報を取得できません: {}", path.display()))?;
    let mode = metadata.permissions().mode() & 0o777;

    if mode == 0o400 || mode == 0o600 {
        return Ok(());
    }

    bail!(
        "設定ファイル {} の権限は 0400 または 0600 である必要があります: 現在 {:o}",
        path.display(),
        mode
    )
}

#[cfg(not(unix))]
fn ensure_private_config_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }

    normalized
}

#[cfg(test)]
mod tests {
    use std::fs;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[cfg(unix)]
    fn chmod(path: &Path, mode: u32) {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("chmod");
    }

    #[test]
    fn relative_fixture_path_is_resolved_against_config_directory() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let config_path = tempdir.path().join("config").join("cbzcal.toml");
        fs::create_dir_all(config_path.parent().expect("parent")).expect("mkdir");
        fs::write(
            &config_path,
            r#"
backend = "fixture"

[fixture]
path = "../fixtures/local.json"
"#,
        )
        .expect("write config");
        #[cfg(unix)]
        chmod(&config_path, 0o600);

        let config = AppConfig::load(&config_path).expect("load config");
        let fixture_path = &config.fixture.expect("fixture").path;

        assert!(fixture_path.is_absolute());
        assert_eq!(
            fixture_path,
            &tempdir.path().join("fixtures").join("local.json")
        );
    }

    #[test]
    fn cybozu_html_optional_login_urls_are_loaded() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let config_path = tempdir.path().join("cbzcal.toml");
        fs::write(
            &config_path,
            r#"
backend = "cybozu-html"

[cybozu-html]
base_url = "https://example.cybozu.com/o/ag.cgi"
office_login_url = "https://example.cybozu.com/login"
office_login_post_url = "https://example.cybozu.com/api/auth/redirect.do"
"#,
        )
        .expect("write config");
        #[cfg(unix)]
        chmod(&config_path, 0o600);

        let config = AppConfig::load(&config_path).expect("load config");
        let cybozu = config.cybozu_html.expect("cybozu-html");

        assert_eq!(
            cybozu.office_login_url.as_deref(),
            Some("https://example.cybozu.com/login")
        );
        assert_eq!(
            cybozu.office_login_post_url.as_deref(),
            Some("https://example.cybozu.com/api/auth/redirect.do")
        );
    }

    #[test]
    fn cybozu_html_relative_session_cache_path_is_resolved() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let config_path = tempdir.path().join("config").join("cbzcal.toml");
        fs::create_dir_all(config_path.parent().expect("parent")).expect("mkdir");
        fs::write(
            &config_path,
            r#"
backend = "cybozu-html"

[cybozu-html]
base_url = "https://example.cybozu.com/o/ag.cgi"
session_cache_path = "../state/cookies.json"
"#,
        )
        .expect("write config");
        #[cfg(unix)]
        chmod(&config_path, 0o600);

        let config = AppConfig::load(&config_path).expect("load config");
        let cybozu = config.cybozu_html.expect("cybozu-html");
        assert_eq!(
            cybozu.session_cache_path(),
            tempdir.path().join("state").join("cookies.json")
        );
    }

    #[test]
    fn cybozu_html_default_session_cache_path_uses_xdg_state_home() {
        let home = env::var_os("HOME");
        let xdg_state = env::var_os("XDG_STATE_HOME");
        let root = tempfile::tempdir().expect("tempdir");

        unsafe {
            env::set_var("HOME", root.path().join("home"));
            env::set_var("XDG_STATE_HOME", root.path().join("state"));
        }

        let path = default_session_cache_path();

        if let Some(value) = home {
            unsafe { env::set_var("HOME", value) };
        } else {
            unsafe { env::remove_var("HOME") };
        }
        if let Some(value) = xdg_state {
            unsafe { env::set_var("XDG_STATE_HOME", value) };
        } else {
            unsafe { env::remove_var("XDG_STATE_HOME") };
        }

        assert_eq!(
            path,
            root.path()
                .join("state")
                .join("cbzcal")
                .join("session-cookies.json")
        );
    }

    #[test]
    fn doctor_warns_when_login_endpoints_are_not_explicitly_configured() {
        let config = AppConfig {
            backend: BackendKind::CybozuHtml,
            fixture: None,
            cybozu_html: Some(CybozuHtmlConfig {
                base_url: "https://example.cybozu.com/o/ag.cgi".to_string(),
                office_login_url: None,
                office_login_post_url: None,
                session_cache_path: None,
                basic_username_env: None,
                basic_password_env: None,
                basic_username: None,
                basic_password: None,
                office_username_env: None,
                office_password_env: None,
                office_username: None,
                office_password: None,
                user_agent: None,
            }),
        };

        let report = config.doctor_report(Path::new("/tmp/cbzcal.toml"));
        let login_url_check = report
            .checks
            .iter()
            .find(|check| check.key == "office-login-url")
            .expect("office-login-url check");
        let login_post_check = report
            .checks
            .iter()
            .find(|check| check.key == "office-login-post-url")
            .expect("office-login-post-url check");

        assert_eq!(login_url_check.level, "warn");
        assert!(
            login_url_check
                .detail
                .contains("https://example.cybozu.com/login")
        );
        assert_eq!(login_post_check.level, "warn");
        assert!(
            login_post_check
                .detail
                .contains("https://example.cybozu.com/api/auth/redirect.do")
        );
    }

    #[test]
    fn yaml_config_is_supported() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let config_path = tempdir.path().join(".cbzcal.yml");
        fs::write(
            &config_path,
            format!(
                r#"
backend: fixture
fixture:
  path: "{}"
"#,
                tempdir.path().join("calendar.json").display()
            ),
        )
        .expect("write config");
        #[cfg(unix)]
        chmod(&config_path, 0o600);

        let config = AppConfig::load(&config_path).expect("load config");
        assert!(matches!(config.backend, BackendKind::Fixture));
    }

    #[cfg(unix)]
    #[test]
    fn insecure_permissions_are_rejected() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let config_path = tempdir.path().join(".cbzcal.yml");
        fs::write(
            &config_path,
            r#"
backend: fixture
fixture:
  path: "fixtures/sample-calendar.json"
"#,
        )
        .expect("write config");
        chmod(&config_path, 0o644);

        let error = AppConfig::load(&config_path).expect_err("permission error");
        assert!(error.to_string().contains("0400 または 0600"));
    }

    #[test]
    fn inline_credentials_are_resolved() {
        let config = CybozuHtmlConfig {
            base_url: "https://example.cybozu.com/o/ag.cgi".to_string(),
            office_login_url: None,
            office_login_post_url: None,
            session_cache_path: None,
            basic_username_env: None,
            basic_password_env: None,
            basic_username: Some("basic-user".to_string()),
            basic_password: Some("basic-pass".to_string()),
            office_username_env: None,
            office_password_env: None,
            office_username: Some("office-user".to_string()),
            office_password: Some("office-pass".to_string()),
            user_agent: None,
        };

        let basic = config
            .resolve_basic_credentials()
            .expect("resolve basic")
            .expect("basic creds");
        let office = config
            .resolve_office_credentials()
            .expect("resolve office")
            .expect("office creds");

        assert_eq!(basic.username, "basic-user");
        assert_eq!(basic.password, "basic-pass");
        assert_eq!(basic.source, CredentialSource::Inline);
        assert_eq!(office.username, "office-user");
        assert_eq!(office.password, "office-pass");
        assert_eq!(office.source, CredentialSource::Inline);
    }

    #[test]
    fn doctor_warns_when_using_inline_credentials() {
        let config = AppConfig {
            backend: BackendKind::CybozuHtml,
            fixture: None,
            cybozu_html: Some(CybozuHtmlConfig {
                base_url: "https://example.cybozu.com/o/ag.cgi".to_string(),
                office_login_url: None,
                office_login_post_url: None,
                session_cache_path: None,
                basic_username_env: None,
                basic_password_env: None,
                basic_username: Some("basic-user".to_string()),
                basic_password: Some("basic-pass".to_string()),
                office_username_env: None,
                office_password_env: None,
                office_username: Some("office-user".to_string()),
                office_password: Some("office-pass".to_string()),
                user_agent: None,
            }),
        };

        let report = config.doctor_report(Path::new("/tmp/.cbzcal.toml"));
        let basic = report
            .checks
            .iter()
            .find(|check| check.key == "basic-auth")
            .expect("basic check");
        let office = report
            .checks
            .iter()
            .find(|check| check.key == "office-login")
            .expect("office check");

        assert_eq!(basic.level, "warn");
        assert!(basic.detail.contains("平文資格情報"));
        assert_eq!(office.level, "warn");
        assert!(office.detail.contains("平文資格情報"));
    }
}
