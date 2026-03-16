#![allow(clippy::type_complexity)]
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, Timelike};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

use crate::{
    backend::ApplyScope,
    cli::{AddArgs, ApplyScopeArg, CloneArgs, DeleteArgs, EventsCommand, ListArgs, UpdateArgs},
    config::{AppConfig, OllamaConfig},
    datetime::{
        current_jst_date, normalize_prompt_duration, normalize_prompt_time, parse_duration,
        parse_flexible_date, parse_prompt_timestamp, to_jst_datetime,
    },
    model::{CalendarEvent, EventVisibility},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptAction {
    List,
    Add,
    Update,
    Clone,
    Delete,
}

#[derive(Debug)]
pub struct PromptExecution {
    pub action: PromptAction,
    pub command: EventsCommand,
    pub shell_command: String,
    pub summary_lines: Vec<String>,
}

impl PromptExecution {
    pub fn supports_yes(&self) -> bool {
        matches!(
            self.action,
            PromptAction::List | PromptAction::Add | PromptAction::Clone
        )
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
struct PromptPlan {
    action: PromptActionWire,
    id: Option<String>,
    title: Option<String>,
    title_suffix: Option<String>,
    date: Option<String>,
    from: Option<String>,
    to: Option<String>,
    at: Option<String>,
    until: Option<String>,
    #[serde(rename = "for")]
    duration: Option<String>,
    all_day: Option<bool>,
    description: Option<String>,
    clear_description: Option<bool>,
    start: Option<String>,
    end: Option<String>,
    visibility: Option<String>,
    scope: Option<String>,
    web: Option<bool>,
    preserve_time: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum PromptActionWire {
    List,
    Add,
    Update,
    Clone,
    Delete,
}

#[derive(Debug, Serialize)]
struct OllamaGenerateRequest<'a> {
    model: &'a str,
    prompt: String,
    stream: bool,
    format: &'a str,
}

#[derive(Debug, Deserialize)]
struct OllamaGenerateResponse {
    response: String,
}

pub fn plan_prompt(
    config: &AppConfig,
    prompt: &str,
    anchor: Option<NaiveDate>,
    existing_event: Option<&CalendarEvent>,
) -> Result<PromptExecution> {
    let anchor = anchor.unwrap_or_else(current_jst_date);
    let ollama = config.ollama.clone().unwrap_or_default();
    let raw = request_plan_from_ollama(&ollama, prompt, anchor)?;
    let plan = normalize_prompt_plan(parse_prompt_plan(&raw)?, prompt, anchor);
    build_execution(plan, anchor, existing_event)
}

pub fn render_preview(execution: &PromptExecution) -> String {
    let mut out = String::from("解釈結果:\n");
    for line in &execution.summary_lines {
        out.push_str("  ");
        out.push_str(line);
        out.push('\n');
    }
    out.push('\n');
    out.push_str("実行コマンド:\n");
    out.push_str("  ");
    out.push_str(&execution.shell_command);
    out
}

fn request_plan_from_ollama(
    ollama: &OllamaConfig,
    prompt: &str,
    anchor: NaiveDate,
) -> Result<String> {
    let endpoint = format!("{}/api/generate", ollama.base_url().trim_end_matches('/'));
    let request = OllamaGenerateRequest {
        model: ollama.model(),
        prompt: build_system_prompt(prompt, anchor),
        stream: false,
        format: "json",
    };

    let client = Client::new();
    let response = client
        .post(&endpoint)
        .json(&request)
        .send()
        .with_context(|| format!("ollama に接続できませんでした: {endpoint}"))?;

    if !response.status().is_success() {
        bail!(
            "ollama の応答が異常です: HTTP {} ({endpoint})",
            response.status()
        );
    }

    let generated: OllamaGenerateResponse = response
        .json()
        .context("ollama の応答を解釈できませんでした")?;
    Ok(generated.response)
}

fn build_system_prompt(user_prompt: &str, anchor: NaiveDate) -> String {
    format!(
        concat!(
            "You convert a Japanese calendar instruction into a single JSON object.\n",
            "Return JSON only. No markdown. No explanation.\n",
            "Today in JST is {anchor}.\n",
            "Allowed action values: list, add, update, clone, delete.\n",
            "Use these keys when needed: action, id, title, title_suffix, date, from, to, at, until, for, all_day, description, clear_description, start, end, visibility, scope, web, preserve_time.\n",
            "Rules:\n",
            "- Prefer short friendly values like today, tomorrow, 2026-03-10, 15:00, 1h.\n",
            "- If you use start/end, include timezone like +09:00.\n",
            "- For '同じ時間' or '同時間', set preserve_time=true and set the target date.\n",
            "- For all-day events, set all_day=true and omit at/until/for/start/end.\n",
            "- For add visibility, use visibility=public or visibility=private.\n",
            "- For update/delete/clone, always include id.\n",
            "- For recurring scope, use this, after, or all.\n",
            "- If an explicit RFC3339 time is more precise, use start/end.\n",
            "- Omit keys that are not needed.\n",
            "User request: {user_prompt}\n"
        ),
        anchor = anchor.format("%Y-%m-%d"),
        user_prompt = user_prompt
    )
}

fn infer_visibility_from_prompt(prompt: &str) -> Option<String> {
    if prompt.contains("非公開") {
        return Some("private".to_string());
    }
    if prompt.contains("公開") && !prompt.contains("公開しない") && !prompt.contains("非公開")
    {
        return Some("public".to_string());
    }
    None
}

fn normalize_prompt_plan(mut plan: PromptPlan, prompt: &str, anchor: NaiveDate) -> PromptPlan {
    if plan
        .id
        .as_deref()
        .is_some_and(|value| looks_like_date_expression(value, anchor))
        && !prompt_mentions_explicit_id(prompt)
    {
        plan.id = None;
    }
    if plan.visibility.is_none() {
        plan.visibility = infer_visibility_from_prompt(prompt);
    }
    if plan.title.is_none() {
        plan.title = infer_title_from_prompt(prompt);
    }
    if plan.date.is_none() {
        plan.date = infer_date_from_prompt(prompt, anchor);
    }
    if plan.id.is_none()
        && prompt_implies_add(prompt)
        && !matches!(plan.action, PromptActionWire::Add)
    {
        plan.action = PromptActionWire::Add;
    }
    if matches!(plan.action, PromptActionWire::Add)
        && plan.date.is_some()
        && plan.start.is_none()
        && plan.end.is_none()
        && plan.at.is_none()
        && plan.until.is_none()
        && plan.duration.is_none()
        && plan.all_day.is_none()
    {
        plan.all_day = Some(true);
    }
    plan
}

fn prompt_mentions_explicit_id(prompt: &str) -> bool {
    prompt.contains("ID") || prompt.contains("id") || prompt.contains("Id")
}

fn looks_like_date_expression(value: &str, anchor: NaiveDate) -> bool {
    let trimmed = value.trim();
    infer_date_from_prompt(trimmed, anchor).is_some()
        || trimmed == "今日"
        || trimmed == "明日"
        || trimmed == "明後日"
}

fn prompt_implies_add(prompt: &str) -> bool {
    prompt.contains("設定") || prompt.contains("追加") || prompt.contains("登録")
}

fn infer_title_from_prompt(prompt: &str) -> Option<String> {
    for (open, close) in [('「', '」'), ('『', '』'), ('"', '"'), ('\'', '\'')] {
        if let Some(title) = extract_quoted(prompt, open, close) {
            return Some(title);
        }
    }
    None
}

fn extract_quoted(input: &str, open: char, close: char) -> Option<String> {
    let start = input.find(open)?;
    let rest = &input[start + open.len_utf8()..];
    let end = rest.find(close)?;
    let value = rest[..end].trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn infer_date_from_prompt(prompt: &str, anchor: NaiveDate) -> Option<String> {
    if prompt.contains("明後日") {
        return anchor
            .checked_add_days(chrono::Days::new(2))
            .map(|date| date.format("%Y-%m-%d").to_string());
    }
    if prompt.contains("明日") {
        return anchor
            .checked_add_days(chrono::Days::new(1))
            .map(|date| date.format("%Y-%m-%d").to_string());
    }
    if prompt.contains("今日") {
        return Some(anchor.format("%Y-%m-%d").to_string());
    }
    if let Some((month, day)) = extract_month_day(prompt) {
        let year = anchor.year();
        if let Some(date) = NaiveDate::from_ymd_opt(year, month, day) {
            return Some(date.format("%Y-%m-%d").to_string());
        }
    }
    if let Some(day) = extract_day_only(prompt) {
        let mut year = anchor.year();
        let mut month = anchor.month();
        if day < anchor.day() {
            if month == 12 {
                year += 1;
                month = 1;
            } else {
                month += 1;
            }
        }
        if let Some(date) = NaiveDate::from_ymd_opt(year, month, day) {
            return Some(date.format("%Y-%m-%d").to_string());
        }
    }
    None
}

fn extract_month_day(prompt: &str) -> Option<(u32, u32)> {
    let slash = prompt.find('/')?;
    let month = prompt[..slash]
        .chars()
        .rev()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>()
        .parse::<u32>()
        .ok()?;
    let rest = &prompt[slash + 1..];
    let day = rest
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>()
        .parse::<u32>()
        .ok()?;
    Some((month, day))
}

fn extract_day_only(prompt: &str) -> Option<u32> {
    let day_pos = prompt.find('日')?;
    let prefix = &prompt[..day_pos];
    let digits = prefix
        .chars()
        .rev()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    if digits.is_empty() {
        return None;
    }
    digits.parse::<u32>().ok()
}

fn parse_prompt_plan(raw: &str) -> Result<PromptPlan> {
    let trimmed = raw.trim();
    let trimmed = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .unwrap_or(trimmed)
        .trim();
    let trimmed = trimmed.strip_suffix("```").unwrap_or(trimmed).trim();
    serde_json::from_str(trimmed).context("prompt 解釈結果の JSON を読めませんでした")
}

fn build_execution(
    plan: PromptPlan,
    anchor: NaiveDate,
    existing_event: Option<&CalendarEvent>,
) -> Result<PromptExecution> {
    match plan.action {
        PromptActionWire::List => build_list_execution(plan),
        PromptActionWire::Add => build_add_execution(plan, anchor),
        PromptActionWire::Update => build_update_execution(plan, anchor, existing_event),
        PromptActionWire::Clone => build_clone_execution(plan, anchor),
        PromptActionWire::Delete => build_delete_execution(plan),
    }
}

fn build_list_execution(plan: PromptPlan) -> Result<PromptExecution> {
    let args = ListArgs {
        json: false,
        from: plan.from,
        to: plan.to,
        date: plan.date,
        duration: plan.duration.map(|value| normalize_prompt_duration(&value)),
    };
    let mut summary = vec!["action: list".to_string()];
    if let Some(date) = &args.date {
        summary.push(format!("date: {date}"));
    }
    if let Some(from) = &args.from {
        summary.push(format!("from: {from}"));
    }
    if let Some(to) = &args.to {
        summary.push(format!("to: {to}"));
    }
    if let Some(duration) = &args.duration {
        summary.push(format!("for: {duration}"));
    }
    let shell_command = build_shell_command("list", &list_flags(&args));
    Ok(PromptExecution {
        action: PromptAction::List,
        command: EventsCommand::List(args),
        shell_command,
        summary_lines: summary,
    })
}

fn build_add_execution(plan: PromptPlan, anchor: NaiveDate) -> Result<PromptExecution> {
    let title = plan
        .title
        .ok_or_else(|| anyhow::anyhow!("add には title が必要です"))?;
    let context_date = plan.date.as_deref();
    let start = parse_optional_timestamp(plan.start.as_deref(), anchor, context_date)?;
    let end = parse_optional_timestamp(plan.end.as_deref(), anchor, context_date)?;
    let uses_strict = start.is_some() || end.is_some();
    let visibility = parse_visibility(plan.visibility.as_deref())?;
    let args = AddArgs {
        json: false,
        title: title.clone(),
        public: matches!(visibility, EventVisibility::Public),
        private: matches!(visibility, EventVisibility::Private),
        start,
        end,
        date: if uses_strict { None } else { plan.date },
        at: if uses_strict {
            None
        } else {
            plan.at.map(|value| normalize_prompt_time(&value))
        },
        until: if uses_strict {
            None
        } else {
            plan.until.map(|value| normalize_prompt_time(&value))
        },
        duration: if uses_strict {
            None
        } else {
            plan.duration.map(|value| normalize_prompt_duration(&value))
        },
        all_day: if uses_strict {
            false
        } else {
            plan.all_day.unwrap_or(false)
        },
        description: plan.description,
        attendees: Vec::new(),
        facility: None,
        calendar: None,
    };
    let mut summary = vec!["action: add".to_string(), format!("title: {title}")];
    if let Some(date) = &args.date {
        summary.push(format!("date: {date}"));
    }
    if let Some(at) = &args.at {
        summary.push(format!("at: {at}"));
    }
    if let Some(until) = &args.until {
        summary.push(format!("until: {until}"));
    }
    if let Some(duration) = &args.duration {
        summary.push(format!("for: {duration}"));
    }
    if let Some(start) = &args.start {
        summary.push(format!("start: {}", start.to_rfc3339()));
    }
    if let Some(end) = &args.end {
        summary.push(format!("end: {}", end.to_rfc3339()));
    }
    if args.all_day {
        summary.push("all_day: true".to_string());
    }
    summary.push(format!(
        "visibility: {}",
        match visibility {
            EventVisibility::Public => "public",
            EventVisibility::Private => "private",
        }
    ));
    let shell_command = build_shell_command("add", &add_flags(&args));
    Ok(PromptExecution {
        action: PromptAction::Add,
        command: EventsCommand::Add(args),
        shell_command,
        summary_lines: summary,
    })
}

fn build_update_execution(
    plan: PromptPlan,
    anchor: NaiveDate,
    existing_event: Option<&CalendarEvent>,
) -> Result<PromptExecution> {
    let id = plan
        .id
        .clone()
        .ok_or_else(|| anyhow::anyhow!("update には id が必要です"))?;
    let (start, end) = resolve_update_times(&plan, anchor, existing_event)?;
    let args = UpdateArgs {
        json: false,
        web: plan.web.unwrap_or(false),
        id: id.clone(),
        scope: parse_scope_arg(plan.scope.as_deref())?,
        title: plan.title,
        start,
        end,
        description: plan.description,
        clear_description: plan.clear_description.unwrap_or(false),
        attendees: Vec::new(),
        clear_attendees: false,
        facility: None,
        clear_facility: false,
        calendar: None,
        clear_calendar: false,
    };
    let mut summary = vec!["action: update".to_string(), format!("id: {id}")];
    if let Some(title) = &args.title {
        summary.push(format!("title: {title}"));
    }
    if let Some(start) = &args.start {
        summary.push(format!("start: {}", start.to_rfc3339()));
    }
    if let Some(end) = &args.end {
        summary.push(format!("end: {}", end.to_rfc3339()));
    }
    if let Some(scope) = &args.scope {
        summary.push(format!("scope: {}", scope_name(*scope)));
    }
    if args.web {
        summary.push("web: true".to_string());
    }
    let shell_command = build_shell_command("update", &update_flags(&args));
    Ok(PromptExecution {
        action: PromptAction::Update,
        command: EventsCommand::Update(args),
        shell_command,
        summary_lines: summary,
    })
}

fn build_clone_execution(plan: PromptPlan, anchor: NaiveDate) -> Result<PromptExecution> {
    let id = plan
        .id
        .clone()
        .ok_or_else(|| anyhow::anyhow!("clone には id が必要です"))?;
    let (start, end) = resolve_target_window(&plan, anchor)?;
    let args = CloneArgs {
        json: false,
        id: id.clone(),
        title: plan.title,
        title_suffix: plan.title_suffix,
        start,
        end,
    };
    let mut summary = vec!["action: clone".to_string(), format!("id: {id}")];
    if let Some(title) = &args.title {
        summary.push(format!("title: {title}"));
    }
    if let Some(suffix) = &args.title_suffix {
        summary.push(format!("title_suffix: {suffix}"));
    }
    if let Some(start) = &args.start {
        summary.push(format!("start: {}", start.to_rfc3339()));
    }
    if let Some(end) = &args.end {
        summary.push(format!("end: {}", end.to_rfc3339()));
    }
    let shell_command = build_shell_command("clone", &clone_flags(&args));
    Ok(PromptExecution {
        action: PromptAction::Clone,
        command: EventsCommand::Clone(args),
        shell_command,
        summary_lines: summary,
    })
}

fn build_delete_execution(plan: PromptPlan) -> Result<PromptExecution> {
    let id = plan
        .id
        .ok_or_else(|| anyhow::anyhow!("delete には id が必要です"))?;
    let args = DeleteArgs {
        json: false,
        id: id.clone(),
        scope: parse_scope_arg(plan.scope.as_deref())?,
    };
    let mut summary = vec!["action: delete".to_string(), format!("id: {id}")];
    if let Some(scope) = &args.scope {
        summary.push(format!("scope: {}", scope_name(*scope)));
    }
    let shell_command = build_shell_command("delete", &delete_flags(&args));
    Ok(PromptExecution {
        action: PromptAction::Delete,
        command: EventsCommand::Delete(args),
        shell_command,
        summary_lines: summary,
    })
}

fn resolve_update_times(
    plan: &PromptPlan,
    anchor: NaiveDate,
    existing_event: Option<&CalendarEvent>,
) -> Result<(Option<DateTime<FixedOffset>>, Option<DateTime<FixedOffset>>)> {
    if plan.start.is_some() || plan.end.is_some() {
        let context_date = plan.date.as_deref();
        return Ok((
            parse_optional_timestamp(plan.start.as_deref(), anchor, context_date)?,
            parse_optional_timestamp(plan.end.as_deref(), anchor, context_date)?,
        ));
    }

    if plan.preserve_time.unwrap_or(false) {
        let existing = existing_event.ok_or_else(|| {
            anyhow::anyhow!("現在の予定時刻を参照する update には既存予定の取得が必要です")
        })?;
        let date = plan
            .date
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("preserve_time を使う場合は date が必要です"))?;
        let date = parse_flexible_date(date, anchor).map_err(anyhow::Error::msg)?;
        let start = to_jst_datetime(
            date,
            existing.starts_at.time().hour(),
            existing.starts_at.time().minute(),
        )
        .map_err(anyhow::Error::msg)?;
        return Ok((Some(start), Some(start + existing.duration())));
    }

    if plan.date.is_some() || plan.at.is_some() || plan.until.is_some() || plan.duration.is_some() {
        let (start, end) = resolve_target_window(plan, anchor)?;
        return Ok((start, end));
    }

    Ok((None, None))
}

fn resolve_target_window(
    plan: &PromptPlan,
    anchor: NaiveDate,
) -> Result<(Option<DateTime<FixedOffset>>, Option<DateTime<FixedOffset>>)> {
    if plan.start.is_some() || plan.end.is_some() {
        let context_date = plan.date.as_deref();
        return Ok((
            parse_optional_timestamp(plan.start.as_deref(), anchor, context_date)?,
            parse_optional_timestamp(plan.end.as_deref(), anchor, context_date)?,
        ));
    }

    let Some(date_input) = plan.date.as_deref() else {
        return Ok((None, None));
    };
    let date = parse_flexible_date(date_input, anchor)?;

    if plan.all_day.unwrap_or(false) {
        let start = to_jst_datetime(date, 0, 0)?;
        let end = start + chrono::TimeDelta::days(1);
        return Ok((Some(start), Some(end)));
    }

    let at = plan
        .at
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("date を使う場合は at も必要です"))?;
    let (start_hour, start_minute) =
        crate::datetime::parse_time_of_day(&normalize_prompt_time(at))?;
    let start = to_jst_datetime(date, start_hour, start_minute)?;
    let end = if let Some(until) = &plan.until {
        let (end_hour, end_minute) =
            crate::datetime::parse_time_of_day(&normalize_prompt_time(until))?;
        to_jst_datetime(date, end_hour, end_minute)?
    } else if let Some(duration) = &plan.duration {
        start + parse_duration(&normalize_prompt_duration(duration))?
    } else {
        bail!("date と at を使う場合は until か for も必要です");
    };
    Ok((Some(start), Some(end)))
}

fn parse_optional_timestamp(
    input: Option<&str>,
    anchor: NaiveDate,
    context_date: Option<&str>,
) -> Result<Option<DateTime<FixedOffset>>> {
    input
        .map(|value| parse_prompt_timestamp(value, anchor, context_date))
        .transpose()
}

fn parse_scope_arg(input: Option<&str>) -> Result<Option<ApplyScopeArg>> {
    input
        .map(|value| match value {
            "this" => Ok(ApplyScopeArg::This),
            "after" => Ok(ApplyScopeArg::After),
            "all" => Ok(ApplyScopeArg::All),
            _ => bail!("scope は this / after / all のいずれかです: {value}"),
        })
        .transpose()
}

fn parse_visibility(input: Option<&str>) -> Result<EventVisibility> {
    match input.unwrap_or("public") {
        "public" => Ok(EventVisibility::Public),
        "private" => Ok(EventVisibility::Private),
        value => bail!("visibility は public / private のいずれかです: {value}"),
    }
}

fn scope_name(scope: ApplyScopeArg) -> &'static str {
    match scope {
        ApplyScopeArg::This => "this",
        ApplyScopeArg::After => "after",
        ApplyScopeArg::All => "all",
    }
}

fn build_shell_command(subcommand: &str, flags: &[(String, Option<String>)]) -> String {
    let mut parts = vec![
        "cbzcal".to_string(),
        "events".to_string(),
        subcommand.to_string(),
    ];
    for (flag, value) in flags {
        parts.push(flag.clone());
        if let Some(value) = value {
            parts.push(shell_escape(value));
        }
    }
    parts.join(" ")
}

fn list_flags(args: &ListArgs) -> Vec<(String, Option<String>)> {
    let mut flags = Vec::new();
    if let Some(date) = &args.date {
        flags.push(("--date".to_string(), Some(date.clone())));
    }
    if let Some(from) = &args.from {
        flags.push(("--from".to_string(), Some(from.clone())));
    }
    if let Some(to) = &args.to {
        flags.push(("--to".to_string(), Some(to.clone())));
    }
    if let Some(duration) = &args.duration {
        flags.push(("--for".to_string(), Some(duration.clone())));
    }
    flags
}

fn add_flags(args: &AddArgs) -> Vec<(String, Option<String>)> {
    let mut flags = vec![("--title".to_string(), Some(args.title.clone()))];
    if args.private {
        flags.push(("--private".to_string(), None));
    } else if args.public {
        flags.push(("--public".to_string(), None));
    }
    if let Some(start) = &args.start {
        flags.push(("--start".to_string(), Some(start.to_rfc3339())));
    }
    if let Some(end) = &args.end {
        flags.push(("--end".to_string(), Some(end.to_rfc3339())));
    }
    if let Some(date) = &args.date {
        flags.push(("--date".to_string(), Some(date.clone())));
    }
    if let Some(at) = &args.at {
        flags.push(("--at".to_string(), Some(at.clone())));
    }
    if let Some(until) = &args.until {
        flags.push(("--until".to_string(), Some(until.clone())));
    }
    if let Some(duration) = &args.duration {
        flags.push(("--for".to_string(), Some(duration.clone())));
    }
    if args.all_day {
        flags.push(("--all-day".to_string(), None));
    }
    if let Some(description) = &args.description {
        flags.push(("--description".to_string(), Some(description.clone())));
    }
    flags
}

fn update_flags(args: &UpdateArgs) -> Vec<(String, Option<String>)> {
    let mut flags = vec![("--id".to_string(), Some(args.id.clone()))];
    if let Some(scope) = &args.scope {
        flags.push(("--scope".to_string(), Some(scope_name(*scope).to_string())));
    }
    if let Some(title) = &args.title {
        flags.push(("--title".to_string(), Some(title.clone())));
    }
    if let Some(start) = &args.start {
        flags.push(("--start".to_string(), Some(start.to_rfc3339())));
    }
    if let Some(end) = &args.end {
        flags.push(("--end".to_string(), Some(end.to_rfc3339())));
    }
    if let Some(description) = &args.description {
        flags.push(("--description".to_string(), Some(description.clone())));
    }
    if args.clear_description {
        flags.push(("--clear-description".to_string(), None));
    }
    if args.web {
        flags.push(("--web".to_string(), None));
    }
    flags
}

fn clone_flags(args: &CloneArgs) -> Vec<(String, Option<String>)> {
    let mut flags = vec![("--id".to_string(), Some(args.id.clone()))];
    if let Some(title) = &args.title {
        flags.push(("--title".to_string(), Some(title.clone())));
    }
    if let Some(suffix) = &args.title_suffix {
        flags.push(("--title-suffix".to_string(), Some(suffix.clone())));
    }
    if let Some(start) = &args.start {
        flags.push(("--start".to_string(), Some(start.to_rfc3339())));
    }
    if let Some(end) = &args.end {
        flags.push(("--end".to_string(), Some(end.to_rfc3339())));
    }
    flags
}

fn delete_flags(args: &DeleteArgs) -> Vec<(String, Option<String>)> {
    let mut flags = vec![("--id".to_string(), Some(args.id.clone()))];
    if let Some(scope) = &args.scope {
        flags.push(("--scope".to_string(), Some(scope_name(*scope).to_string())));
    }
    flags
}

fn shell_escape(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | ':' | '/' | '@' | '.'))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

