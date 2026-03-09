use std::io::{self, Write};
use std::process::Command as ProcessCommand;

use anyhow::{Result, bail};
use chrono::{Datelike, FixedOffset, NaiveDate, TimeZone, Timelike};
use serde::Serialize;

use crate::{
    backend::{ApplyScope, CalendarBackend, CybozuHtmlBackend, ListQuery, build_backend},
    cli::{ApplyScopeArg, Cli, Command, EventsCommand, ResolvedEventsArgs},
    config::AppConfig,
    model::CalendarEvent,
    prompt::{apply_scope_from_arg, plan_prompt, render_preview},
};

#[derive(Debug, Serialize)]
struct EventEnvelope<T: Serialize> {
    backend: &'static str,
    data: T,
}

#[derive(Debug, Serialize)]
struct ApiEvent<'a> {
    short_id: String,
    #[serde(flatten)]
    event: &'a CalendarEvent,
}

pub fn execute(cli: Cli) -> Result<String> {
    let loaded = AppConfig::load_with_resolution(cli.config.as_deref())?;
    let verbose = cli.verbose;

    match cli.command {
        Command::Doctor => render_json(&loaded.config.doctor_report(&loaded.path)),
        Command::ProbeLogin => {
            let cybozu = loaded
                .config
                .cybozu_html
                .clone()
                .ok_or_else(|| anyhow::anyhow!("[cybozu-html] セクションがありません"))?;
            render_json(&CybozuHtmlBackend::probe_login(cybozu)?)
        }
        Command::Events(events) => {
            let mut backend = build_backend(&loaded.config)?;
            let output = match events.resolve()? {
                ResolvedEventsArgs::Prompt(prompt) => {
                    let existing_event = extract_short_id_hint(&prompt.prompt)
                        .map(|id| find_event_by_id(backend.as_mut(), &id))
                        .transpose()?;
                    let execution = plan_prompt(
                        &loaded.config,
                        &prompt.prompt,
                        None,
                        existing_event.as_ref(),
                    )?;
                    if prompt.yes && !execution.supports_yes() {
                        bail!(
                            "`--yes` は prompt モードの list/add/clone でのみ使えます。update/delete では確認が必須です"
                        );
                    }
                    let preview = render_preview(&execution);
                    println!("{preview}");
                    if !prompt.yes && !confirm_execution()? {
                        emit_verbose_notices(verbose, backend.drain_notices());
                        return Ok("キャンセルしました".to_string());
                    }

                    execute_events_command(backend.as_mut(), execution.command)?
                }
                ResolvedEventsArgs::Command(command) => {
                    execute_events_command(backend.as_mut(), command)?
                }
            };
            emit_verbose_notices(verbose, backend.drain_notices());
            Ok(output)
        }
    }
}

fn execute_events_command(
    backend: &mut dyn CalendarBackend,
    command: EventsCommand,
) -> Result<String> {
    match command {
        EventsCommand::List(args) => {
            let query: ListQuery = args.query()?;
            let events = backend.list_events(query.with_default_window())?;
            if args.json {
                render_json(&EventEnvelope {
                    backend: backend.name(),
                    data: render_events(&events),
                })
            } else {
                render_event_list(&events)
            }
        }
        EventsCommand::Add(args) => {
            let event = backend.add_event(args.new_event()?)?;
            render_event_result("追加しました", backend.name(), &event, args.json)
        }
        EventsCommand::Update(args) => {
            let patch = args.patch()?;
            let scope = args.scope.map(into_apply_scope);
            if patch.is_empty() && !args.web {
                bail!(
                    "更新対象がありません。少なくとも 1 つの変更オプションを指定するか `--web` を付けてください"
                );
            }
            if patch.is_empty() {
                let url = backend.event_web_url(&args.id)?;
                open_in_browser(&url)?;
                Ok(format!("ブラウザで開きました\n  {url}"))
            } else {
                let event = backend.update_event(&args.id, patch, scope)?;
                if args.web {
                    let url = backend.event_web_url(&event.id)?;
                    open_in_browser(&url)?;
                }
                render_event_result("更新しました", backend.name(), &event, args.json)
            }
        }
        EventsCommand::Clone(args) => {
            let overrides = args.overrides()?;
            let event = backend.clone_event(&args.id, overrides)?;
            render_event_result("複製しました", backend.name(), &event, args.json)
        }
        EventsCommand::Delete(args) => {
            if args.id.is_empty() {
                bail!("削除対象の ID が空です");
            }
            let event = backend.delete_event(&args.id, args.scope.map(into_apply_scope))?;
            render_event_result("削除しました", backend.name(), &event, args.json)
        }
    }
}

