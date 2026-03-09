use anyhow::{Context, Result, bail};
use reqwest::{
    blocking::{Client, RequestBuilder},
    header::LOCATION,
};
use serde::Serialize;
use serde_json::Value;

use crate::{
    backend::{CalendarBackend, ListQuery},
    config::{CredentialPair, CredentialSource, CybozuHtmlConfig},
    model::{CalendarEvent, CloneOverrides, EventPatch, NewEvent},
};

pub struct CybozuHtmlBackend {
    config: CybozuHtmlConfig,
    client: Client,
}

struct ResponseSnapshot {
    status: u16,
    url: String,
    body: String,
}

struct RedirectTarget {
    status: u16,
    location: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct LoginProbeReport {
    pub base_url: String,
    pub initial_url: String,
    pub login_page_url: String,
    pub login_post_url: String,
    pub get_token_url: String,
    pub login_json_url: String,
    pub available_days_url: String,
    pub schedule_index_url: String,
    pub basic_auth_source: &'static str,
    pub office_auth_source: &'static str,
    pub initial_status: u16,
    pub get_token_status: u16,
    pub login_json_status: u16,
    pub available_days_status: u16,
    pub get_token_body_hint: String,
    pub login_json_body_hint: String,
    pub available_days_body_hint: String,
    pub initial_page_kind: &'static str,
    pub reached_login_page: bool,
    pub initial_page_title: Option<String>,
    pub initial_body_hint: String,
    pub login_page_title: Option<String>,
    pub login_body_hint: String,
    pub final_url_after_login: String,
    pub login_status: u16,
    pub login_succeeded: bool,
    pub schedule_index_status: u16,
    pub schedule_index_accessible: bool,
    pub schedule_index_title: Option<String>,
    pub schedule_body_hint: String,
    pub note: Option<String>,
}

impl CybozuHtmlBackend {
    pub fn new(config: CybozuHtmlConfig) -> Result<Self> {
        let user_agent = config
            .user_agent
            .clone()
            .unwrap_or_else(|| format!("cbzcal/{}", env!("CARGO_PKG_VERSION")));

        let client = Client::builder()
            .cookie_store(true)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(user_agent)
            .build()?;

        Ok(Self { config, client })
    }

    pub fn probe_login(config: CybozuHtmlConfig) -> Result<LoginProbeReport> {
        let backend = Self::new(config)?;
        backend.run_login_probe()
    }

