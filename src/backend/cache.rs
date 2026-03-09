use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::{CalendarEvent, CloneOverrides, EventPatch, NewEvent};
use super::{ApplyScope, CalendarBackend, ListQuery};

#[derive(Debug, Serialize, Deserialize)]
struct CachedEvents {
    timestamp: SystemTime,
    query: ListQuery,
    events: Vec<CalendarEvent>,
}

pub struct CachingBackend {
    inner: Box<dyn CalendarBackend>,
    cache_path: PathBuf,
    ttl: Duration,
    disabled: bool,
}

impl CachingBackend {
    pub fn new(inner: Box<dyn CalendarBackend>, cache_path: PathBuf, disabled: bool) -> Self {
        Self {
            inner,
            cache_path,
            ttl: Duration::from_secs(3600), // 1 hour
            disabled,
        }
    }

    fn load_cache(&self, query: &ListQuery) -> Option<Vec<CalendarEvent>> {
        if self.disabled {
            return None;
        }

        let content = fs::read_to_string(&self.cache_path).ok()?;
        let cached: CachedEvents = serde_json::from_str(&content).ok()?;

        let now = SystemTime::now();
        let elapsed = now.duration_since(cached.timestamp).ok()?;

        if elapsed < self.ttl && cached.query == *query {
            Some(cached.events)
        } else {
            None
        }
    }

    fn save_cache(&self, query: &ListQuery, events: &[CalendarEvent]) -> Result<()> {
        if self.disabled {
            return Ok(());
        }

        let cached = CachedEvents {
            timestamp: SystemTime::now(),
            query: query.clone(),
            events: events.to_vec(),
        };

        if let Some(parent) = self.cache_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let content = serde_json::to_string(&cached)?;
        fs::write(&self.cache_path, content)
            .with_context(|| format!("キャッシュを保存できません: {}", self.cache_path.display()))?;

        Ok(())
    }

    fn invalidate_cache(&self) {
        let _ = fs::remove_file(&self.cache_path);
    }
}

impl CalendarBackend for CachingBackend {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    fn list_events(&mut self, query: ListQuery) -> Result<Vec<CalendarEvent>> {
        if let Some(events) = self.load_cache(&query) {
            return Ok(events);
        }

        let events = self.inner.list_events(query.clone())?;
        self.save_cache(&query, &events)?;
        Ok(events)
    }

    fn add_event(&mut self, input: NewEvent) -> Result<CalendarEvent> {
        let result = self.inner.add_event(input);
        if result.is_ok() {
            self.invalidate_cache();
        }
        result
    }

    fn update_event(
        &mut self,
        id: &str,
        patch: EventPatch,
        scope: Option<ApplyScope>,
    ) -> Result<CalendarEvent> {
        let result = self.inner.update_event(id, patch, scope);
        if result.is_ok() {
            self.invalidate_cache();
        }
        result
    }

    fn clone_event(&mut self, id: &str, overrides: CloneOverrides) -> Result<CalendarEvent> {
        let result = self.inner.clone_event(id, overrides);
        if result.is_ok() {
            self.invalidate_cache();
        }
        result
    }

    fn delete_event(&mut self, id: &str, scope: Option<ApplyScope>) -> Result<CalendarEvent> {
        let result = self.inner.delete_event(id, scope);
        if result.is_ok() {
            self.invalidate_cache();
        }
        result
    }

    fn event_web_url(&mut self, id: &str) -> Result<String> {
        self.inner.event_web_url(id)
    }

    fn drain_notices(&mut self) -> Vec<String> {
        self.inner.drain_notices()
    }
}