fn confirm_execution() -> Result<bool> {
    print!("実行しますか? [y/N] ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(matches!(
        input.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn render_json<T: Serialize>(value: &T) -> Result<String> {
    Ok(serde_json::to_string_pretty(value)?)
}

fn emit_verbose_notices(verbose: u8, notices: Vec<String>) {
    if verbose == 0 {
        return;
    }
    for notice in notices {
        eprintln!("[verbose] {notice}");
    }
}

fn into_apply_scope(scope: ApplyScopeArg) -> ApplyScope {
    apply_scope_from_arg(Some(scope)).expect("scope")
}

fn open_in_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut cmd = ProcessCommand::new("open");
        cmd.arg(url);
        cmd
    };

    #[cfg(target_os = "linux")]
    let mut command = {
        let mut cmd = ProcessCommand::new("xdg-open");
        cmd.arg(url);
        cmd
    };

    #[cfg(target_os = "windows")]
    let mut command = {
        let mut cmd = ProcessCommand::new("cmd");
        cmd.args(["/C", "start", "", url]);
        cmd
    };

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        bail!("この OS では `--web` に未対応です");
    }

    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    {
        let status = command.status()?;
        if !status.success() {
            bail!("ブラウザ起動に失敗しました: {status}");
        }
        Ok(())
    }
}

fn render_event(event: &CalendarEvent) -> ApiEvent<'_> {
    ApiEvent {
        short_id: event.short_id(),
        event,
    }
}

fn render_events(events: &[CalendarEvent]) -> Vec<ApiEvent<'_>> {
    events.iter().map(render_event).collect()
}

fn render_event_result(
    action: &str,
    backend: &'static str,
    event: &CalendarEvent,
    json: bool,
) -> Result<String> {
    if json {
        render_json(&EventEnvelope {
            backend,
            data: render_event(event),
        })
    } else {
        let mut out = String::new();
        out.push_str(action);
        out.push('\n');
        out.push_str(&render_single_event(event));
        Ok(out)
    }
}

fn render_event_list(events: &[CalendarEvent]) -> Result<String> {
    if events.is_empty() {
        return Ok("予定はありません".to_string());
    }

    let mut sorted = events.iter().collect::<Vec<_>>();
    sorted.sort_by_key(|event| (event.starts_at, event.ends_at, event.title.as_str()));

    let mut out = String::new();
    let mut current_date = None;
    for event in sorted {
        let date = event.starts_at.date_naive();
        if current_date != Some(date) {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&format!(
                "{:04}-{:02}-{:02} ({})\n",
                date.year(),
                date.month(),
                date.day(),
                weekday_abbr(date.weekday()),
            ));
            current_date = Some(date);
        }

        out.push_str("  ");
        out.push_str(&format_event_time(event));
        out.push_str("  ");
        out.push_str(&event.title);
        out.push(' ');
        out.push('[');
        out.push_str(&event.short_id());
        out.push(']');
        out.push('\n');
    }

    if out.ends_with('\n') {
        out.pop();
    }

    Ok(out)
}

fn render_single_event(event: &CalendarEvent) -> String {
    let date = event.starts_at.date_naive();
    format!(
        "  {:04}-{:02}-{:02} ({})\n  {}  {} [{}]",
        date.year(),
        date.month(),
        date.day(),
        weekday_abbr(date.weekday()),
        format_event_time(event),
        event.title,
        event.short_id(),
    )
}

fn format_event_time(event: &CalendarEvent) -> String {
    let duration = event.ends_at - event.starts_at;
    if duration.num_days() == 1
        && event.starts_at.time().hour() == 0
        && event.starts_at.time().minute() == 0
        && event.ends_at.time().hour() == 0
        && event.ends_at.time().minute() == 0
    {
        return "終日".to_string();
    }

    if event.starts_at.date_naive() == event.ends_at.date_naive() {
        return format!(
            "{:02}:{:02}-{:02}:{:02}",
            event.starts_at.time().hour(),
            event.starts_at.time().minute(),
            event.ends_at.time().hour(),
            event.ends_at.time().minute(),
        );
    }

    format!(
        "{:02}/{:02} {:02}:{:02} -> {:02}/{:02} {:02}:{:02}",
        event.starts_at.month(),
        event.starts_at.day(),
        event.starts_at.time().hour(),
        event.starts_at.time().minute(),
        event.ends_at.month(),
        event.ends_at.day(),
        event.ends_at.time().hour(),
        event.ends_at.time().minute(),
    )
}