    fn run_login_probe(&self) -> Result<LoginProbeReport> {
        let basic_credentials = self.config.resolve_basic_credentials()?;
        let office_credentials = self
            .config
            .resolve_office_credentials()?
            .context("Cybozu ログイン資格情報がありません")?;

        let initial = self
            .get_following_redirects(&self.config.base_url, &basic_credentials)
            .with_context(|| format!("初期 URL へ接続できません: {}", self.config.base_url))?;
        let initial_status = initial.status;
        let initial_url = initial.url;
        let initial_body = initial.body;
        let initial_page_title = extract_title(&initial_body);
        let initial_body_hint = body_hint(&initial_body);
        let reached_login_page = is_login_page(&initial_url, &initial_body);
        let initial_is_authenticated = is_authenticated_page(&initial_url, &initial_body);
        let initial_page_kind = if reached_login_page {
            "login"
        } else if initial_is_authenticated {
            "authenticated"
        } else {
            "unknown"
        };

        let login_post_url = self
            .config
            .office_login_post_url
            .clone()
            .or_else(|| infer_related_url(&self.config.base_url, "/api/auth/redirect.do"))
            .ok_or_else(|| anyhow::anyhow!("Cybozu ログイン POST 先を確定できません"))?;
        let get_token_url =
            infer_related_url(&self.config.base_url, "/api/auth/getToken.json?_lc=ja")
                .ok_or_else(|| anyhow::anyhow!("getToken URL を確定できません"))?;
        let login_json_url =
            infer_related_url(&self.config.base_url, "/api/auth/login.json?_lc=ja")
                .ok_or_else(|| anyhow::anyhow!("login.json URL を確定できません"))?;
        let available_days_url =
            infer_related_url(&self.config.base_url, "/api/auth/availableDays.json?_lc=ja")
                .ok_or_else(|| anyhow::anyhow!("availableDays URL を確定できません"))?;

        let (
            login_page_url,
            login_page_title,
            login_body_hint,
            final_url_after_login,
            get_token_status,
            login_json_status,
            available_days_status,
            get_token_body_hint,
            login_json_body_hint,
            available_days_body_hint,
            login_status,
            login_succeeded,
            note,
        ) = if reached_login_page {
            let request_token = uuid::Uuid::new_v4().to_string();
            let redirect = extract_hidden_input_value(&initial_body, "redirect")
                .or_else(|| extract_query_parameter(&initial_url, "redirect"))
                .ok_or_else(|| anyhow::anyhow!("ログイン画面の redirect 値を取得できません"))?;

            let get_token = self
                .post_json(
                    &get_token_url,
                    serde_json::json!({
                        "__REQUEST_TOKEN__": &request_token,
                    }),
                    &initial_url,
                    &basic_credentials,
                )
                .context("getToken.json に失敗しました")?;
            let api_token = extract_result_token(&get_token.body)
                .ok_or_else(|| anyhow::anyhow!("getToken.json から token を取得できません"))?;

            let login_json = self
                .post_json(
                    &login_json_url,
                    serde_json::json!({
                        "username": office_credentials.username.as_str(),
                        "password": office_credentials.password.as_str(),
                        "keepUsername": true,
                        "redirect": redirect.as_str(),
                        "__REQUEST_TOKEN__": api_token.as_str(),
                    }),
                    &initial_url,
                    &basic_credentials,
                )
                .context("login.json に失敗しました")?;

            let available_days = self
                .post_json(
                    &available_days_url,
                    serde_json::json!({
                        "__REQUEST_TOKEN__": api_token.as_str(),
                    }),
                    &initial_url,
                    &basic_credentials,
                )
                .context("availableDays.json に失敗しました")?;

            let redirect_response = self
                .post_form_once(
                    &login_post_url,
                    &[
                        ("username", office_credentials.username.as_str()),
                        ("password", office_credentials.password.as_str()),
                        ("redirect", redirect.as_str()),
                    ],
                    &basic_credentials,
                )
                .with_context(|| format!("ログイン POST に失敗しました: {login_post_url}"))?;
            let login_status = redirect_response.status;
            let (final_url_after_login, login_body_hint, login_succeeded, note) =
                if let Some(location) = redirect_response.location {
                    match self.get_following_redirects(&location, &basic_credentials) {
                        Ok(login_response) => {
                            let login_body_hint = body_hint(&login_response.body);
                            let login_succeeded =
                                is_authenticated_page(&login_response.url, &login_response.body);
                            (login_response.url, login_body_hint, login_succeeded, None)
                        }
                        Err(error) => (
                            location,
                            String::new(),
                            false,
                            Some(format!("redirect 後の追跡に失敗しました: {error}")),
                        ),
                    }
                } else {
                    (
                        login_post_url.clone(),
                        String::new(),
                        false,
                        Some("redirect.do の Location を取得できませんでした".to_string()),
                    )
                };
            let login_page_title = initial_page_title.clone();

            (
                initial_url.clone(),
                login_page_title,
                login_body_hint,
                final_url_after_login,
                get_token.status,
                login_json.status,
                available_days.status,
                body_hint(&get_token.body),
                body_hint(&login_json.body),
                body_hint(&available_days.body),
                login_status,
                login_succeeded,
                note,
            )
        } else if initial_is_authenticated {
            (
                initial_url.clone(),
                initial_page_title.clone(),
                body_hint(&initial_body),
                initial_url.clone(),
                0,
                0,
                0,
                String::new(),
                String::new(),
                String::new(),
                initial_status,
                true,
                Some("初回アクセスで認証済みページに到達しました".to_string()),
            )
        } else {
            (
                initial_url.clone(),
                initial_page_title.clone(),
                body_hint(&initial_body),
                initial_url.clone(),
                0,
                0,
                0,
                String::new(),
                String::new(),
                String::new(),
                initial_status,
                false,
                Some(
                    "初回ページが login/authenticated のどちらにも判定できませんでした".to_string(),
                ),
            )
        };

        let schedule_index_url =
            infer_related_url(&self.config.base_url, "/o/ag.cgi?page=ScheduleIndex")
                .unwrap_or_else(|| format!("{}?page=ScheduleIndex", self.config.base_url));
        let (
            schedule_index_status,
            schedule_final_url,
            schedule_index_title,
            schedule_index_accessible,
            schedule_body_hint,
        ) = if login_succeeded {
            let schedule_response = self
                .get_following_redirects(&schedule_index_url, &basic_credentials)
                .with_context(|| {
                    format!("ScheduleIndex の取得に失敗しました: {schedule_index_url}")
                })?;
            let schedule_index_status = schedule_response.status;
            let schedule_final_url = schedule_response.url;
            let schedule_body = schedule_response.body;
            let schedule_index_title = extract_title(&schedule_body);
            let schedule_index_accessible =
                is_schedule_index_page(&schedule_final_url, &schedule_body);
            let schedule_body_hint = body_hint(&schedule_body);
            (
                schedule_index_status,
                schedule_final_url,
                schedule_index_title,
                schedule_index_accessible,
                schedule_body_hint,
            )
        } else {
            (0, schedule_index_url.clone(), None, false, String::new())
        };

        Ok(LoginProbeReport {
            base_url: self.config.base_url.clone(),
            initial_url,
            login_page_url,
            login_post_url,
            get_token_url,
            login_json_url,
            available_days_url,
            schedule_index_url: schedule_final_url,
            basic_auth_source: credential_source_name(basic_credentials.as_ref()),
            office_auth_source: credential_source_name(Some(&office_credentials)),
            initial_status,
            get_token_status,
            login_json_status,
            available_days_status,
            get_token_body_hint,
            login_json_body_hint,
            available_days_body_hint,
            initial_page_kind,
            reached_login_page,
            initial_page_title,
            initial_body_hint,
            login_page_title,
            login_body_hint,
            final_url_after_login,
            login_status,
            login_succeeded,
            schedule_index_status,
            schedule_index_accessible,
            schedule_index_title,
            schedule_body_hint,
            note,
        })
    }

