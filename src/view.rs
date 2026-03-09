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
            now_marker_shown = false; // Reset marker for each day
        }

        // Show "Now" marker if we are at today and now is between previous event end and this event start
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
        out.push_str(&event.title);
        out.push(' ');
        out.push('[');
        out.push_str(&event.short_id());
        out.push(']');
        out.push('\n');

        // Show "Now" marker if ongoing
        if is_ongoing && !now_marker_shown {
            // If ongoing, the marker is effectively "inside" or "after" it. 
            // We'll skip a separate "Now" line if it's already highlighted as ongoing.
            now_marker_shown = true; 
        }

        // Show "Now" marker if just passed
        if let Some(now_val) = now {
            if !now_marker_shown && date == now_val.date_naive() && event.is_passed(now_val) {
                // Check next event or end of day? 
                // For simplicity, we'll check it in the next loop iteration or at the end of the day.
            }
        }
    }

    // Handle now marker if it's at the end of the day (all events passed)
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
        event.title,
        event.short_id(),
    )
}-{:02}-{:02} ({})\n  {}  {} [{}]",
        date.year(),
        date.month(),
        date.day(),
        weekday_abbr(date.weekday()),
        format_event_time(event),
        event.title,
        event.short_id(),
    )
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
        let now = jst.with_ymd_and_hms(2026, 3, 9, 14, 0, 0).single().expect("now");
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
                starts_at: jst.with_ymd_and_hms(2026, 3, 9, 13, 30, 0).single().unwrap(),
                ends_at: jst.with_ymd_and_hms(2026, 3, 9, 14, 30, 0).single().unwrap(),
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
        // Marker should be merged if ongoing event exists
        assert!(rendered.contains("> 13:30-14:30  今 [e2@2026-03-09]"));
        
        // Let's test marker between events
        let now_between = jst.with_ymd_and_hms(2026, 3, 9, 11, 0, 0).single().expect("now");
        let rendered_between = render_event_list(&events, Some(now_between)).expect("render");
        assert!(rendered_between.contains("--- 現在 (11:00) ---"));
    }
}