fn weekday_abbr(weekday: chrono::Weekday) -> &'static str {
    match weekday {
        chrono::Weekday::Mon => "Mon",
        chrono::Weekday::Tue => "Tue",
        chrono::Weekday::Wed => "Wed",
        chrono::Weekday::Thu => "Thu",
        chrono::Weekday::Fri => "Fri",
        chrono::Weekday::Sat => "Sat",
        chrono::Weekday::Sun => "Sun",
    }
}

fn extract_short_id_hint(prompt: &str) -> Option<String> {
    prompt
        .split_whitespace()
        .find(|token| token.contains('@'))
        .map(|token| {
            token
                .trim_matches(|ch: char| matches!(ch, '"' | '\'' | '「' | '」' | '。' | '、'))
                .to_string()
        })
}

fn find_event_by_id(backend: &mut dyn CalendarBackend, id: &str) -> Result<CalendarEvent> {
    let date = extract_date_from_event_identifier(id)
        .ok_or_else(|| anyhow::anyhow!("ID から日付を解決できませんでした: {id}"))?;
    let jst = FixedOffset::east_opt(9 * 60 * 60).expect("jst");
    let from = jst
        .with_ymd_and_hms(date.year(), date.month(), date.day(), 0, 0, 0)
        .single()
        .expect("start of day");
    let to = from + chrono::TimeDelta::days(1);
    let events = backend.list_events(ListQuery {
        from: Some(from),
        to: Some(to),
    })?;
    events
        .into_iter()
        .find(|event| event.id == id || event.short_id() == id)
        .ok_or_else(|| anyhow::anyhow!("対象予定が見つかりませんでした: {id}"))
}

fn extract_date_from_event_identifier(id: &str) -> Option<NaiveDate> {
    if let Some((_, date)) = id.split_once('@') {
        return NaiveDate::parse_from_str(date, "%Y-%m-%d").ok();
    }

    let url = reqwest::Url::parse(&format!("https://example.invalid/?{id}")).ok()?;
    for (key, value) in url.query_pairs() {
        if key == "Date" {
            return parse_da_date(&value);
        }
    }
    None
}

fn parse_da_date(value: &str) -> Option<NaiveDate> {
    let value = value.strip_prefix("da.")?;
    let mut parts = value.split('.');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    NaiveDate::from_ymd_opt(year, month, day)
}

#[cfg(test)]
mod tests {
    use chrono::{FixedOffset, NaiveDate, TimeZone};

    use super::*;
    use crate::model::EventVisibility;

    #[test]
    fn renders_human_readable_event_list() {
        let jst = FixedOffset::east_opt(9 * 60 * 60).expect("jst");
        let events = vec![
            CalendarEvent {
                id: "sEID=3096840&UID=379&GID=183&Date=da.2026.3.9&BDate=da.2026.3.9".to_string(),
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
            },
            CalendarEvent {
                id: "sEID=3096808&UID=379&GID=183&Date=da.2026.3.10&BDate=da.2026.3.10".to_string(),
                title: "休み".to_string(),
                description: None,
                starts_at: jst
                    .with_ymd_and_hms(2026, 3, 10, 0, 0, 0)
                    .single()
                    .expect("start"),
                ends_at: jst
                    .with_ymd_and_hms(2026, 3, 11, 0, 0, 0)
                    .single()
                    .expect("end"),
                attendees: Vec::new(),
                facility: None,
                calendar: None,
                visibility: EventVisibility::Public,
                version: 1,
            },
        ];

        let rendered = render_event_list(&events).expect("render");
        assert_eq!(
            rendered,
            "2026-03-09 (Mon)\n  13:30-14:30  サンプル設定 [3096840@2026-03-09]\n\n2026-03-10 (Tue)\n  終日  休み [3096808@2026-03-10]"
        );
    }

    #[test]
    fn renders_human_readable_single_event_result() {
        let jst = FixedOffset::east_opt(9 * 60 * 60).expect("jst");
        let event = CalendarEvent {
            id: "sEID=3096840&UID=379&GID=183&Date=da.2026.3.9&BDate=da.2026.3.9".to_string(),
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
        };

        let rendered =
            render_event_result("追加しました", "fixture", &event, false).expect("render");
        assert_eq!(
            rendered,
            "追加しました\n  2026-03-09 (Mon)\n  13:30-14:30  サンプル設定 [3096840@2026-03-09]"
        );
    }

    #[test]
    fn extracts_date_from_short_id() {
        assert_eq!(
            extract_date_from_event_identifier("3096840@2026-03-09"),
            Some(NaiveDate::from_ymd_opt(2026, 3, 9).expect("date"))
        );
    }
}
