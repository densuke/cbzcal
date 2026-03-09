pub mod cybozu_html;
pub mod fixture;

use anyhow::{Result, bail};
use chrono::{DateTime, Days, FixedOffset, TimeZone, Utc};

use crate::{
    config::{AppConfig, BackendKind},
    model::{CalendarEvent, CloneOverrides, EventPatch, NewEvent},
};

pub use cybozu_html::CybozuHtmlBackend;
pub use fixture::FixtureBackend;

#[derive(Debug, Clone)]
pub struct ListQuery {
    pub from: Option<DateTime<FixedOffset>>,
    pub to: Option<DateTime<FixedOffset>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyScope {
    This,
    After,
    All,
}

impl ListQuery {
    pub fn with_default_window(self) -> Self {
        self.with_default_window_from(current_jst_midnight())
    }

    fn with_default_window_from(self, anchor: DateTime<FixedOffset>) -> Self {
        match (self.from, self.to) {
            (Some(from), Some(to)) => Self {
                from: Some(from),
                to: Some(to),
            },
            (Some(from), None) => Self {
                from: Some(from),
                to: Some(from + chrono::TimeDelta::days(7)),
            },
            (None, Some(to)) => Self {
                from: Some(to - chrono::TimeDelta::days(7)),
                to: Some(to),
            },
            (None, None) => Self {
                from: Some(anchor),
                to: Some(anchor + chrono::TimeDelta::days(7)),
            },
        }
    }
}

fn current_jst_midnight() -> DateTime<FixedOffset> {
    let offset = FixedOffset::east_opt(9 * 60 * 60).expect("valid JST offset");
    let today = Utc::now().with_timezone(&offset).date_naive();
    let midnight = today
        .checked_add_days(Days::new(0))
        .expect("same date")
        .and_hms_opt(0, 0, 0)
        .expect("midnight");
    offset
        .from_local_datetime(&midnight)
        .single()
        .expect("valid local midnight")
}

pub trait CalendarBackend {
    fn name(&self) -> &'static str;
    fn list_events(&mut self, query: ListQuery) -> Result<Vec<CalendarEvent>>;
    fn add_event(&mut self, input: NewEvent) -> Result<CalendarEvent>;
    fn update_event(
        &mut self,
        id: &str,
        patch: EventPatch,
        scope: Option<ApplyScope>,
    ) -> Result<CalendarEvent>;
    fn clone_event(&mut self, id: &str, overrides: CloneOverrides) -> Result<CalendarEvent>;
    fn delete_event(&mut self, id: &str, scope: Option<ApplyScope>) -> Result<CalendarEvent>;
    fn event_web_url(&mut self, _id: &str) -> Result<String> {
        bail!("このバックエンドは `--web` に未対応です")
    }
    fn drain_notices(&mut self) -> Vec<String> {
        Vec::new()
    }
}

pub fn build_backend(config: &AppConfig) -> Result<Box<dyn CalendarBackend>> {
    match config.backend {
        BackendKind::Fixture => {
            let fixture = config
                .fixture
                .clone()
                .expect("fixture backend requires fixture config");
            Ok(Box::new(FixtureBackend::open(fixture.path)?))
        }
        BackendKind::CybozuHtml => {
            let cybozu = config
                .cybozu_html
                .clone()
                .expect("cybozu-html backend requires config");
            Ok(Box::new(CybozuHtmlBackend::new(cybozu)?))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(input: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(input).expect("timestamp")
    }

    #[test]
    fn defaults_to_one_week_from_anchor_when_unbounded() {
        let query = ListQuery {
            from: None,
            to: None,
        }
        .with_default_window_from(ts("2026-03-09T00:00:00+09:00"));

        assert_eq!(query.from, Some(ts("2026-03-09T00:00:00+09:00")));
        assert_eq!(query.to, Some(ts("2026-03-16T00:00:00+09:00")));
    }

    #[test]
    fn extends_from_only_query_to_one_week() {
        let query = ListQuery {
            from: Some(ts("2026-03-12T09:00:00+09:00")),
            to: None,
        }
        .with_default_window_from(ts("2026-03-09T00:00:00+09:00"));

        assert_eq!(query.from, Some(ts("2026-03-12T09:00:00+09:00")));
        assert_eq!(query.to, Some(ts("2026-03-19T09:00:00+09:00")));
    }

    #[test]
    fn extends_to_only_query_backwards_one_week() {
        let query = ListQuery {
            from: None,
            to: Some(ts("2026-03-16T00:00:00+09:00")),
        }
        .with_default_window_from(ts("2026-03-09T00:00:00+09:00"));

        assert_eq!(query.from, Some(ts("2026-03-09T00:00:00+09:00")));
        assert_eq!(query.to, Some(ts("2026-03-16T00:00:00+09:00")));
    }
}
