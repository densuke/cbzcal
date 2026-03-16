use anyhow::Result;
use chrono::{Datelike, Timelike};
use serde::Serialize;

use crate::datetime::weekday_abbr;
use crate::model::CalendarEvent;

#[derive(Debug, Serialize)]
pub struct EventEnvelope<T: Serialize> {
    pub backend: &'static str,
    pub data: T,
}

#[derive(Debug, Serialize)]
pub struct ApiEvent<'a> {
    pub short_id: String,
    #[serde(flatten)]
    pub event: &'a CalendarEvent,
}

pub fn render_json<T: Serialize>(value: &T) -> Result<String> {
    Ok(serde_json::to_string_pretty(value)?)
}

pub fn render_event(event: &CalendarEvent) -> ApiEvent<'_> {
    ApiEvent {
        short_id: event.short_id(),
        event,
    }
}

pub fn render_events(events: &[CalendarEvent]) -> Vec<ApiEvent<'_>> {
    events.iter().map(render_event).collect()
}

pub fn render_event_result(
    action: &str,
    backend: &'static str,
    event: &CalendarEvent,
    json: bool,
    now: Option<chrono::DateTime<chrono::FixedOffset>>,
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
        out.push_str(&render_single_event(event, now));
        Ok(out)
    }
}

#[allow(clippy::collapsible_if)]
pub fn render_event_list(
    events: &[CalendarEvent],
    now: Option<chrono::DateTime<chrono::FixedOffset>>,
) -> Result<String> {
    if events.is_empty() {
        return Ok("予定はありません".to_string());
    }

    let mut sorted = events.iter().collect::<Vec<_>>();
    sorted.sort_by_key(|event| (event.starts_at, event.ends_at, event.title.as_str()));

    let mut out = String::new();
    let mut current_date = None;
    let mut now_marker_shown = false;

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
            now_marker_shown = false;
        }

        if let Some(now_val) = now {
            if !now_marker_shown && date == now_val.date_naive() && now_val < event.starts_at {
                out.push_str(&format!(
                    "  --- 現在 ({:02}:{:02}) ---\n",
                    now_val.time().hour(),
                    now_val.time().minute()
                ));
                now_marker_shown = true;
            }
        }

        let is_ongoing = now.map(|n| event.is_ongoing(n)).unwrap_or(false);
        let prefix = if is_ongoing { "> " } else { "  " };

        out.push_str(prefix);
        out.push_str(&format_event_time(event));
        out.push_str("  ");
        out.push_str(&sanitize_terminal_output(&event.title));
        out.push(' ');
        out.push('[');
        out.push_str(&sanitize_terminal_output(&event.short_id()));
        out.push(']');
        out.push('\n');

        if is_ongoing {
            now_marker_shown = true;
        }
    }

    if let Some(now_val) = now {
        if !now_marker_shown && current_date == Some(now_val.date_naive()) {
            out.push_str(&format!(
                "  --- 現在 ({:02}:{:02}) ---\n",
                now_val.time().hour(),
                now_val.time().minute()
            ));
        }
    }

    if out.ends_with('\n') {
        out.pop();
    }

    Ok(out)
}

pub fn render_single_event(
    event: &CalendarEvent,
    now: Option<chrono::DateTime<chrono::FixedOffset>>,
) -> String {
    let date = event.starts_at.date_naive();
    let is_ongoing = now.map(|n| event.is_ongoing(n)).unwrap_or(false);
    let prefix = if is_ongoing { "> " } else { "  " };

    format!(
        "  {:04}-{:02}-{:02} ({})\n{}  {}  {} [{}]",
        date.year(),
        date.month(),
        date.day(),
        weekday_abbr(date.weekday()),
        prefix,
        format_event_time(event),
        sanitize_terminal_output(&event.title),
        sanitize_terminal_output(&event.short_id()),
    )
}

pub fn sanitize_terminal_output(input: &str) -> String {
    // 改行(\n, \r)とタブ(\t)以外の制御文字(ASCII < 0x20 や 0x7F 等)を除去します
    // 特にエスケープ(0x1B)を除去することでターミナル制御コードインジェクションを防ぎます
    input
        .chars()
        .filter(|c| {
            let u = *c as u32;
            !(u < 0x20 && *c != '\n' && *c != '\r' && *c != '\t') && u != 0x7F
        })
        .collect()
}