    fn request_with_optional_basic(
        &self,
        builder: RequestBuilder,
        credentials: &Option<CredentialPair>,
    ) -> RequestBuilder {
        if let Some(credentials) = credentials {
            builder.basic_auth(&credentials.username, Some(&credentials.password))
        } else {
            builder
        }
    }

    fn get_following_redirects(
        &self,
        url: &str,
        credentials: &Option<CredentialPair>,
    ) -> Result<ResponseSnapshot> {
        self.send_following_redirects(RequestPlan::Get(url.to_string()), credentials)
    }

    fn post_form_once(
        &self,
        url: &str,
        form: &[(&str, &str)],
        credentials: &Option<CredentialPair>,
    ) -> Result<RedirectTarget> {
        let response = self
            .request_with_optional_basic(self.client.post(url), credentials)
            .form(form)
            .send()?;
        let location = response
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| response.url().join(value).ok())
            .map(|url| url.to_string());

        Ok(RedirectTarget {
            status: response.status().as_u16(),
            location,
        })
    }

    fn post_json(
        &self,
        url: &str,
        body: Value,
        referer: &str,
        credentials: &Option<CredentialPair>,
    ) -> Result<ResponseSnapshot> {
        let response = self
            .request_with_optional_basic(self.client.post(url), credentials)
            .header("Referer", referer)
            .header("Origin", request_origin(url)?)
            .json(&body)
            .send()?;
        let status = response.status().as_u16();
        let url = response.url().to_string();
        let body = response.text()?;
        Ok(ResponseSnapshot { status, url, body })
    }

    fn send_following_redirects(
        &self,
        mut plan: RequestPlan,
        credentials: &Option<CredentialPair>,
    ) -> Result<ResponseSnapshot> {
        for _ in 0..10 {
            let response = match &plan {
                RequestPlan::Get(url) => self
                    .request_with_optional_basic(self.client.get(url), credentials)
                    .send()?,
            };

            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(|| anyhow::anyhow!("redirect location がありません"))?;
                let redirect_url = response
                    .url()
                    .join(location)
                    .map(|url| url.to_string())
                    .context("redirect URL を解決できません")?;
                plan = RequestPlan::Get(redirect_url);
                continue;
            }

            let status = response.status().as_u16();
            let url = response.url().to_string();
            let body = response.text()?;
            if is_redirect_stub_page(&body) {
                if let Some(redirect_url) = extract_js_redirect_url(&url, &body) {
                    plan = RequestPlan::Get(redirect_url);
                    continue;
                }
            }

            return Ok(ResponseSnapshot { status, url, body });
        }

        bail!("redirect が多すぎます")
    }

    fn pending_contract_error(&self, operation: &str) -> anyhow::Error {
        anyhow::anyhow!(
            "`{operation}` は未実装です。{} の HTML/フォーム契約を採取し、docs/03-development-flow.md の Phase 1 を完了してから有効化してください",
            self.config.base_url
        )
    }
}

impl CalendarBackend for CybozuHtmlBackend {
    fn name(&self) -> &'static str {
        "cybozu-html"
    }

    fn list_events(&mut self, _query: ListQuery) -> Result<Vec<CalendarEvent>> {
        bail!(self.pending_contract_error("events list"));
    }

    fn add_event(&mut self, _input: NewEvent) -> Result<CalendarEvent> {
        bail!(self.pending_contract_error("events add"));
    }

    fn update_event(&mut self, _id: &str, _patch: EventPatch) -> Result<CalendarEvent> {
        bail!(self.pending_contract_error("events update"));
    }

    fn clone_event(&mut self, _id: &str, _overrides: CloneOverrides) -> Result<CalendarEvent> {
        bail!(self.pending_contract_error("events clone"));
    }

    fn delete_event(&mut self, _id: &str) -> Result<()> {
        bail!(self.pending_contract_error("events delete"));
    }
}

