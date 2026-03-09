use std::collections::HashSet;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Datelike, Days, FixedOffset, NaiveDate, TimeZone, Utc};
use reqwest::{
    Url,
    blocking::{Client, RequestBuilder},
    header::LOCATION,
};
use scraper::{Html, Selector};
use serde::Serialize;
use serde_json::Value;

use crate::{
    backend::{CalendarBackend, ListQuery},
    config::{CredentialPair, CredentialSource, CybozuHtmlConfig},
    model::{CalendarEvent, CloneOverrides, EventPatch, NewEvent},
};

const JST_OFFSET_SECONDS: i32 = 9 * 60 * 60;

pub struct CybozuHtmlBackend {
    config: CybozuHtmlConfig,
    client: Client,
}

#[derive(Clone)]
struct ResponseSnapshot {
    status: u16,
    url: String,
    body: String,
}

struct RedirectTarget {
    status: u16,
    location: Option<String>,
}

struct LoginExecutionReport {
    get_token_status: u16,
    login_json_status: u16,
    available_days_status: u16,
    get_token_body_hint: String,
    login_json_body_hint: String,
    available_days_body_hint: String,
    login_status: u16,
    final_response: ResponseSnapshot,
    note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScheduleViewIdentity {
    uid: String,
    gid: String,
    date: String,
    bdate: String,
    seid: String,
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
        let initial_url = initial.url.clone();
        let initial_body = initial.body.clone();
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
        let login_post_url = self.login_post_url()?;
        let get_token_url = self.get_token_url()?;
        let login_json_url = self.login_json_url()?;
        let available_days_url = self.available_days_url()?;

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
            let login = self
                .execute_office_login(&initial, &basic_credentials, &office_credentials)
                .context("Cybozu ログインシーケンスに失敗しました")?;
            let login_page_title = initial_page_title.clone();
            let final_url_after_login = login.final_response.url.clone();
            let login_body_hint = body_hint(&login.final_response.body);
            let login_succeeded =
                is_authenticated_page(&login.final_response.url, &login.final_response.body);

            (
                initial_url.clone(),
                login_page_title,
                login_body_hint,
                final_url_after_login,
                login.get_token_status,
                login.login_json_status,
                login.available_days_status,
                login.get_token_body_hint,
                login.login_json_body_hint,
                login.available_days_body_hint,
                login.login_status,
                login_succeeded,
                login.note,
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

    fn execute_office_login(
        &self,
        initial: &ResponseSnapshot,
        basic_credentials: &Option<CredentialPair>,
        office_credentials: &CredentialPair,
    ) -> Result<LoginExecutionReport> {
        let login_post_url = self.login_post_url()?;
        let get_token_url = self.get_token_url()?;
        let login_json_url = self.login_json_url()?;
        let available_days_url = self.available_days_url()?;
        let request_token = uuid::Uuid::new_v4().to_string();
        let redirect = extract_hidden_input_value(&initial.body, "redirect")
            .or_else(|| extract_query_parameter(&initial.url, "redirect"))
            .ok_or_else(|| anyhow::anyhow!("ログイン画面の redirect 値を取得できません"))?;

        let get_token = self
            .post_json(
                &get_token_url,
                serde_json::json!({
                    "__REQUEST_TOKEN__": &request_token,
                }),
                &initial.url,
                basic_credentials,
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
                &initial.url,
                basic_credentials,
            )
            .context("login.json に失敗しました")?;

        let available_days = self
            .post_json(
                &available_days_url,
                serde_json::json!({
                    "__REQUEST_TOKEN__": api_token.as_str(),
                }),
                &initial.url,
                basic_credentials,
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
                basic_credentials,
            )
            .with_context(|| format!("ログイン POST に失敗しました: {login_post_url}"))?;
        let login_status = redirect_response.status;
        let final_response = if let Some(location) = redirect_response.location {
            self.get_following_redirects(&location, basic_credentials)
                .with_context(|| format!("redirect 後の追跡に失敗しました: {location}"))?
        } else {
            bail!("redirect.do の Location を取得できませんでした");
        };

        Ok(LoginExecutionReport {
            get_token_status: get_token.status,
            login_json_status: login_json.status,
            available_days_status: available_days.status,
            get_token_body_hint: body_hint(&get_token.body),
            login_json_body_hint: body_hint(&login_json.body),
            available_days_body_hint: body_hint(&available_days.body),
            login_status,
            final_response,
            note: None,
        })
    }

    fn authenticated_schedule_index(
        &self,
        query: &ListQuery,
    ) -> Result<(Option<CredentialPair>, Vec<ResponseSnapshot>)> {
        let basic_credentials = self.config.resolve_basic_credentials()?;
        let office_credentials = self
            .config
            .resolve_office_credentials()?
            .context("Cybozu ログイン資格情報がありません")?;
        let initial = self
            .get_following_redirects(&self.config.base_url, &basic_credentials)
            .with_context(|| format!("初期 URL へ接続できません: {}", self.config.base_url))?;

        let first_schedule_index = if is_login_page(&initial.url, &initial.body) {
            let login = self
                .execute_office_login(&initial, &basic_credentials, &office_credentials)
                .context("Cybozu ログインシーケンスに失敗しました")?;
            self.fetch_schedule_index(
                &basic_credentials,
                None,
                extract_schedule_index_gid(&login.final_response.body, &login.final_response.url)
                    .as_deref(),
            )?
        } else if is_schedule_index_page(&initial.url, &initial.body) {
            initial
        } else if is_authenticated_page(&initial.url, &initial.body) {
            self.fetch_schedule_index(&basic_credentials, None, None)?
        } else {
            bail!("初回ページが login/authenticated のどちらにも判定できませんでした");
        };

        let default_week =
            extract_schedule_anchor_date(&first_schedule_index.body, &first_schedule_index.url)
                .unwrap_or_else(|| week_start(today_jst()));
        let gid = extract_schedule_index_gid(&first_schedule_index.body, &first_schedule_index.url);
        let target_weeks = list_target_weeks(query, default_week);

        let mut pages = Vec::new();
        let mut seen_weeks = HashSet::new();
        for week in target_weeks {
            if !seen_weeks.insert(week) {
                continue;
            }
            if week == default_week {
                pages.push(first_schedule_index.clone());
                continue;
            }

            pages.push(self.fetch_schedule_index(
                &basic_credentials,
                Some(week),
                gid.as_deref(),
            )?);
        }

        Ok((basic_credentials, pages))
    }

    fn fetch_schedule_index(
        &self,
        basic_credentials: &Option<CredentialPair>,
        week_date: Option<NaiveDate>,
        gid: Option<&str>,
    ) -> Result<ResponseSnapshot> {
        let url = self.schedule_index_url(week_date, gid)?;
        let response = self
            .get_following_redirects(&url, basic_credentials)
            .with_context(|| format!("ScheduleIndex の取得に失敗しました: {url}"))?;
        if !is_schedule_index_page(&response.url, &response.body) {
            bail!("ScheduleIndex に到達できませんでした: {}", response.url);
        }
        Ok(response)
    }

    fn schedule_index_url(
        &self,
        week_date: Option<NaiveDate>,
        gid: Option<&str>,
    ) -> Result<String> {
        let mut url = Url::parse(&self.config.base_url)?;
        {
            let mut query = url.query_pairs_mut();
            query.clear();
            query.append_pair("page", "ScheduleIndex");
            if let Some(gid) = gid {
                query.append_pair("GID", gid);
            }
            if let Some(week_date) = week_date {
                let date = format_da_date(week_date);
                query.append_pair("Date", &date);
                query.append_pair("cp", "");
                query.append_pair("sp", "");
                query.append_pair("BDate", &date);
                query.append_pair("BKGID", "");
                query.append_pair("sEID", "");
                query.append_pair("Head", "0");
                query.append_pair("Text", "");
            }
        }

        Ok(url.to_string())
    }

    fn login_post_url(&self) -> Result<String> {
        self.config
            .office_login_post_url
            .clone()
            .or_else(|| infer_related_url(&self.config.base_url, "/api/auth/redirect.do"))
            .ok_or_else(|| anyhow::anyhow!("Cybozu ログイン POST 先を確定できません"))
    }

    fn get_token_url(&self) -> Result<String> {
        infer_related_url(&self.config.base_url, "/api/auth/getToken.json?_lc=ja")
            .ok_or_else(|| anyhow::anyhow!("getToken URL を確定できません"))
    }

    fn login_json_url(&self) -> Result<String> {
        infer_related_url(&self.config.base_url, "/api/auth/login.json?_lc=ja")
            .ok_or_else(|| anyhow::anyhow!("login.json URL を確定できません"))
    }

    fn available_days_url(&self) -> Result<String> {
        infer_related_url(&self.config.base_url, "/api/auth/availableDays.json?_lc=ja")
            .ok_or_else(|| anyhow::anyhow!("availableDays URL を確定できません"))
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

    fn list_events(&mut self, query: ListQuery) -> Result<Vec<CalendarEvent>> {
        let (_, pages) = self.authenticated_schedule_index(&query)?;
        let mut events = Vec::new();

        for page in pages {
            let calendar_name = extract_calendar_name(&page.body);
            events.extend(parse_schedule_index_events(
                &page.body,
                &page.url,
                calendar_name.as_deref(),
            )?);
        }

        events.retain(|event| {
            let starts_before_upper = query.to.is_none_or(|upper| event.starts_at < upper);
            let ends_after_lower = query.from.is_none_or(|lower| event.ends_at > lower);
            starts_before_upper && ends_after_lower
        });
        events.sort_by(|left, right| {
            left.starts_at
                .cmp(&right.starts_at)
                .then_with(|| left.title.cmp(&right.title))
                .then_with(|| left.id.cmp(&right.id))
        });
        let mut seen_occurrences = HashSet::new();
        events.retain(|event| seen_occurrences.insert(occurrence_key_from_event_id(&event.id)));

        Ok(events)
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

fn parse_schedule_index_events(
    html: &str,
    page_url: &str,
    calendar_name: Option<&str>,
) -> Result<Vec<CalendarEvent>> {
    let document = Html::parse_document(html);
    let container_selector =
        Selector::parse("div.dragTarget[data-cb-eid]").expect("valid schedule item selector");
    let link_selector = Selector::parse("a.event[href*=\"page=ScheduleView\"]")
        .expect("valid schedule link selector");

    let mut events = Vec::new();
    for container in document.select(&container_selector) {
        let Some(link) = container.select(&link_selector).next() else {
            continue;
        };

        let href = link
            .value()
            .attr("href")
            .ok_or_else(|| anyhow::anyhow!("ScheduleView リンクの href がありません"))?;
        let identity = parse_schedule_view_identity(page_url, href)?;
        let anchor_date = container
            .value()
            .attr("data-cb-date")
            .and_then(parse_da_date)
            .or_else(|| parse_da_date(&identity.date))
            .ok_or_else(|| anyhow::anyhow!("基準日を取得できません: {}", identity.seid))?;
        let raw_starts_at = container
            .value()
            .attr("data-cb-st")
            .ok_or_else(|| anyhow::anyhow!("data-cb-st がありません: {}", identity.seid))?;
        let starts_at = parse_cb_datetime(raw_starts_at, anchor_date)
            .with_context(|| format!("開始日時を解釈できません: {}", identity.seid))?;
        let raw_ends_at = container
            .value()
            .attr("data-cb-et")
            .ok_or_else(|| anyhow::anyhow!("data-cb-et がありません: {}", identity.seid))?;
        let mut ends_at = parse_cb_datetime(raw_ends_at, anchor_date)
            .with_context(|| format!("終了日時を解釈できません: {}", identity.seid))?;
        if is_all_day_cb_marker(raw_starts_at)
            && is_all_day_cb_marker(raw_ends_at)
            && ends_at == starts_at
        {
            ends_at += chrono::TimeDelta::days(1);
        }
        let title = normalize_whitespace(&link.text().collect::<String>());
        if title.is_empty() {
            continue;
        }

        events.push(CalendarEvent {
            id: composite_event_id(&identity),
            title,
            description: None,
            starts_at,
            ends_at,
            attendees: Vec::new(),
            facility: None,
            calendar: calendar_name.map(str::to_string),
            version: 1,
        });
    }

    Ok(events)
}

fn parse_schedule_view_identity(page_url: &str, href: &str) -> Result<ScheduleViewIdentity> {
    let absolute = Url::parse(page_url)?.join(href)?;
    let absolute = absolute.to_string();
    Ok(ScheduleViewIdentity {
        uid: required_query_parameter(&absolute, "UID")?,
        gid: required_query_parameter(&absolute, "GID")?,
        date: required_query_parameter(&absolute, "Date")?,
        bdate: required_query_parameter(&absolute, "BDate")?,
        seid: required_query_parameter(&absolute, "sEID")?,
    })
}

fn required_query_parameter(url: &str, key: &str) -> Result<String> {
    extract_query_parameter(url, key)
        .ok_or_else(|| anyhow::anyhow!("必須クエリ {key} がありません: {url}"))
}

fn composite_event_id(identity: &ScheduleViewIdentity) -> String {
    format!(
        "sEID={}&UID={}&GID={}&Date={}&BDate={}",
        identity.seid, identity.uid, identity.gid, identity.date, identity.bdate
    )
}

fn occurrence_key_from_event_id(id: &str) -> String {
    let url = format!("https://example.invalid/?{id}");
    let seid = extract_query_parameter(&url, "sEID");
    let date = extract_query_parameter(&url, "Date");
    let bdate = extract_query_parameter(&url, "BDate");
    match (seid, date, bdate) {
        (Some(seid), Some(date), Some(bdate)) => format!("sEID={seid}&Date={date}&BDate={bdate}"),
        _ => id.to_string(),
    }
}

fn parse_cb_datetime(value: &str, anchor_date: NaiveDate) -> Result<DateTime<FixedOffset>> {
    let parts = value.split('.').collect::<Vec<_>>();
    let (year, month, day, hour, minute, second) = match parts.as_slice() {
        ["dt", year, month, day, hour, minute, second] => (
            year.parse::<i32>()?,
            month.parse::<u32>()?,
            day.parse::<u32>()?,
            parse_cb_time_part(hour)?,
            parse_cb_time_part(minute)?,
            parse_cb_time_part(second)?,
        ),
        ["tm", hour, minute, second] => (
            anchor_date.year(),
            anchor_date.month(),
            anchor_date.day(),
            parse_cb_time_part(hour)?,
            parse_cb_time_part(minute)?,
            parse_cb_time_part(second)?,
        ),
        _ => bail!("未対応の日時形式です: {value}"),
    };
    let offset = jst_offset();
    offset
        .with_ymd_and_hms(year, month, day, hour, minute, second)
        .single()
        .ok_or_else(|| anyhow::anyhow!("日時を構築できません: {value}"))
}

fn parse_cb_time_part(value: &str) -> Result<u32> {
    if value == "-1" {
        return Ok(0);
    }
    Ok(value.parse::<u32>()?)
}

fn is_all_day_cb_marker(value: &str) -> bool {
    matches!(
        value.split('.').collect::<Vec<_>>().as_slice(),
        ["dt", _, _, _, "-1", "-1", "-1"]
    )
}

fn extract_schedule_anchor_date(html: &str, page_url: &str) -> Option<NaiveDate> {
    extract_hidden_input_value(html, "Date")
        .and_then(|value| parse_da_date(&value))
        .or_else(|| {
            extract_query_parameter(page_url, "Date").and_then(|value| parse_da_date(&value))
        })
}

fn extract_schedule_index_gid(html: &str, page_url: &str) -> Option<String> {
    let document = Html::parse_document(html);
    let link_selector =
        Selector::parse("a[href*=\"GID=\"], button[onclick*=\"GID=\"]").expect("valid selector");

    for element in document.select(&link_selector) {
        if let Some(href) = element.value().attr("href")
            && let Ok(absolute) = Url::parse(page_url).and_then(|url| url.join(href))
            && let Some(gid) = extract_query_parameter(absolute.as_str(), "GID")
        {
            return Some(gid);
        }

        if let Some(onclick) = element.value().attr("onclick")
            && let Some(fragment) = extract_single_quoted_fragment(onclick)
            && let Ok(absolute) = Url::parse(page_url).and_then(|url| url.join(&fragment))
            && let Some(gid) = extract_query_parameter(absolute.as_str(), "GID")
        {
            return Some(gid);
        }
    }

    extract_query_parameter(page_url, "GID")
}

fn extract_single_quoted_fragment(value: &str) -> Option<String> {
    let start = value.find('\'')? + 1;
    let rest = &value[start..];
    let end = rest.find('\'')?;
    Some(rest[..end].to_string())
}

fn extract_calendar_name(html: &str) -> Option<String> {
    let document = Html::parse_document(html);
    let selector = Selector::parse("select option[selected]").expect("valid selector");
    let option = document.select(&selector).next()?;
    let label = normalize_whitespace(&option.text().collect::<String>());
    (!label.is_empty()).then_some(label)
}

fn normalize_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn parse_da_date(value: &str) -> Option<NaiveDate> {
    let stripped = value.strip_prefix("da.")?;
    let mut parts = stripped.split('.');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    NaiveDate::from_ymd_opt(year, month, day)
}

fn format_da_date(value: NaiveDate) -> String {
    format!("da.{}.{}.{}", value.year(), value.month(), value.day())
}

fn list_target_weeks(query: &ListQuery, default_week: NaiveDate) -> Vec<NaiveDate> {
    match (query.from, query.to) {
        (Some(from), Some(to)) if to <= from => Vec::new(),
        (Some(from), Some(to)) => {
            let start = week_start(from.date_naive());
            let end = week_start((to - chrono::TimeDelta::seconds(1)).date_naive());
            weeks_between(start, end)
        }
        (Some(from), None) => vec![week_start(from.date_naive())],
        (None, Some(to)) => vec![week_start(
            (to - chrono::TimeDelta::seconds(1)).date_naive(),
        )],
        (None, None) => vec![default_week],
    }
}

fn week_start(date: NaiveDate) -> NaiveDate {
    let days_from_monday = date.weekday().num_days_from_monday();
    date.checked_sub_days(Days::new(days_from_monday.into()))
        .expect("valid monday calculation")
}

fn weeks_between(start: NaiveDate, end: NaiveDate) -> Vec<NaiveDate> {
    let mut weeks = Vec::new();
    let mut current = start;
    while current <= end {
        weeks.push(current);
        current = current
            .checked_add_days(Days::new(7))
            .expect("valid week increment");
    }
    weeks
}

fn today_jst() -> NaiveDate {
    Utc::now().with_timezone(&jst_offset()).date_naive()
}

fn jst_offset() -> FixedOffset {
    FixedOffset::east_opt(JST_OFFSET_SECONDS).expect("valid JST offset")
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;

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

    #[test]
    fn parses_schedule_index_events_from_drag_target_nodes() {
        let html = r#"
<html>
  <body>
    <select name="GID">
      <option value="183" selected>ＩＴＧ</option>
    </select>
    <div class="dragTarget dnd-eventdiv-draggable"
         data-cb-date="da.2026.3.9"
         data-cb-uid="379"
         data-cb-st="dt.2026.3.9.9.30.0"
         data-cb-et="dt.2026.3.9.16.30.0"
         data-cb-eid="3092194">
      <div class="eventLink">
        <div class="eventInner">
          <span class="eventDateTime">9:30-16:30&nbsp;</span><br>
          <span class="eventDetail">
            <a class="event"
               href="ag.cgi?page=ScheduleView&amp;UID=379&amp;GID=183&amp;Date=da.2026.3.9&amp;BDate=da.2026.3.9&amp;sEID=3092194&amp;CP=sg"
               title="退寮対応のため職員室待機予定">退寮対応のため職員室待機予定</a>
          </span>
        </div>
      </div>
    </div>
    <div class="dragTarget dnd-eventdiv-draggable"
         data-cb-date="da.2026.3.9"
         data-cb-uid="379"
         data-cb-st="dt.2026.3.9.17.30.0"
         data-cb-et="dt.2026.3.9.17.30.0"
         data-cb-eid="2570212">
      <div class="eventLink">
        <div class="eventInner">
          <span class="eventDetail">
            <a class="event"
               href="ag.cgi?page=ScheduleView&amp;UID=379&amp;GID=183&amp;Date=da.2026.3.9&amp;BDate=da.2026.3.9&amp;sEID=2570212&amp;CP=sg">撤退<img alt="繰り返し予定"></a>
          </span>
        </div>
      </div>
    </div>
    <div class="dragTarget dnd-eventdiv-draggable"
         data-cb-date="da.2026.3.10"
         data-cb-uid="379"
         data-cb-st="dt.2026.3.10.-1.-1.-1"
         data-cb-et="dt.2026.3.10.-1.-1.-1"
         data-cb-eid="3095230">
      <div class="eventLink">
        <div class="eventInner">
          <span class="eventDetail">
            <a class="event"
               href="ag.cgi?page=ScheduleView&amp;UID=379&amp;GID=183&amp;Date=da.2026.3.10&amp;BDate=da.2026.3.9&amp;sEID=3095230&amp;CP=sg">【広報】放課後オープン2件</a>
          </span>
        </div>
      </div>
    </div>
    <div class="dragTarget dnd-eventdiv-draggable"
         data-cb-date="da.2026.3.9"
         data-cb-uid="229"
         data-cb-st="dt.2026.3.9.17.0.0"
         data-cb-et="dt.2026.3.9.17.0.0"
         data-cb-eid="private">
      <div class="eventLink">
        <div class="eventInner">
          <span class="eventDetail">予定あり</span>
        </div>
      </div>
    </div>
  </body>
</html>
"#;

        let events = parse_schedule_index_events(
            html,
            "https://example.cybozu.com/o/ag.cgi?page=ScheduleIndex",
            extract_calendar_name(html).as_deref(),
        )
        .expect("events");

        assert_eq!(events.len(), 3);
        assert_eq!(events[0].title, "退寮対応のため職員室待機予定");
        assert_eq!(events[0].calendar.as_deref(), Some("ＩＴＧ"));
        assert_eq!(
            events[0].id,
            "sEID=3092194&UID=379&GID=183&Date=da.2026.3.9&BDate=da.2026.3.9"
        );
        assert_eq!(
            events[1].starts_at,
            DateTime::parse_from_rfc3339("2026-03-09T17:30:00+09:00").expect("timestamp")
        );
        assert_eq!(events[1].title, "撤退");
        assert_eq!(
            events[2].starts_at,
            DateTime::parse_from_rfc3339("2026-03-10T00:00:00+09:00").expect("timestamp")
        );
        assert_eq!(
            events[2].ends_at,
            DateTime::parse_from_rfc3339("2026-03-11T00:00:00+09:00").expect("timestamp")
        );
    }

    #[test]
    fn target_weeks_expand_across_multiple_schedule_pages() {
        let query = ListQuery {
            from: Some(
                DateTime::parse_from_rfc3339("2026-03-09T00:00:00+09:00").expect("timestamp"),
            ),
            to: Some(DateTime::parse_from_rfc3339("2026-03-23T00:00:00+09:00").expect("timestamp")),
        };

        let weeks = list_target_weeks(&query, NaiveDate::from_ymd_opt(2026, 3, 9).expect("date"));
        assert_eq!(
            weeks,
            vec![
                NaiveDate::from_ymd_opt(2026, 3, 9).expect("date"),
                NaiveDate::from_ymd_opt(2026, 3, 16).expect("date"),
            ]
        );
    }

    #[test]
    fn occurrence_key_ignores_uid_for_group_week_duplicates() {
        assert_eq!(
            occurrence_key_from_event_id(
                "sEID=3048561&UID=379&GID=183&Date=da.2026.3.10&BDate=da.2026.3.9"
            ),
            "sEID=3048561&Date=da.2026.3.10&BDate=da.2026.3.9"
        );
    }
}
