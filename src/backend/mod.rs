pub mod cybozu_html;
pub mod fixture;

use anyhow::Result;
use chrono::{DateTime, FixedOffset};

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

pub trait CalendarBackend {
    fn name(&self) -> &'static str;
    fn list_events(&mut self, query: ListQuery) -> Result<Vec<CalendarEvent>>;
    fn add_event(&mut self, input: NewEvent) -> Result<CalendarEvent>;
    fn update_event(&mut self, id: &str, patch: EventPatch) -> Result<CalendarEvent>;
    fn clone_event(&mut self, id: &str, overrides: CloneOverrides) -> Result<CalendarEvent>;
    fn delete_event(&mut self, id: &str) -> Result<()>;
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