fn credential_source_name(credentials: Option<&CredentialPair>) -> &'static str {
    match credentials.map(|credentials| &credentials.source) {
        Some(CredentialSource::Env { .. }) => "env",
        Some(CredentialSource::Inline) => "inline",
        None => "none",
    }
}

enum RequestPlan {
    Get(String),
}

fn extract_title(html: &str) -> Option<String> {
    let start = html.find("<title>")?;
    let end = html[start + 7..].find("</title>")?;
    Some(html[start + 7..start + 7 + end].trim().to_string())
}

fn extract_hidden_input_value(html: &str, name: &str) -> Option<String> {
    let needle = format!("name=\"{name}\"");
    let position = html.find(&needle)?;
    let suffix = &html[position..];
    let value_start = suffix.find("value=\"")? + 7;
    let value_end = suffix[value_start..].find('"')?;
    Some(suffix[value_start..value_start + value_end].to_string())
}

fn extract_query_parameter(url: &str, key: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    parsed
        .query_pairs()
        .find_map(|(candidate, value)| (candidate == key).then(|| value.into_owned()))
}

fn extract_result_token(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    value
        .get("result")?
        .get("token")?
        .as_str()
        .map(str::to_string)
}

fn is_login_page(url: &str, body: &str) -> bool {
    url.contains("/login")
        || body.contains("api/auth/redirect.do")
        || body.contains("name=\"username\"")
        || body.contains("name=\"password\"")
        || body.contains("location.replace(") && body.contains("/login?")
        || extract_title(body).as_deref() == Some("ログイン")
}

fn is_authenticated_page(url: &str, body: &str) -> bool {
    url.contains("/o/ag.cgi") && !is_login_page(url, body)
}

fn is_schedule_index_page(url: &str, body: &str) -> bool {
    url.contains("ScheduleIndex") && body.contains("スケジュール")
}

fn is_redirect_stub_page(body: &str) -> bool {
    body.contains("リダイレクト中") && body.contains("location.replace(")
}

fn infer_related_url(base_url: &str, path_and_query: &str) -> Option<String> {
    let base = reqwest::Url::parse(base_url).ok()?;
    let joined = base.join(path_and_query).ok()?;
    Some(joined.to_string())
}

fn request_origin(url: &str) -> Result<String> {
    let parsed = reqwest::Url::parse(url)?;
    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("host がありません"))?;
    Ok(format!("{}://{}", parsed.scheme(), host))
}

fn body_hint(body: &str) -> String {
    body.split_whitespace()
        .take(40)
        .collect::<Vec<_>>()
        .join(" ")
}

fn extract_js_redirect_url(base_url: &str, body: &str) -> Option<String> {
    let marker = "location.replace(";
    let start = body.find(marker)? + marker.len();
    let suffix = &body[start..];
    let quote_position = suffix.find(['"', '\''])?;
    let quote = suffix[quote_position..].chars().next()?;
    let rest = &suffix[quote_position + quote.len_utf8()..];
    let end = rest.find(quote)?;
    let relative = &rest[..end];
    infer_related_url(base_url, relative)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_title_and_hidden_redirect() {
        let html = r#"
<html>
  <head><title>ログイン</title></head>
  <body>
    <form action="/api/auth/redirect.do">
      <input type="hidden" name="redirect" value="https://example.cybozu.com/o/ag.cgi?">
    </form>
  </body>
</html>
"#;

        assert_eq!(extract_title(html).as_deref(), Some("ログイン"));
        assert_eq!(
            extract_hidden_input_value(html, "redirect").as_deref(),
            Some("https://example.cybozu.com/o/ag.cgi?")
        );
    }

    #[test]
    fn infers_schedule_index_url_from_base_url() {
        assert_eq!(
            infer_related_url(
                "https://example.cybozu.com/o/ag.cgi",
                "/o/ag.cgi?page=ScheduleIndex"
            )
            .as_deref(),
            Some("https://example.cybozu.com/o/ag.cgi?page=ScheduleIndex")
        );
    }

    #[test]
    fn extracts_javascript_redirect_url() {
        let html = r#"
<html>
  <head>
    <script>location.replace("/login?redirect=https%3A%2F%2Fexample.cybozu.com%2Fo%2Fag.cgi%3F" + location.hash );</script>
  </head>
</html>
"#;

        assert_eq!(
            extract_js_redirect_url("https://example.cybozu.com/o/ag.cgi", html).as_deref(),
            Some(
                "https://example.cybozu.com/login?redirect=https%3A%2F%2Fexample.cybozu.com%2Fo%2Fag.cgi%3F"
            )
        );
    }
}