pub fn apply_scope_from_arg(scope: Option<ApplyScopeArg>) -> Option<ApplyScope> {
    scope.map(|value| match value {
        ApplyScopeArg::This => ApplyScope::This,
        ApplyScopeArg::After => ApplyScope::After,
        ApplyScopeArg::All => ApplyScope::All,
    })
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::model::EventVisibility;

    fn anchor() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 3, 9).expect("date")
    }

    fn event() -> CalendarEvent {
        let jst = FixedOffset::east_opt(9 * 60 * 60).expect("jst");
        CalendarEvent {
            id: "sEID=2570212&UID=379&GID=183&Date=da.2026.3.9&BDate=da.2026.3.9".to_string(),
            title: "サンプル設定".to_string(),
            description: None,
            starts_at: jst
                .with_ymd_and_hms(2026, 3, 9, 13, 30, 0)
                .single()
                .expect("start"),
            ends_at: jst
                .with_ymd_and_hms(2026, 3, 9, 14, 30, 0)
                .single()
                .expect("end"),
            attendees: Vec::new(),
            facility: None,
            calendar: None,
            visibility: EventVisibility::Public,
            version: 1,
        }
    }

    #[test]
    fn update_plan_can_preserve_existing_time() {
        let plan = PromptPlan {
            action: PromptActionWire::Update,
            id: Some("2570212@2026-03-09".to_string()),
            title: None,
            title_suffix: None,
            date: Some("tomorrow".to_string()),
            from: None,
            to: None,
            at: None,
            until: None,
            duration: None,
            all_day: None,
            description: None,
            clear_description: None,
            start: None,
            end: None,
            visibility: None,
            scope: None,
            web: None,
            preserve_time: Some(true),
        };

        let execution = build_execution(plan, anchor(), Some(&event())).expect("execution");
        let EventsCommand::Update(args) = execution.command else {
            panic!("update");
        };
        assert_eq!(
            args.start.expect("start").to_rfc3339(),
            "2026-03-10T13:30:00+09:00"
        );
        assert_eq!(
            args.end.expect("end").to_rfc3339(),
            "2026-03-10T14:30:00+09:00"
        );
    }

    #[test]
    fn add_preview_uses_prompt_long_option_compatible_flags() {
        let plan = PromptPlan {
            action: PromptActionWire::Add,
            id: None,
            title: Some("伊藤様と打ち合わせ".to_string()),
            title_suffix: None,
            date: Some("tomorrow".to_string()),
            from: None,
            to: None,
            at: Some("15:00".to_string()),
            until: None,
            duration: Some("1h".to_string()),
            all_day: None,
            description: None,
            clear_description: None,
            start: None,
            end: None,
            visibility: None,
            scope: None,
            web: None,
            preserve_time: None,
        };

        let execution = build_execution(plan, anchor(), None).expect("execution");
        assert_eq!(
            execution.shell_command,
            "cbzcal events add --title '伊藤様と打ち合わせ' --public --date tomorrow --at 15:00 --for 1h"
        );
    }

    #[test]
    fn prompt_timestamp_accepts_naive_jst_datetime() {
        let timestamp =
            parse_prompt_timestamp("2026-03-10 17:30", anchor(), None).expect("timestamp");
        assert_eq!(timestamp.to_rfc3339(), "2026-03-10T17:30:00+09:00");
    }

    #[test]
    fn prompt_timestamp_accepts_time_with_context_date() {
        let timestamp =
            parse_prompt_timestamp("17時半", anchor(), Some("tomorrow")).expect("timestamp");
        assert_eq!(timestamp.to_rfc3339(), "2026-03-10T17:30:00+09:00");
    }

    #[test]
    fn prompt_duration_normalizes_japanese_units() {
        assert_eq!(normalize_prompt_duration("3時間30分"), "3h30m");
    }

    #[test]
    fn prompt_time_normalizes_offset_suffix() {
        assert_eq!(normalize_prompt_time("17:30+09:00"), "17:30");
        assert_eq!(normalize_prompt_time("17:30:00+09:00"), "17:30");
    }

    #[test]
    fn add_execution_prefers_strict_timestamps_over_date_fields() {
        let plan = PromptPlan {
            action: PromptActionWire::Add,
            id: None,
            title: Some("ミーティング".to_string()),
            title_suffix: None,
            date: Some("2026-03-10".to_string()),
            from: None,
            to: None,
            at: Some("17:30".to_string()),
            until: None,
            duration: Some("3h".to_string()),
            all_day: None,
            description: None,
            clear_description: None,
            start: Some("2026-03-10T17:30:00+09:00".to_string()),
            end: Some("2026-03-10T20:30:00+09:00".to_string()),
            visibility: None,
            scope: None,
            web: None,
            preserve_time: None,
        };

        let execution = build_execution(plan, anchor(), None).expect("execution");
        let EventsCommand::Add(args) = execution.command else {
            panic!("add");
        };
        assert!(args.public);
        assert!(!args.private);
        assert!(args.date.is_none());
        assert!(args.at.is_none());
        assert!(args.duration.is_none());
        assert_eq!(
            execution.shell_command,
            "cbzcal events add --title 'ミーティング' --public --start '2026-03-10T17:30:00+09:00' --end '2026-03-10T20:30:00+09:00'"
        );
    }

    #[test]
    fn prompt_visibility_falls_back_to_private_hint() {
        assert_eq!(
            infer_visibility_from_prompt("明日の17時半から3時間、非公開で設定"),
            Some("private".to_string())
        );
    }

    #[test]
    fn normalize_prompt_plan_infers_all_day_add_from_date_only_request() {
        let plan = PromptPlan {
            action: PromptActionWire::Update,
            id: Some("13日".to_string()),
            title: None,
            title_suffix: None,
            date: None,
            from: None,
            to: None,
            at: None,
            until: None,
            duration: None,
            all_day: None,
            description: None,
            clear_description: None,
            start: None,
            end: None,
            visibility: None,
            scope: None,
            web: None,
            preserve_time: None,
        };

        let normalized = normalize_prompt_plan(plan, "13日は「有給」で設定", anchor());
        assert!(matches!(normalized.action, PromptActionWire::Add));
        assert_eq!(normalized.title.as_deref(), Some("有給"));
        assert_eq!(normalized.date.as_deref(), Some("2026-03-13"));
        assert_eq!(normalized.all_day, Some(true));
        assert!(normalized.id.is_none());
    }

    fn make_plan(action: PromptActionWire) -> PromptPlan {
        PromptPlan {
            action,
            id: None,
            title: None,
            title_suffix: None,
            date: None,
            from: None,
            to: None,
            at: None,
            until: None,
            duration: None,
            all_day: None,
            description: None,
            clear_description: None,
            start: None,
            end: None,
            visibility: None,
            scope: None,
            web: None,
            preserve_time: None,
        }
    }

    #[test]
    fn render_preview_formats_output() {
        let execution = PromptExecution {
            action: PromptAction::Delete,
            command: EventsCommand::Delete(DeleteArgs {
                id: "abc@2026-03-09".to_string(),
                scope: None,
                json: false,
            }),
            shell_command: "cbzcal events delete --id abc@2026-03-09".to_string(),
            summary_lines: vec![
                "action: delete".to_string(),
                "id: abc@2026-03-09".to_string(),
            ],
        };
        let output = render_preview(&execution);
        assert!(output.contains("解釈結果:"));
        assert!(output.contains("action: delete"));
        assert!(output.contains("実行コマンド:"));
        assert!(output.contains("cbzcal events delete"));
    }

    #[test]
    fn apply_scope_from_arg_all_variants() {
        assert_eq!(apply_scope_from_arg(None), None);
        assert_eq!(
            apply_scope_from_arg(Some(ApplyScopeArg::This)),
            Some(ApplyScope::This)
        );
        assert_eq!(
            apply_scope_from_arg(Some(ApplyScopeArg::After)),
            Some(ApplyScope::After)
        );
        assert_eq!(
            apply_scope_from_arg(Some(ApplyScopeArg::All)),
            Some(ApplyScope::All)
        );
    }

    #[test]
    fn build_delete_execution_creates_correct_command() {
        let mut plan = make_plan(PromptActionWire::Delete);
        plan.id = Some("abc@2026-03-09".to_string());
        plan.scope = Some("this".to_string());
        let execution = build_execution(plan, anchor(), None).expect("execution");
        assert!(matches!(execution.action, PromptAction::Delete));
        assert!(execution.shell_command.contains("delete"));
        assert!(execution.shell_command.contains("--id abc@2026-03-09"));
        assert!(execution.shell_command.contains("--scope this"));
        let summary = execution.summary_lines.join("\n");
        assert!(summary.contains("action: delete"));
        assert!(summary.contains("id: abc@2026-03-09"));
    }

    #[test]
    fn build_delete_execution_errors_without_id() {
        let plan = make_plan(PromptActionWire::Delete);
        let result = build_execution(plan, anchor(), None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("id"));
    }

    #[test]
    fn build_clone_execution_with_title_suffix() {
        let mut plan = make_plan(PromptActionWire::Clone);
        plan.id = Some("abc@2026-03-09".to_string());
        plan.title_suffix = Some("(コピー)".to_string());
        plan.date = Some("tomorrow".to_string());
        plan.at = Some("10:00".to_string());
        plan.duration = Some("1h".to_string());
        let execution = build_execution(plan, anchor(), None).expect("execution");
        assert!(matches!(execution.action, PromptAction::Clone));
        assert!(execution.shell_command.contains("clone"));
        assert!(execution.shell_command.contains("--title-suffix"));
        let summary = execution.summary_lines.join("\n");
        assert!(summary.contains("action: clone"));
        assert!(summary.contains("title_suffix: (コピー)"));
    }

    #[test]
    fn build_clone_execution_errors_without_id() {
        let plan = make_plan(PromptActionWire::Clone);
        let result = build_execution(plan, anchor(), None);
        assert!(result.is_err());
    }

    #[test]
    fn build_list_execution_with_date_range() {
        let mut plan = make_plan(PromptActionWire::List);
        plan.from = Some("2026-03-09".to_string());
        plan.to = Some("2026-03-16".to_string());
        plan.duration = Some("7d".to_string());
        let execution = build_execution(plan, anchor(), None).expect("execution");
        assert!(matches!(execution.action, PromptAction::List));
        assert!(execution.shell_command.contains("list"));
        assert!(execution.shell_command.contains("--from"));
        assert!(execution.shell_command.contains("--to"));
        assert!(execution.shell_command.contains("--for"));
    }

    #[test]
    fn build_list_execution_with_date_only() {
        let mut plan = make_plan(PromptActionWire::List);
        plan.date = Some("today".to_string());
        let execution = build_execution(plan, anchor(), None).expect("execution");
        let summary = execution.summary_lines.join("\n");
        assert!(summary.contains("date: today"));
    }

    #[test]
    fn shell_escape_handles_various_strings() {
        assert_eq!(shell_escape(""), "''");
        assert_eq!(shell_escape("safe-string"), "safe-string");
        assert_eq!(
            shell_escape("2026-03-10T17:30:00+09:00"),
            "'2026-03-10T17:30:00+09:00'"
        );
        assert_eq!(shell_escape("hello world"), "'hello world'");
        assert_eq!(shell_escape("it's"), "'it'\"'\"'s'");
    }

    #[test]
    fn parse_scope_arg_valid_values() {
        assert!(matches!(parse_scope_arg(None), Ok(None)));
        assert!(matches!(
            parse_scope_arg(Some("this")),
            Ok(Some(ApplyScopeArg::This))
        ));
        assert!(matches!(
            parse_scope_arg(Some("after")),
            Ok(Some(ApplyScopeArg::After))
        ));
        assert!(matches!(
            parse_scope_arg(Some("all")),
            Ok(Some(ApplyScopeArg::All))
        ));
    }

    #[test]
    fn parse_scope_arg_rejects_invalid_value() {
        let result = parse_scope_arg(Some("invalid"));
        assert!(result.is_err());
    }

    #[test]
    fn parse_visibility_rejects_invalid_value() {
        let result = parse_visibility(Some("secret"));
        assert!(result.is_err());
    }

    #[test]
    fn infer_visibility_from_prompt_detects_public() {
        assert_eq!(
            infer_visibility_from_prompt("この予定は公開で登録してください"),
            Some("public".to_string())
        );
    }

    #[test]
    fn infer_visibility_public_not_matched_when_negated() {
        assert_eq!(infer_visibility_from_prompt("公開しない"), None);
        assert_eq!(
            infer_visibility_from_prompt("非公開"),
            Some("private".to_string())
        );
    }

    #[test]
    fn infer_date_handles_日付後日() {
        let result = infer_date_from_prompt("明後日15時にミーティング", anchor());
        assert_eq!(result, Some("2026-03-11".to_string()));
    }

    #[test]
    fn infer_date_handles_slash_format() {
        let result = infer_date_from_prompt("3/15の10時から", anchor());
        assert_eq!(result, Some("2026-03-15".to_string()));
    }

    #[test]
    fn infer_date_handles_day_only() {
        let result = infer_date_from_prompt("25日に予定を追加", anchor());
        assert_eq!(result, Some("2026-03-25".to_string()));
    }

    #[test]
    fn infer_date_day_rolls_to_next_month_when_past() {
        // anchor は March 9 なので 5日 はすでに過ぎている → 4月5日
        let result = infer_date_from_prompt("5日に予定を追加", anchor());
        assert_eq!(result, Some("2026-04-05".to_string()));
    }

    #[test]
    fn extract_quoted_handles_various_styles() {
        assert_eq!(
            infer_title_from_prompt("「会議」を追加"),
            Some("会議".to_string())
        );
        assert_eq!(
            infer_title_from_prompt("『報告会』を追加"),
            Some("報告会".to_string())
        );
        assert_eq!(infer_title_from_prompt("何も引用なし"), None);
    }

    #[test]
    fn parse_prompt_plan_strips_markdown_fence() {
        let json = "```json\n{\"action\":\"list\"}\n```";
        let plan = parse_prompt_plan(json).expect("plan");
        assert!(matches!(plan.action, PromptActionWire::List));
    }

    #[test]
    fn parse_prompt_plan_plain_json() {
        let plan = parse_prompt_plan(r#"{"action":"add","title":"打ち合わせ"}"#).expect("plan");
        assert!(matches!(plan.action, PromptActionWire::Add));
        assert_eq!(plan.title.as_deref(), Some("打ち合わせ"));
    }

    #[test]
    fn parse_prompt_plan_invalid_json_returns_error() {
        let result = parse_prompt_plan("not json");
        assert!(result.is_err());
    }

    #[test]
    fn supports_yes_is_false_for_update_and_delete() {
        let make_exec = |action: PromptAction| PromptExecution {
            action,
            command: EventsCommand::List(ListArgs {
                from: None,
                to: None,
                date: None,
                duration: None,
                json: false,
            }),
            shell_command: String::new(),
            summary_lines: Vec::new(),
        };
        assert!(!make_exec(PromptAction::Update).supports_yes());
        assert!(!make_exec(PromptAction::Delete).supports_yes());
        assert!(make_exec(PromptAction::Add).supports_yes());
        assert!(make_exec(PromptAction::Clone).supports_yes());
        assert!(make_exec(PromptAction::List).supports_yes());
    }

    #[test]
    fn resolve_target_window_all_day_returns_midnight() {
        let mut plan = make_plan(PromptActionWire::Clone);
        plan.date = Some("2026-03-15".to_string());
        plan.all_day = Some(true);
        let (start, end) = resolve_target_window(&plan, anchor()).expect("window");
        assert_eq!(
            start.expect("start").to_rfc3339(),
            "2026-03-15T00:00:00+09:00"
        );
        assert_eq!(end.expect("end").to_rfc3339(), "2026-03-16T00:00:00+09:00");
    }

    #[test]
    fn resolve_target_window_with_until() {
        let mut plan = make_plan(PromptActionWire::Clone);
        plan.date = Some("2026-03-15".to_string());
        plan.at = Some("10:00".to_string());
        plan.until = Some("12:00".to_string());
        let (start, end) = resolve_target_window(&plan, anchor()).expect("window");
        assert_eq!(
            start.expect("start").to_rfc3339(),
            "2026-03-15T10:00:00+09:00"
        );
        assert_eq!(end.expect("end").to_rfc3339(), "2026-03-15T12:00:00+09:00");
    }

    #[test]
    fn resolve_target_window_with_duration() {
        let mut plan = make_plan(PromptActionWire::Clone);
        plan.date = Some("2026-03-15".to_string());
        plan.at = Some("14:00".to_string());
        plan.duration = Some("2h".to_string());
        let (start, end) = resolve_target_window(&plan, anchor()).expect("window");
        assert_eq!(
            start.expect("start").to_rfc3339(),
            "2026-03-15T14:00:00+09:00"
        );
        assert_eq!(end.expect("end").to_rfc3339(), "2026-03-15T16:00:00+09:00");
    }

    #[test]
    fn resolve_target_window_no_date_returns_none() {
        let plan = make_plan(PromptActionWire::Clone);
        let (start, end) = resolve_target_window(&plan, anchor()).expect("window");
        assert!(start.is_none());
        assert!(end.is_none());
    }

    #[test]
    fn resolve_target_window_errors_date_at_without_end() {
        let mut plan = make_plan(PromptActionWire::Clone);
        plan.date = Some("2026-03-15".to_string());
        plan.at = Some("10:00".to_string());
        let result = resolve_target_window(&plan, anchor());
        assert!(result.is_err());
    }

    #[test]
    fn resolve_target_window_errors_date_without_at() {
        let mut plan = make_plan(PromptActionWire::Clone);
        plan.date = Some("2026-03-15".to_string());
        // no at, no all_day → error
        let result = resolve_target_window(&plan, anchor());
        assert!(result.is_err());
    }

    #[test]
    fn prompt_mentions_explicit_id_keywords() {
        assert!(prompt_mentions_explicit_id("ID:abc-123 を削除"));
        assert!(prompt_mentions_explicit_id("このid を…"));
        assert!(!prompt_mentions_explicit_id("明日の予定を削除"));
    }

    #[test]
    fn prompt_implies_add_keywords() {
        assert!(prompt_implies_add("明日15時に予定を追加"));
        assert!(prompt_implies_add("打ち合わせを登録してください"));
        assert!(prompt_implies_add("リマインダーを設定する"));
        assert!(!prompt_implies_add("明日の予定を確認"));
    }

    #[test]
    fn looks_like_date_expression_matches_keywords() {
        assert!(looks_like_date_expression("今日", anchor()));
        assert!(looks_like_date_expression("明日", anchor()));
        assert!(looks_like_date_expression("明後日", anchor()));
        assert!(!looks_like_date_expression("abc123", anchor()));
    }

    #[test]
    fn scope_name_all_variants() {
        assert_eq!(scope_name(ApplyScopeArg::This), "this");
        assert_eq!(scope_name(ApplyScopeArg::After), "after");
        assert_eq!(scope_name(ApplyScopeArg::All), "all");
    }

    #[test]
    fn build_update_execution_errors_without_id() {
        let mut plan = make_plan(PromptActionWire::Update);
        plan.title = Some("新しいタイトル".to_string());
        let result = build_execution(plan, anchor(), None);
        assert!(result.is_err());
    }

    #[test]
    fn build_add_execution_errors_without_title() {
        let mut plan = make_plan(PromptActionWire::Add);
        plan.date = Some("tomorrow".to_string());
        let result = build_execution(plan, anchor(), None);
        assert!(result.is_err());
    }

    #[test]
    fn build_add_execution_private_all_day() {
        let mut plan = make_plan(PromptActionWire::Add);
        plan.title = Some("プライベート予定".to_string());
        plan.date = Some("tomorrow".to_string());
        plan.all_day = Some(true);
        plan.visibility = Some("private".to_string());
        let execution = build_execution(plan, anchor(), None).expect("execution");
        let EventsCommand::Add(args) = execution.command else {
            panic!("add")
        };
        assert!(args.private);
        assert!(!args.public);
        assert!(args.all_day);
        assert!(
            execution
                .summary_lines
                .iter()
                .any(|l| l.contains("all_day"))
        );
    }

    #[test]
    fn build_update_with_description_and_web_and_scope() {
        let mut plan = make_plan(PromptActionWire::Update);
        plan.id = Some("abc@2026-03-09".to_string());
        plan.title = Some("新タイトル".to_string());
        plan.description = Some("詳細".to_string());
        plan.scope = Some("after".to_string());
        plan.web = Some(true);
        let execution = build_execution(plan, anchor(), None).expect("execution");
        assert!(matches!(execution.action, PromptAction::Update));
        assert!(execution.shell_command.contains("--scope after"));
        assert!(execution.shell_command.contains("--description"));
        assert!(execution.shell_command.contains("--web"));
        let summary = execution.summary_lines.join("\n");
        assert!(summary.contains("web: true"));
        assert!(summary.contains("scope: after"));
    }

    #[test]
    fn preserve_time_errors_without_existing_event() {
        let mut plan = make_plan(PromptActionWire::Update);
        plan.id = Some("abc@2026-03-09".to_string());
        plan.date = Some("tomorrow".to_string());
        plan.preserve_time = Some(true);
        let result = build_execution(plan, anchor(), None);
        assert!(result.is_err());
    }

    #[test]
    fn preserve_time_errors_without_date() {
        let ev = event();
        let mut plan = make_plan(PromptActionWire::Update);
        plan.id = Some("abc@2026-03-09".to_string());
        plan.preserve_time = Some(true);
        // date は省略
        let result = build_execution(plan, anchor(), Some(&ev));
        assert!(result.is_err());
    }

    #[test]
    fn resolve_update_times_with_at_and_until() {
        let mut plan = make_plan(PromptActionWire::Update);
        plan.id = Some("abc@2026-03-09".to_string());
        plan.date = Some("2026-03-15".to_string());
        plan.at = Some("9:00".to_string());
        plan.until = Some("10:30".to_string());
        let execution = build_execution(plan, anchor(), None).expect("execution");
        let EventsCommand::Update(args) = execution.command else {
            panic!("update")
        };
        assert_eq!(
            args.start.expect("start").to_rfc3339(),
            "2026-03-15T09:00:00+09:00"
        );
        assert_eq!(
            args.end.expect("end").to_rfc3339(),
            "2026-03-15T10:30:00+09:00"
        );
    }

    #[test]
    fn build_clone_with_explicit_start_end() {
        let mut plan = make_plan(PromptActionWire::Clone);
        plan.id = Some("abc@2026-03-09".to_string());
        plan.title = Some("クローン".to_string());
        plan.start = Some("2026-03-15T10:00:00+09:00".to_string());
        plan.end = Some("2026-03-15T11:00:00+09:00".to_string());
        let execution = build_execution(plan, anchor(), None).expect("execution");
        assert!(execution.shell_command.contains("--start"));
        assert!(execution.shell_command.contains("--end"));
        let summary = execution.summary_lines.join("\n");
        assert!(summary.contains("title: クローン"));
    }
}
