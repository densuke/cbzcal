use std::collections::HashSet;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Datelike, Days, FixedOffset, NaiveDate, TimeZone, Timelike, Utc};
use reqwest::{
    Url,
    blocking::{Client, RequestBuilder},
    header::LOCATION,
};
use scraper::{ElementRef, Html, Selector};
use serde::Serialize;
use serde_json::Value;

use crate::{
    backend::{CalendarBackend, ListQuery},
    config::{CredentialPair, CredentialSource, CybozuHtmlConfig},
    model::{CalendarEvent, CloneOverrides, EventPatch, NewEvent, short_id_from_event_id},
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScheduleIndexContext {
    current_user_uid: String,
    gid: String,
    week_start: NaiveDate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HtmlForm {
    action_url: String,
    fields: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ShortEventReference {
    seid: String,
    date: NaiveDate,
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
        let (basic_credentials, first_schedule_index) =
            self.bootstrap_authenticated_schedule_index()?;

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

    fn bootstrap_authenticated_schedule_index(
        &self,
    ) -> Result<(Option<CredentialPair>, ResponseSnapshot)> {
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

        Ok((basic_credentials, first_schedule_index))
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

    fn extract_schedule_index_context(
        &self,
        page: &ResponseSnapshot,
    ) -> Result<ScheduleIndexContext> {
        let current_user_uid =
            extract_current_user_uid(&page.body, &page.url).ok_or_else(|| {
                anyhow::anyhow!("現在ユーザーの UID を ScheduleIndex から取得できません")
            })?;
        let gid = extract_schedule_index_gid(&page.body, &page.url)
            .ok_or_else(|| anyhow::anyhow!("ScheduleIndex の GID を取得できません"))?;
        let week_start = extract_schedule_anchor_date(&page.body, &page.url)
            .unwrap_or_else(|| week_start(today_jst()));

        Ok(ScheduleIndexContext {
            current_user_uid,
            gid,
            week_start,
        })
    }

    fn schedule_entry_url(
        &self,
        context: &ScheduleIndexContext,
        event_date: NaiveDate,
    ) -> Result<String> {
        let mut url = Url::parse(&self.config.base_url)?;
        let bdate = week_start(event_date);
        {
            let mut query = url.query_pairs_mut();
            query.clear();
            query.append_pair("page", "ScheduleEntry");
            query.append_pair("UID", &context.current_user_uid);
            query.append_pair("GID", &context.gid);
            query.append_pair("Date", &format_da_date(event_date));
            query.append_pair("BDate", &format_da_date(bdate));
            query.append_pair("cp", "sg");
        }

        Ok(url.to_string())
    }

    fn fetch_schedule_entry(
        &self,
        basic_credentials: &Option<CredentialPair>,
        context: &ScheduleIndexContext,
        event_date: NaiveDate,
    ) -> Result<ResponseSnapshot> {
        let url = self.schedule_entry_url(context, event_date)?;
        let response = self
            .get_following_redirects(&url, basic_credentials)
            .with_context(|| format!("ScheduleEntry の取得に失敗しました: {url}"))?;
        if !is_schedule_entry_page(&response.url, &response.body) {
            bail!("ScheduleEntry に到達できませんでした: {}", response.url);
        }
        Ok(response)
    }

    fn schedule_modify_url(&self, identity: &ScheduleViewIdentity) -> Result<String> {
        let mut url = Url::parse(&self.config.base_url)?;
        {
            let mut query = url.query_pairs_mut();
            query.clear();
            query.append_pair("page", "ScheduleModify");
            query.append_pair("UID", &identity.uid);
            query.append_pair("GID", &identity.gid);
            query.append_pair("Date", &identity.date);
            query.append_pair("BDate", &identity.bdate);
            query.append_pair("sEID", &identity.seid);
            query.append_pair("cp", "sgv");
        }

        Ok(url.to_string())
    }

    fn fetch_schedule_modify(
        &self,
        basic_credentials: &Option<CredentialPair>,
        identity: &ScheduleViewIdentity,
    ) -> Result<ResponseSnapshot> {
        let url = self.schedule_modify_url(identity)?;
        let response = self
            .get_following_redirects(&url, basic_credentials)
            .with_context(|| format!("ScheduleModify の取得に失敗しました: {url}"))?;
        if !is_schedule_modify_page(&response.url, &response.body) {
            bail!("ScheduleModify に到達できませんでした: {}", response.url);
        }
        Ok(response)
    }

    fn schedule_delete_url(&self, identity: &ScheduleViewIdentity) -> Result<String> {
        let mut url = Url::parse(&self.config.base_url)?;
        {
            let mut query = url.query_pairs_mut();
            query.clear();
            query.append_pair("page", "ScheduleDelete");
            query.append_pair("UID", &identity.uid);
            query.append_pair("GID", &identity.gid);
            query.append_pair("Date", &identity.date);
            query.append_pair("BDate", &identity.bdate);
            query.append_pair("sEID", &identity.seid);
            query.append_pair("cp", "sgv");
        }

        Ok(url.to_string())
    }

    fn fetch_schedule_delete(
        &self,
        basic_credentials: &Option<CredentialPair>,
        identity: &ScheduleViewIdentity,
    ) -> Result<ResponseSnapshot> {
        let url = self.schedule_delete_url(identity)?;
        let response = self
            .get_following_redirects(&url, basic_credentials)
            .with_context(|| format!("ScheduleDelete の取得に失敗しました: {url}"))?;
        if !is_schedule_delete_page(&response.url, &response.body) {
            bail!("ScheduleDelete に到達できませんでした: {}", response.url);
        }
        Ok(response)
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

    fn post_form_following_redirects(
        &self,
        url: &str,
        form: &[(String, String)],
        referer: &str,
        credentials: &Option<CredentialPair>,
    ) -> Result<ResponseSnapshot> {
        let response = self
            .request_with_optional_basic(self.client.post(url), credentials)
            .header("Referer", referer)
            .header("Origin", request_origin(url)?)
            .form(form)
            .send()?;

        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| anyhow::anyhow!("POST 後の redirect location がありません"))?;
            let redirect_url = response
                .url()
                .join(location)
                .map(|url| url.to_string())
                .context("POST 後の redirect URL を解決できません")?;
            return self.get_following_redirects(&redirect_url, credentials);
        }

        let status = response.status().as_u16();
        let response_url = response.url().to_string();
        let body = response.text()?;
        if is_redirect_stub_page(&body)
            && let Some(redirect_url) = extract_js_redirect_url(&response_url, &body)
        {
            return self.get_following_redirects(&redirect_url, credentials);
        }

        Ok(ResponseSnapshot {
            status,
            url: response_url,
            body,
        })
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
            let current_user_uid = extract_current_user_uid(&page.body, &page.url);
            events.extend(parse_schedule_index_events(
                &page.body,
                &page.url,
                calendar_name.as_deref(),
                current_user_uid.as_deref(),
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

    fn add_event(&mut self, input: NewEvent) -> Result<CalendarEvent> {
        input.validate()?;
        validate_supported_add_input(&input)?;

        let starts_at = input.starts_at.with_timezone(&jst_offset());
        let event_date = starts_at.date_naive();
        let target_week = week_start(event_date);
        let (basic_credentials, first_schedule_index) =
            self.bootstrap_authenticated_schedule_index()?;
        let mut context = self.extract_schedule_index_context(&first_schedule_index)?;
        context.week_start = target_week;

        let schedule_entry = self.fetch_schedule_entry(&basic_credentials, &context, event_date)?;
        let mut form = parse_html_form(&schedule_entry.body, &schedule_entry.url, "ScheduleEntry")?;
        populate_schedule_entry_form(&mut form.fields, &input, &context);
        let submit_response = self.post_form_following_redirects(
            &form.action_url,
            &form.fields,
            &schedule_entry.url,
            &basic_credentials,
        )?;
        if is_login_page(&submit_response.url, &submit_response.body) {
            bail!("予定登録後にログイン画面へ戻りました。セッションを維持できていません");
        }

        let week_page =
            self.fetch_schedule_index(&basic_credentials, Some(target_week), Some(&context.gid))?;
        let calendar_name = extract_calendar_name(&week_page.body);
        let events = parse_schedule_index_events(
            &week_page.body,
            &week_page.url,
            calendar_name.as_deref(),
            Some(&context.current_user_uid),
        )?;

        find_created_event(events, &input)
            .ok_or_else(|| anyhow::anyhow!("登録後の予定を ScheduleIndex から特定できませんでした"))
    }

    fn update_event(&mut self, id: &str, patch: EventPatch) -> Result<CalendarEvent> {
        validate_supported_update_patch(&patch)?;

        let (basic_credentials, identity) = self.resolve_event_identity(id)?;
        let schedule_modify = self.fetch_schedule_modify(&basic_credentials, &identity)?;
        let mut form = parse_html_form(
            &schedule_modify.body,
            &schedule_modify.url,
            "ScheduleModify",
        )?;
        let current_event = parse_schedule_modify_event(&form.fields, &identity)?;
        let updated_event = current_event.apply_patch(&patch)?;
        validate_supported_update_event(&updated_event)?;
        populate_schedule_modify_form(&mut form.fields, &updated_event, &identity);
        let submit_response = self.post_form_following_redirects(
            &form.action_url,
            &form.fields,
            &schedule_modify.url,
            &basic_credentials,
        )?;
        if is_login_page(&submit_response.url, &submit_response.body) {
            bail!("予定変更後にログイン画面へ戻りました。セッションを維持できていません");
        }

        let target_week = week_start(
            updated_event
                .starts_at
                .with_timezone(&jst_offset())
                .date_naive(),
        );
        let week_page =
            self.fetch_schedule_index(&basic_credentials, Some(target_week), Some(&identity.gid))?;
        let calendar_name = extract_calendar_name(&week_page.body);
        let events = parse_schedule_index_events(
            &week_page.body,
            &week_page.url,
            calendar_name.as_deref(),
            Some(&identity.uid),
        )?;

        find_event_by_seid(events, &identity.seid)
            .map(|mut event| {
                event.description = updated_event.description.clone();
                event
            })
            .ok_or_else(|| anyhow::anyhow!("更新後の予定を ScheduleIndex から特定できませんでした"))
    }

    fn clone_event(&mut self, _id: &str, _overrides: CloneOverrides) -> Result<CalendarEvent> {
        bail!(self.pending_contract_error("events clone"));
    }

    fn delete_event(&mut self, _id: &str) -> Result<()> {
        let (basic_credentials, identity) = self.resolve_event_identity(_id)?;
        let schedule_delete = self.fetch_schedule_delete(&basic_credentials, &identity)?;
        let mut form = parse_html_form(
            &schedule_delete.body,
            &schedule_delete.url,
            "ScheduleDelete",
        )?;
        validate_supported_delete_form(&form.fields)?;
        populate_schedule_delete_form(&mut form.fields, &identity);
        let submit_response = self.post_form_following_redirects(
            &form.action_url,
            &form.fields,
            &schedule_delete.url,
            &basic_credentials,
        )?;
        if is_login_page(&submit_response.url, &submit_response.body) {
            bail!("予定削除後にログイン画面へ戻りました。セッションを維持できていません");
        }

        let target_week = parse_da_date(&identity.bdate)
            .or_else(|| parse_da_date(&identity.date).map(week_start))
            .ok_or_else(|| anyhow::anyhow!("削除確認用の週情報を解釈できません"))?;
        let week_page =
            self.fetch_schedule_index(&basic_credentials, Some(target_week), Some(&identity.gid))?;
        let calendar_name = extract_calendar_name(&week_page.body);
        let events = parse_schedule_index_events(
            &week_page.body,
            &week_page.url,
            calendar_name.as_deref(),
            Some(&identity.uid),
        )?;

        if find_event_by_seid(events, &identity.seid).is_some() {
            bail!(
                "削除後も対象予定が ScheduleIndex に残っています: {}",
                identity.seid
            );
        }

        Ok(())
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

fn is_schedule_entry_page(url: &str, body: &str) -> bool {
    url.contains("ScheduleEntry") && body.contains("name=\"ScheduleEntry\"")
}

fn is_schedule_modify_page(url: &str, body: &str) -> bool {
    url.contains("ScheduleModify") && body.contains("name=\"ScheduleModify\"")
}

fn is_schedule_delete_page(url: &str, body: &str) -> bool {
    url.contains("ScheduleDelete") && body.contains("name=\"ScheduleDelete\"")
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

fn validate_supported_add_input(input: &NewEvent) -> Result<()> {
    if input.title.trim().is_empty() {
        bail!("タイトルは必須です");
    }
    if !input.attendees.is_empty() {
        bail!("現時点の `events add` は参加者追加に未対応です");
    }
    if input.facility.is_some() {
        bail!("現時点の `events add` は設備予約に未対応です");
    }
    if input.calendar.is_some() {
        bail!("現時点の `events add` はカレンダー指定に未対応です");
    }

    let starts_at = input.starts_at.with_timezone(&jst_offset());
    let ends_at = input.ends_at.with_timezone(&jst_offset());
    if starts_at.second() != 0 || ends_at.second() != 0 {
        bail!("現時点の `events add` は分単位のみ対応です");
    }
    if !is_supported_single_day_event(starts_at, ends_at) {
        bail!("現時点の `events add` は単日予定のみ対応です");
    }

    Ok(())
}

fn validate_supported_update_patch(patch: &EventPatch) -> Result<()> {
    if patch.attendees.is_some() {
        bail!("現時点の `events update` は参加者更新に未対応です");
    }
    if patch.facility.is_some() {
        bail!("現時点の `events update` は設備予約更新に未対応です");
    }
    if patch.calendar.is_some() {
        bail!("現時点の `events update` はカレンダー更新に未対応です");
    }

    Ok(())
}

fn validate_supported_delete_form(fields: &[(String, String)]) -> Result<()> {
    if form_value(fields, "Apply").is_some() {
        bail!("現時点の `events delete` は繰り返し予定に未対応です");
    }
    match form_value(fields, "Member") {
        Some("all") => Ok(()),
        Some("single") => bail!("現時点の `events delete` は参加者単位の削除に未対応です"),
        Some(value) => bail!("現時点の `events delete` は未対応の削除種別です: {value}"),
        None => bail!("削除フォームの Member が見つかりません"),
    }
}

impl CybozuHtmlBackend {
    fn resolve_event_identity(
        &self,
        id_or_short_id: &str,
    ) -> Result<(Option<CredentialPair>, ScheduleViewIdentity)> {
        let (basic_credentials, first_schedule_index) =
            self.bootstrap_authenticated_schedule_index()?;
        let identity = if id_or_short_id.contains('&') {
            parse_composite_event_id(id_or_short_id)?
        } else {
            self.resolve_short_event_identity(
                id_or_short_id,
                &basic_credentials,
                &first_schedule_index,
            )?
        };
        Ok((basic_credentials, identity))
    }

    fn resolve_short_event_identity(
        &self,
        short_id: &str,
        basic_credentials: &Option<CredentialPair>,
        first_schedule_index: &ResponseSnapshot,
    ) -> Result<ScheduleViewIdentity> {
        let short = parse_short_event_reference(short_id)?;
        let context = self.extract_schedule_index_context(first_schedule_index)?;
        let target_week = week_start(short.date);
        let default_week =
            extract_schedule_anchor_date(&first_schedule_index.body, &first_schedule_index.url)
                .unwrap_or_else(|| week_start(today_jst()));
        let page = if target_week == default_week {
            first_schedule_index.clone()
        } else {
            self.fetch_schedule_index(basic_credentials, Some(target_week), Some(&context.gid))?
        };
        let calendar_name = extract_calendar_name(&page.body);
        let events = parse_schedule_index_events(
            &page.body,
            &page.url,
            calendar_name.as_deref(),
            Some(&context.current_user_uid),
        )?;
        let event = find_event_by_short_id(events, short_id)
            .ok_or_else(|| anyhow::anyhow!("短縮 ID に対応する予定が見つかりません: {short_id}"))?;
        parse_composite_event_id(&event.id)
    }
}

fn is_supported_single_day_event(
    starts_at: DateTime<FixedOffset>,
    ends_at: DateTime<FixedOffset>,
) -> bool {
    if starts_at.date_naive() == ends_at.date_naive() {
        return true;
    }

    starts_at.hour() == 0
        && starts_at.minute() == 0
        && ends_at.hour() == 0
        && ends_at.minute() == 0
        && starts_at
            .date_naive()
            .checked_add_days(Days::new(1))
            .is_some_and(|next_day| next_day == ends_at.date_naive())
}

fn validate_supported_update_event(event: &CalendarEvent) -> Result<()> {
    let starts_at = event.starts_at.with_timezone(&jst_offset());
    let ends_at = event.ends_at.with_timezone(&jst_offset());
    if starts_at.second() != 0 || ends_at.second() != 0 {
        bail!("現時点の `events update` は分単位のみ対応です");
    }
    if !is_supported_single_day_event(starts_at, ends_at) {
        bail!("現時点の `events update` は単日予定のみ対応です");
    }

    Ok(())
}

fn parse_html_form(html: &str, page_url: &str, form_name: &str) -> Result<HtmlForm> {
    let document = Html::parse_document(html);
    let form_selector = Selector::parse(&format!("form[name=\"{form_name}\"]"))
        .map_err(|error| anyhow::anyhow!("form selector を解釈できません: {error}"))?;
    let field_selector =
        Selector::parse("input[name], textarea[name], select[name]").expect("valid field selector");
    let form = document
        .select(&form_selector)
        .next()
        .ok_or_else(|| anyhow::anyhow!("{form_name} form が見つかりません"))?;
    let action = form.value().attr("action").unwrap_or(page_url);
    let action_url = Url::parse(page_url)?.join(action)?.to_string();
    let mut fields = Vec::new();

    for field in form.select(&field_selector) {
        let Some(name) = field.value().attr("name") else {
            continue;
        };
        let tag = field.value().name();
        match tag {
            "input" => {
                let input_type = field
                    .value()
                    .attr("type")
                    .unwrap_or("text")
                    .to_ascii_lowercase();
                match input_type.as_str() {
                    "submit" | "button" | "image" | "file" => {}
                    "checkbox" | "radio" => {
                        if field.value().attr("checked").is_some() {
                            let value = field.value().attr("value").unwrap_or("on");
                            fields.push((name.to_string(), value.to_string()));
                        }
                    }
                    _ => {
                        let value = field.value().attr("value").unwrap_or("");
                        fields.push((name.to_string(), value.to_string()));
                    }
                }
            }
            "textarea" => {
                fields.push((name.to_string(), field.text().collect::<String>()));
            }
            "select" => {
                if let Some(value) = extract_select_value(field) {
                    fields.push((name.to_string(), value));
                }
            }
            _ => {}
        }
    }

    Ok(HtmlForm { action_url, fields })
}

fn extract_select_value(select: ElementRef<'_>) -> Option<String> {
    let option_selector = Selector::parse("option").expect("valid option selector");
    let options = select.select(&option_selector).collect::<Vec<_>>();
    let selected = options
        .iter()
        .find(|option| option.value().attr("selected").is_some())
        .copied()
        .or_else(|| options.first().copied())?;
    Some(selected.value().attr("value").unwrap_or("").to_string())
}

fn set_form_value(fields: &mut Vec<(String, String)>, name: &str, value: impl Into<String>) {
    let value = value.into();
    let mut matched = false;
    for (field_name, field_value) in fields.iter_mut() {
        if field_name == name {
            *field_value = value.clone();
            matched = true;
        }
    }
    if !matched {
        fields.push((name.to_string(), value));
    }
}

fn form_value<'a>(fields: &'a [(String, String)], name: &str) -> Option<&'a str> {
    fields
        .iter()
        .find_map(|(field_name, field_value)| (field_name == name).then_some(field_value.as_str()))
}

fn required_form_value<'a>(fields: &'a [(String, String)], name: &str) -> Result<&'a str> {
    form_value(fields, name).ok_or_else(|| anyhow::anyhow!("必須フォーム項目 {name} がありません"))
}

fn populate_schedule_entry_form(
    fields: &mut Vec<(String, String)>,
    input: &NewEvent,
    context: &ScheduleIndexContext,
) {
    let starts_at = input.starts_at.with_timezone(&jst_offset());
    let ends_at = input.ends_at.with_timezone(&jst_offset());
    let event_date = starts_at.date_naive();
    let bdate = week_start(event_date);
    let is_all_day = starts_at.hour() == 0
        && starts_at.minute() == 0
        && ends_at.hour() == 0
        && ends_at.minute() == 0
        && starts_at
            .date_naive()
            .checked_add_days(Days::new(1))
            .is_some_and(|next_day| next_day == ends_at.date_naive());

    set_form_value(fields, "page", "ScheduleEntry");
    set_form_value(fields, "UID", context.current_user_uid.clone());
    set_form_value(fields, "GID", context.gid.clone());
    set_form_value(fields, "Date", format_da_date(event_date));
    set_form_value(fields, "BDate", format_da_date(bdate));
    set_form_value(fields, "SetDate.Year", starts_at.year().to_string());
    set_form_value(fields, "SetDate.Month", starts_at.month().to_string());
    set_form_value(fields, "SetDate.Day", starts_at.day().to_string());
    set_form_value(fields, "SetMultiDates", format_da_date(event_date));
    if is_all_day {
        set_form_value(fields, "SetTime.Hour", "");
        set_form_value(fields, "SetTime.Minute", "");
        set_form_value(fields, "EndTime.Hour", "");
        set_form_value(fields, "EndTime.Minute", "");
    } else {
        set_form_value(fields, "SetTime.Hour", starts_at.hour().to_string());
        set_form_value(
            fields,
            "SetTime.Minute",
            format!("{:02}", starts_at.minute()),
        );
        set_form_value(fields, "EndTime.Hour", ends_at.hour().to_string());
        set_form_value(fields, "EndTime.Minute", format!("{:02}", ends_at.minute()));
    }
    set_form_value(fields, "Detail", input.title.trim().to_string());
    set_form_value(
        fields,
        "Memo",
        input.description.as_deref().unwrap_or("").to_string(),
    );
    set_form_value(fields, "sUID", context.current_user_uid.clone());
    set_form_value(fields, "Entry", "登録する");
}

fn parse_schedule_modify_event(
    fields: &[(String, String)],
    identity: &ScheduleViewIdentity,
) -> Result<CalendarEvent> {
    let date = parse_schedule_form_date(fields)?;
    let (starts_at, ends_at) = parse_schedule_form_time_range(fields, date)?;
    Ok(CalendarEvent {
        id: composite_event_id(identity),
        title: required_form_value(fields, "Detail")?.trim().to_string(),
        description: form_value(fields, "Memo")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        starts_at,
        ends_at,
        attendees: Vec::new(),
        facility: None,
        calendar: None,
        version: 1,
    })
}

fn parse_schedule_form_date(fields: &[(String, String)]) -> Result<NaiveDate> {
    let year = required_form_value(fields, "SetDate.Year")?.parse::<i32>()?;
    let month = required_form_value(fields, "SetDate.Month")?.parse::<u32>()?;
    let day = required_form_value(fields, "SetDate.Day")?.parse::<u32>()?;
    NaiveDate::from_ymd_opt(year, month, day)
        .ok_or_else(|| anyhow::anyhow!("フォームの日付を構築できません"))
}

fn parse_schedule_form_time_range(
    fields: &[(String, String)],
    date: NaiveDate,
) -> Result<(DateTime<FixedOffset>, DateTime<FixedOffset>)> {
    let start_hour = required_form_value(fields, "SetTime.Hour")?;
    let start_minute = required_form_value(fields, "SetTime.Minute")?;
    let end_hour = required_form_value(fields, "EndTime.Hour")?;
    let end_minute = required_form_value(fields, "EndTime.Minute")?;

    if start_hour.is_empty()
        && start_minute.is_empty()
        && end_hour.is_empty()
        && end_minute.is_empty()
    {
        let starts_at = jst_offset()
            .from_local_datetime(
                &date
                    .and_hms_opt(0, 0, 0)
                    .ok_or_else(|| anyhow::anyhow!("終日予定の開始日時を構築できません"))?,
            )
            .single()
            .ok_or_else(|| anyhow::anyhow!("終日予定の開始日時を構築できません"))?;
        let next_day = date
            .checked_add_days(Days::new(1))
            .ok_or_else(|| anyhow::anyhow!("終日予定の終了日を計算できません"))?;
        let ends_at = jst_offset()
            .from_local_datetime(
                &next_day
                    .and_hms_opt(0, 0, 0)
                    .ok_or_else(|| anyhow::anyhow!("終日予定の終了日時を構築できません"))?,
            )
            .single()
            .ok_or_else(|| anyhow::anyhow!("終日予定の終了日時を構築できません"))?;
        return Ok((starts_at, ends_at));
    }

    let starts_at = schedule_form_datetime(date, start_hour, start_minute)?;
    let ends_at = schedule_form_datetime(date, end_hour, end_minute)?;
    Ok((starts_at, ends_at))
}

fn schedule_form_datetime(
    date: NaiveDate,
    hour: &str,
    minute: &str,
) -> Result<DateTime<FixedOffset>> {
    let hour = hour.parse::<u32>()?;
    let minute = minute.parse::<u32>()?;
    if hour == 24 && minute == 0 {
        let next_day = date
            .checked_add_days(Days::new(1))
            .ok_or_else(|| anyhow::anyhow!("24:00 の終了日を計算できません"))?;
        return jst_offset()
            .from_local_datetime(
                &next_day
                    .and_hms_opt(0, 0, 0)
                    .ok_or_else(|| anyhow::anyhow!("24:00 の日時を構築できません"))?,
            )
            .single()
            .ok_or_else(|| anyhow::anyhow!("24:00 の日時を構築できません"));
    }

    jst_offset()
        .from_local_datetime(
            &date
                .and_hms_opt(hour, minute, 0)
                .ok_or_else(|| anyhow::anyhow!("フォーム日時を構築できません"))?,
        )
        .single()
        .ok_or_else(|| anyhow::anyhow!("フォーム日時を構築できません"))
}

fn populate_schedule_modify_form(
    fields: &mut Vec<(String, String)>,
    event: &CalendarEvent,
    identity: &ScheduleViewIdentity,
) {
    let starts_at = event.starts_at.with_timezone(&jst_offset());
    let ends_at = event.ends_at.with_timezone(&jst_offset());
    let event_date = starts_at.date_naive();
    let bdate = week_start(event_date);
    let is_all_day = starts_at.hour() == 0
        && starts_at.minute() == 0
        && ends_at.hour() == 0
        && ends_at.minute() == 0
        && starts_at
            .date_naive()
            .checked_add_days(Days::new(1))
            .is_some_and(|next_day| next_day == ends_at.date_naive());

    set_form_value(fields, "page", "ScheduleModify");
    set_form_value(fields, "sEID", identity.seid.clone());
    set_form_value(fields, "UID", identity.uid.clone());
    set_form_value(fields, "GID", identity.gid.clone());
    set_form_value(fields, "Date", format_da_date(event_date));
    set_form_value(fields, "BDate", format_da_date(bdate));
    set_form_value(fields, "SetDate.Year", starts_at.year().to_string());
    set_form_value(fields, "SetDate.Month", starts_at.month().to_string());
    set_form_value(fields, "SetDate.Day", starts_at.day().to_string());
    if is_all_day {
        set_form_value(fields, "SetTime.Hour", "");
        set_form_value(fields, "SetTime.Minute", "");
        set_form_value(fields, "EndTime.Hour", "");
        set_form_value(fields, "EndTime.Minute", "");
    } else {
        set_form_value(fields, "SetTime.Hour", starts_at.hour().to_string());
        set_form_value(
            fields,
            "SetTime.Minute",
            format!("{:02}", starts_at.minute()),
        );
        set_form_value(fields, "EndTime.Hour", format_end_hour(ends_at, is_all_day));
        set_form_value(fields, "EndTime.Minute", format!("{:02}", ends_at.minute()));
    }
    set_form_value(fields, "Detail", event.title.trim().to_string());
    set_form_value(
        fields,
        "Memo",
        event.description.as_deref().unwrap_or("").to_string(),
    );
    set_form_value(fields, "Modify", "変更する");
}

fn format_end_hour(ends_at: DateTime<FixedOffset>, is_all_day: bool) -> String {
    if is_all_day {
        String::new()
    } else {
        ends_at.hour().to_string()
    }
}

fn populate_schedule_delete_form(
    fields: &mut Vec<(String, String)>,
    identity: &ScheduleViewIdentity,
) {
    set_form_value(fields, "page", "ScheduleDelete");
    set_form_value(fields, "sEID", identity.seid.clone());
    set_form_value(fields, "UID", identity.uid.clone());
    set_form_value(fields, "GID", identity.gid.clone());
    set_form_value(fields, "Date", identity.date.clone());
    set_form_value(fields, "BDate", identity.bdate.clone());
    set_form_value(fields, "Member", "all");
    set_form_value(fields, "Yes", "削除する");
}

fn find_created_event(events: Vec<CalendarEvent>, input: &NewEvent) -> Option<CalendarEvent> {
    let starts_at = input.starts_at.with_timezone(&jst_offset());
    let ends_at = input.ends_at.with_timezone(&jst_offset());
    let mut matches = events
        .into_iter()
        .filter(|event| {
            event.title == input.title.trim()
                && event.starts_at == starts_at
                && event.ends_at == ends_at
        })
        .collect::<Vec<_>>();
    matches.sort_by_key(|event| extract_numeric_seid(&event.id).unwrap_or_default());
    let mut created = matches.pop()?;
    created.description = input.description.clone();
    Some(created)
}

fn find_event_by_seid(events: Vec<CalendarEvent>, seid: &str) -> Option<CalendarEvent> {
    events.into_iter().find(|event| {
        let url = format!("https://example.invalid/?{}", event.id);
        extract_query_parameter(&url, "sEID").as_deref() == Some(seid)
    })
}

fn find_event_by_short_id(events: Vec<CalendarEvent>, short_id: &str) -> Option<CalendarEvent> {
    events
        .into_iter()
        .find(|event| short_id_from_event_id(&event.id) == short_id)
}

fn extract_numeric_seid(id: &str) -> Option<u64> {
    let url = format!("https://example.invalid/?{id}");
    extract_query_parameter(&url, "sEID")?.parse::<u64>().ok()
}

fn parse_composite_event_id(id: &str) -> Result<ScheduleViewIdentity> {
    let url = format!("https://example.invalid/?{id}");
    Ok(ScheduleViewIdentity {
        uid: required_query_parameter(&url, "UID")?,
        gid: required_query_parameter(&url, "GID")?,
        date: required_query_parameter(&url, "Date")?,
        bdate: required_query_parameter(&url, "BDate")?,
        seid: required_query_parameter(&url, "sEID")?,
    })
}

fn parse_short_event_reference(input: &str) -> Result<ShortEventReference> {
    let (seid, date) = input.split_once('@').ok_or_else(|| {
        anyhow::anyhow!("短縮 ID は `sEID@YYYY-MM-DD` 形式で指定してください: {input}")
    })?;
    if seid.is_empty() {
        bail!("短縮 ID の sEID が空です: {input}");
    }
    let date = NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .with_context(|| format!("短縮 ID の日付を解釈できません: {input}"))?;
    Ok(ShortEventReference {
        seid: seid.to_string(),
        date,
    })
}

fn parse_schedule_index_events(
    html: &str,
    page_url: &str,
    calendar_name: Option<&str>,
    current_user_uid: Option<&str>,
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
        if let Some(current_user_uid) = current_user_uid
            && identity.uid != current_user_uid
        {
            continue;
        }
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

fn extract_current_user_uid(html: &str, page_url: &str) -> Option<String> {
    let document = Html::parse_document(html);
    let selector =
        Selector::parse("a[href*=\"page=ScheduleEntry\"][href*=\"UID=\"]").expect("valid selector");

    for link in document.select(&selector) {
        let label = normalize_whitespace(&link.text().collect::<String>());
        if label != "予定を登録する" {
            continue;
        }

        let href = link.value().attr("href")?;
        let absolute = Url::parse(page_url).ok()?.join(href).ok()?;
        if let Some(uid) = extract_query_parameter(absolute.as_str(), "UID") {
            return Some(uid);
        }
    }

    None
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
            Some("379"),
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

    #[test]
    fn extracts_current_user_uid_from_primary_schedule_entry_link() {
        let html = r#"
<html>
  <body>
    <a href="ag.cgi?page=ScheduleEntry&amp;UID=379&amp;GID=183&amp;Date=da.2026.3.9&amp;BDate=da.2026.3.9&amp;cp=sg">予定を登録する</a>
    <a href="ag.cgi?page=ScheduleEntry&amp;UID=999&amp;GID=183&amp;Date=da.2026.3.9&amp;BDate=da.2026.3.9&amp;CP=sg"><img alt="予定の登録"></a>
  </body>
</html>
"#;

        assert_eq!(
            extract_current_user_uid(
                html,
                "https://example.cybozu.com/o/ag.cgi?page=ScheduleIndex"
            )
            .as_deref(),
            Some("379")
        );
    }

    #[test]
    fn parses_schedule_entry_form_fields() {
        let html = r#"
<html>
  <body>
    <form name="ScheduleEntry" method="POST" action="ag.cgi?">
      <input type="hidden" name="page" value="ScheduleEntry">
      <input type="hidden" name="UID" value="379">
      <input type="hidden" name="csrf_ticket" value="ticket">
      <input type="text" name="Detail" value="">
      <textarea name="Memo"></textarea>
      <select name="FGID">
        <option value="100">A</option>
        <option value="394" selected>B</option>
      </select>
      <input type="file" name="files[]">
      <input type="submit" name="Entry" value="登録する">
    </form>
  </body>
</html>
"#;

        let form = parse_html_form(
            html,
            "https://example.cybozu.com/o/ag.cgi?page=ScheduleEntry",
            "ScheduleEntry",
        )
        .expect("form");

        assert_eq!(form.action_url, "https://example.cybozu.com/o/ag.cgi?");
        assert!(
            form.fields
                .contains(&(String::from("page"), String::from("ScheduleEntry")))
        );
        assert!(
            form.fields
                .contains(&(String::from("UID"), String::from("379")))
        );
        assert!(
            form.fields
                .contains(&(String::from("csrf_ticket"), String::from("ticket")))
        );
        assert!(
            form.fields
                .contains(&(String::from("FGID"), String::from("394")))
        );
        assert!(!form.fields.iter().any(|(name, _)| name == "files[]"));
        assert!(!form.fields.iter().any(|(name, _)| name == "Entry"));
    }

    #[test]
    fn populates_schedule_entry_form_for_single_day_event() {
        let mut fields = vec![
            ("page".to_string(), "ScheduleEntry".to_string()),
            ("UID".to_string(), "379".to_string()),
            ("GID".to_string(), "183".to_string()),
            ("Date".to_string(), "da.2026.3.9".to_string()),
            ("BDate".to_string(), "da.2026.3.9".to_string()),
            ("SetDate.Year".to_string(), "2026".to_string()),
            ("SetDate.Month".to_string(), "3".to_string()),
            ("SetDate.Day".to_string(), "9".to_string()),
            ("SetMultiDates".to_string(), "da.2026.3.9".to_string()),
            ("SetTime.Hour".to_string(), "".to_string()),
            ("SetTime.Minute".to_string(), "".to_string()),
            ("EndTime.Hour".to_string(), "".to_string()),
            ("EndTime.Minute".to_string(), "".to_string()),
            ("Detail".to_string(), "".to_string()),
            ("Memo".to_string(), "".to_string()),
            ("sUID".to_string(), "379".to_string()),
        ];
        let input = NewEvent {
            title: "テスト予定".to_string(),
            description: Some("本文".to_string()),
            starts_at: DateTime::parse_from_rfc3339("2026-03-11T09:30:00+09:00")
                .expect("timestamp"),
            ends_at: DateTime::parse_from_rfc3339("2026-03-11T10:15:00+09:00").expect("timestamp"),
            attendees: Vec::new(),
            facility: None,
            calendar: None,
        };
        let context = ScheduleIndexContext {
            current_user_uid: "379".to_string(),
            gid: "183".to_string(),
            week_start: NaiveDate::from_ymd_opt(2026, 3, 9).expect("date"),
        };

        populate_schedule_entry_form(&mut fields, &input, &context);

        assert!(fields.contains(&(String::from("Date"), String::from("da.2026.3.11"))));
        assert!(fields.contains(&(String::from("BDate"), String::from("da.2026.3.9"))));
        assert!(fields.contains(&(String::from("SetTime.Hour"), String::from("9"))));
        assert!(fields.contains(&(String::from("SetTime.Minute"), String::from("30"))));
        assert!(fields.contains(&(String::from("EndTime.Hour"), String::from("10"))));
        assert!(fields.contains(&(String::from("EndTime.Minute"), String::from("15"))));
        assert!(fields.contains(&(String::from("Detail"), String::from("テスト予定"))));
        assert!(fields.contains(&(String::from("Memo"), String::from("本文"))));
        assert!(fields.contains(&(String::from("Entry"), String::from("登録する"))));
    }

    #[test]
    fn rejects_multi_day_add_requests() {
        let input = NewEvent {
            title: "テスト予定".to_string(),
            description: None,
            starts_at: DateTime::parse_from_rfc3339("2026-03-11T23:00:00+09:00")
                .expect("timestamp"),
            ends_at: DateTime::parse_from_rfc3339("2026-03-12T00:30:00+09:00").expect("timestamp"),
            attendees: Vec::new(),
            facility: None,
            calendar: None,
        };

        let error = validate_supported_add_input(&input).expect_err("should reject");
        assert!(error.to_string().contains("単日予定"));
    }

    #[test]
    fn allows_all_day_add_requests() {
        let input = NewEvent {
            title: "終日予定".to_string(),
            description: None,
            starts_at: DateTime::parse_from_rfc3339("2026-03-11T00:00:00+09:00")
                .expect("timestamp"),
            ends_at: DateTime::parse_from_rfc3339("2026-03-12T00:00:00+09:00").expect("timestamp"),
            attendees: Vec::new(),
            facility: None,
            calendar: None,
        };

        validate_supported_add_input(&input).expect("should allow");
    }

    #[test]
    fn parses_composite_event_identity() {
        let identity = parse_composite_event_id(
            "sEID=3096804&UID=379&GID=183&Date=da.2099.1.7&BDate=da.2099.1.5",
        )
        .expect("identity");

        assert_eq!(identity.seid, "3096804");
        assert_eq!(identity.uid, "379");
        assert_eq!(identity.gid, "183");
        assert_eq!(identity.date, "da.2099.1.7");
        assert_eq!(identity.bdate, "da.2099.1.5");
    }

    #[test]
    fn parses_short_event_reference() {
        let short = parse_short_event_reference("3096804@2099-01-07").expect("short");
        assert_eq!(short.seid, "3096804");
        assert_eq!(
            short.date,
            NaiveDate::from_ymd_opt(2099, 1, 7).expect("date")
        );
    }

    #[test]
    fn finds_event_by_short_id() {
        let events = vec![
            CalendarEvent {
                id: "sEID=3096804&UID=379&GID=183&Date=da.2099.1.7&BDate=da.2099.1.5".to_string(),
                title: "updated title".to_string(),
                description: None,
                starts_at: DateTime::parse_from_rfc3339("2099-01-07T13:00:00+09:00")
                    .expect("timestamp"),
                ends_at: DateTime::parse_from_rfc3339("2099-01-07T14:30:00+09:00")
                    .expect("timestamp"),
                attendees: Vec::new(),
                facility: None,
                calendar: None,
                version: 1,
            },
            CalendarEvent {
                id: "fixture-123".to_string(),
                title: "fixture".to_string(),
                description: None,
                starts_at: DateTime::parse_from_rfc3339("2099-01-07T15:00:00+09:00")
                    .expect("timestamp"),
                ends_at: DateTime::parse_from_rfc3339("2099-01-07T16:00:00+09:00")
                    .expect("timestamp"),
                attendees: Vec::new(),
                facility: None,
                calendar: None,
                version: 1,
            },
        ];

        let found = find_event_by_short_id(events, "3096804@2099-01-07").expect("event");
        assert_eq!(found.title, "updated title");
    }

    #[test]
    fn parses_schedule_modify_form_into_event() {
        let fields = vec![
            ("SetDate.Year".to_string(), "2099".to_string()),
            ("SetDate.Month".to_string(), "1".to_string()),
            ("SetDate.Day".to_string(), "7".to_string()),
            ("SetTime.Hour".to_string(), "9".to_string()),
            ("SetTime.Minute".to_string(), "00".to_string()),
            ("EndTime.Hour".to_string(), "11".to_string()),
            ("EndTime.Minute".to_string(), "00".to_string()),
            (
                "Detail".to_string(),
                "[cbzcal] friendly time probe 20260309".to_string(),
            ),
            ("Memo".to_string(), "friendly timed add".to_string()),
        ];
        let identity = ScheduleViewIdentity {
            uid: "379".to_string(),
            gid: "183".to_string(),
            date: "da.2099.1.7".to_string(),
            bdate: "da.2099.1.5".to_string(),
            seid: "3096804".to_string(),
        };

        let event = parse_schedule_modify_event(&fields, &identity).expect("event");

        assert_eq!(
            event.id,
            "sEID=3096804&UID=379&GID=183&Date=da.2099.1.7&BDate=da.2099.1.5"
        );
        assert_eq!(event.title, "[cbzcal] friendly time probe 20260309");
        assert_eq!(event.description.as_deref(), Some("friendly timed add"));
        assert_eq!(
            event.starts_at,
            DateTime::parse_from_rfc3339("2099-01-07T09:00:00+09:00").expect("timestamp")
        );
        assert_eq!(
            event.ends_at,
            DateTime::parse_from_rfc3339("2099-01-07T11:00:00+09:00").expect("timestamp")
        );
    }

    #[test]
    fn populates_schedule_modify_form_for_single_day_event() {
        let mut fields = vec![
            ("page".to_string(), "ScheduleModify".to_string()),
            ("sEID".to_string(), "3096804".to_string()),
            ("UID".to_string(), "379".to_string()),
            ("GID".to_string(), "183".to_string()),
            ("Date".to_string(), "da.2099.1.7".to_string()),
            ("BDate".to_string(), "da.2099.1.5".to_string()),
            ("SetDate.Year".to_string(), "2099".to_string()),
            ("SetDate.Month".to_string(), "1".to_string()),
            ("SetDate.Day".to_string(), "7".to_string()),
            ("SetTime.Hour".to_string(), "9".to_string()),
            ("SetTime.Minute".to_string(), "00".to_string()),
            ("EndTime.Hour".to_string(), "11".to_string()),
            ("EndTime.Minute".to_string(), "00".to_string()),
            ("Detail".to_string(), "old".to_string()),
            ("Memo".to_string(), "old memo".to_string()),
        ];
        let event = CalendarEvent {
            id: "sEID=3096804&UID=379&GID=183&Date=da.2099.1.8&BDate=da.2099.1.5".to_string(),
            title: "updated title".to_string(),
            description: Some("updated memo".to_string()),
            starts_at: DateTime::parse_from_rfc3339("2099-01-08T13:30:00+09:00")
                .expect("timestamp"),
            ends_at: DateTime::parse_from_rfc3339("2099-01-08T15:00:00+09:00").expect("timestamp"),
            attendees: Vec::new(),
            facility: None,
            calendar: None,
            version: 2,
        };
        let identity = ScheduleViewIdentity {
            uid: "379".to_string(),
            gid: "183".to_string(),
            date: "da.2099.1.7".to_string(),
            bdate: "da.2099.1.5".to_string(),
            seid: "3096804".to_string(),
        };

        populate_schedule_modify_form(&mut fields, &event, &identity);

        assert!(fields.contains(&(String::from("Date"), String::from("da.2099.1.8"))));
        assert!(fields.contains(&(String::from("BDate"), String::from("da.2099.1.5"))));
        assert!(fields.contains(&(String::from("SetTime.Hour"), String::from("13"))));
        assert!(fields.contains(&(String::from("SetTime.Minute"), String::from("30"))));
        assert!(fields.contains(&(String::from("EndTime.Hour"), String::from("15"))));
        assert!(fields.contains(&(String::from("EndTime.Minute"), String::from("00"))));
        assert!(fields.contains(&(String::from("Detail"), String::from("updated title"))));
        assert!(fields.contains(&(String::from("Memo"), String::from("updated memo"))));
        assert!(fields.contains(&(String::from("Modify"), String::from("変更する"))));
    }

    #[test]
    fn validates_simple_delete_form() {
        let fields = vec![("Member".to_string(), "all".to_string())];
        validate_supported_delete_form(&fields).expect("simple delete should be supported");
    }

    #[test]
    fn rejects_delete_form_for_single_member_leave() {
        let fields = vec![("Member".to_string(), "single".to_string())];
        let error = validate_supported_delete_form(&fields).expect_err("should reject");
        assert!(error.to_string().contains("参加者単位"));
    }

    #[test]
    fn populates_schedule_delete_form() {
        let mut fields = vec![
            ("page".to_string(), "ScheduleDelete".to_string()),
            ("sEID".to_string(), "3096804".to_string()),
            ("Date".to_string(), "da.2099.1.7".to_string()),
            ("BDate".to_string(), "da.2099.1.5".to_string()),
            ("UID".to_string(), "379".to_string()),
            ("GID".to_string(), "183".to_string()),
            ("Member".to_string(), "all".to_string()),
        ];
        let identity = ScheduleViewIdentity {
            uid: "379".to_string(),
            gid: "183".to_string(),
            date: "da.2099.1.7".to_string(),
            bdate: "da.2099.1.5".to_string(),
            seid: "3096804".to_string(),
        };

        populate_schedule_delete_form(&mut fields, &identity);

        assert!(fields.contains(&(String::from("page"), String::from("ScheduleDelete"))));
        assert!(fields.contains(&(String::from("sEID"), String::from("3096804"))));
        assert!(fields.contains(&(String::from("Member"), String::from("all"))));
        assert!(fields.contains(&(String::from("Yes"), String::from("削除する"))));
    }
}