pub fn format_event_time(event: &CalendarEvent) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::EventVisibility;
    use chrono::{FixedOffset, TimeZone};

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

        let rendered = render_event_list(&events, None).expect("render");
        assert_eq!(
            rendered,
            "2026-03-09 (Mon)\n  13:30-14:30  サンプル設定 [3096840@2026-03-09]\n\n2026-03-10 (Tue)\n  終日  休み [3096808@2026-03-10]"
        );
    }

    #[test]
    fn renders_now_marker_and_highlight() {
        let jst = FixedOffset::east_opt(9 * 60 * 60).expect("jst");
        let now = jst
            .with_ymd_and_hms(2026, 3, 9, 14, 0, 0)
            .single()
            .expect("now");
        let events = vec![
            CalendarEvent {
                id: "e1@2026-03-09".to_string(),
                title: "前".to_string(),
                description: None,
                starts_at: jst.with_ymd_and_hms(2026, 3, 9, 9, 0, 0).single().unwrap(),
                ends_at: jst.with_ymd_and_hms(2026, 3, 9, 10, 0, 0).single().unwrap(),
                attendees: Vec::new(),
                facility: None,
                calendar: None,
                visibility: EventVisibility::Public,
                version: 1,
            },
            CalendarEvent {
                id: "e2@2026-03-09".to_string(),
                title: "今".to_string(),
                description: None,
                starts_at: jst
                    .with_ymd_and_hms(2026, 3, 9, 13, 30, 0)
                    .single()
                    .unwrap(),
                ends_at: jst
                    .with_ymd_and_hms(2026, 3, 9, 14, 30, 0)
                    .single()
                    .unwrap(),
                attendees: Vec::new(),
                facility: None,
                calendar: None,
                visibility: EventVisibility::Public,
                version: 1,
            },
            CalendarEvent {
                id: "e3@2026-03-09".to_string(),
                title: "後".to_string(),
                description: None,
                starts_at: jst.with_ymd_and_hms(2026, 3, 9, 16, 0, 0).single().unwrap(),
                ends_at: jst.with_ymd_and_hms(2026, 3, 9, 17, 0, 0).single().unwrap(),
                attendees: Vec::new(),
                facility: None,
                calendar: None,
                visibility: EventVisibility::Public,
                version: 1,
            },
        ];

        let rendered = render_event_list(&events, Some(now)).expect("render");
        assert!(rendered.contains("> 13:30-14:30  今 [e2@2026-03-09]"));

        let now_between = jst
            .with_ymd_and_hms(2026, 3, 9, 11, 0, 0)
            .single()
            .expect("now");
        let rendered_between = render_event_list(&events, Some(now_between)).expect("render");
        assert!(rendered_between.contains("--- 現在 (11:00) ---"));
    }

    #[test]
    fn sanitizes_terminal_control_characters() {
        assert_eq!(sanitize_terminal_output("Normal Text"), "Normal Text");
        assert_eq!(
            sanitize_terminal_output("Escape\x1b[31mCode\x1b[0m"),
            "Escape[31mCode[0m"
        );
        assert_eq!(
            sanitize_terminal_output("Tab\tAnd\nNewline"),
            "Tab\tAnd\nNewline"
        );
        assert_eq!(sanitize_terminal_output("Backspace\x08"), "Backspace");
    }

    #[test]
    fn renders_empty_event_list() {
        let rendered = render_event_list(&[], None).expect("render");
        assert_eq!(rendered, "予定はありません");
    }

    #[test]
    fn render_event_result_text_contains_action() {
        let jst = FixedOffset::east_opt(9 * 60 * 60).expect("jst");
        let event = CalendarEvent {
            id: "sEID=100&UID=1&GID=1&Date=da.2026.3.9&BDate=da.2026.3.9".to_string(),
            title: "テスト".to_string(),
            description: None,
            starts_at: jst.with_ymd_and_hms(2026, 3, 9, 10, 0, 0).single().unwrap(),
            ends_at: jst.with_ymd_and_hms(2026, 3, 9, 11, 0, 0).single().unwrap(),
            attendees: Vec::new(),
            facility: None,
            calendar: None,
            visibility: EventVisibility::Public,
            version: 1,
        };
        let result =
            render_event_result("追加しました", "stub", &event, false, None).expect("render");
        assert!(result.contains("追加しました"));
        assert!(result.contains("テスト"));
    }

    #[test]
    fn format_event_time_all_day() {
        let jst = FixedOffset::east_opt(9 * 60 * 60).expect("jst");
        let event = CalendarEvent {
            id: "1".to_string(),
            title: "終日予定".to_string(),
            description: None,
            starts_at: jst.with_ymd_and_hms(2026, 3, 9, 0, 0, 0).single().unwrap(),
            ends_at: jst.with_ymd_and_hms(2026, 3, 10, 0, 0, 0).single().unwrap(),
            attendees: Vec::new(),
            facility: None,
            calendar: None,
            visibility: EventVisibility::Public,
            version: 1,
        };
        assert_eq!(format_event_time(&event), "終日");
    }
}
