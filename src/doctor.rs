use reqwest::Url;
use serde::Serialize;
use std::{env, path::Path};

use crate::config::{AppConfig, BackendKind};

#[derive(Debug, Serialize)]
pub struct DoctorReport {
    pub config_path: String,
    pub backend: String,
    pub ready: bool,
    pub checks: Vec<DoctorCheck>,
    pub next_steps: Vec<String>,
}

impl DoctorReport {
    pub fn new(
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
    pub fn ok(key: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            level: "ok",
            detail: detail.into(),
        }
    }

    pub fn warn(key: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            level: "warn",
            detail: detail.into(),
        }
    }

    pub fn error(key: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            level: "error",
            detail: detail.into(),
        }
    }
}

pub fn generate_report(config: &AppConfig, config_path: &Path) -> DoctorReport {
    let mut checks = vec![DoctorCheck::ok(
        "config",
        format!("設定ファイルを読み込みました: {}", config_path.display()),
    )];

    let mut next_steps = Vec::new();

    match config.backend {
        BackendKind::Fixture => {
            if let Some(fixture) = &config.fixture {
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
            let Some(cybozu) = &config.cybozu_html else {
                checks.push(DoctorCheck::error(
                    "backend",
                    "[cybozu-html] セクションがありません".to_string(),
                ));
                return DoctorReport::new(config_path, config.backend.as_str(), checks, next_steps);
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

    DoctorReport::new(config_path, config.backend.as_str(), checks, next_steps)
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

fn infer_related_url(base_url: &str, path: &str) -> Option<String> {
    let mut url = Url::parse(base_url).ok()?;
    url.set_path(path);
    url.set_query(None);
    Some(url.to_string())
}
