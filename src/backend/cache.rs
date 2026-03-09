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

        if elapsed < self.ttl && cached.query.contains(query) {
            let filtered = cached
                .events
                .into_iter()
                .filter(|event| event.overlaps(query.from, query.to))
                .collect();
            Some(filtered)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::EventVisibility;
    use chrono::{DateTime, FixedOffset};
    use std::sync::{Arc, Mutex};

    struct CounterBackend {
        calls: Arc<Mutex<usize>>,
        events: Vec<CalendarEvent>,
    }

    impl CalendarBackend for CounterBackend {
        fn name(&self) -> &'static str {
            "counter"
        }
        fn list_events(&mut self, _query: ListQuery) -> Result<Vec<CalendarEvent>> {
            *self.calls.lock().unwrap() += 1;
            Ok(self.events.clone())
        }
        fn add_event(&mut self, _input: NewEvent) -> Result<CalendarEvent> {
            unimplemented!()
        }
        fn update_event(
            &mut self,
            _id: &str,
            _patch: EventPatch,
            _scope: Option<ApplyScope>,
        ) -> Result<CalendarEvent> {
            unimplemented!()
        }
        fn clone_event(&mut self, _id: &str, _overrides: CloneOverrides) -> Result<CalendarEvent> {
            unimplemented!()
        }
        fn delete_event(&mut self, _id: &str, _scope: Option<ApplyScope>) -> Result<CalendarEvent> {
            unimplemented!()
        }
        fn event_web_url(&mut self, _id: &str) -> Result<String> {
            unimplemented!()
        }
        fn drain_notices(&mut self) -> Vec<String> {
            Vec::new()
        }
    }

    fn ts(input: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(input).expect("timestamp")
    }

    fn dummy_event(id: &str, start: &str, end: &str) -> CalendarEvent {
        CalendarEvent {
            id: id.to_string(),
            title: "T".to_string(),
            description: None,
            starts_at: ts(start),
            ends_at: ts(end),
            attendees: Vec::new(),
            facility: None,
            calendar: None,
            visibility: EventVisibility::Public,
            version: 1,
        }
    }

    #[test]
    fn cache_hit_on_sub_range() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let cache_path = temp.path().join("cache.json");

        let calls = Arc::new(Mutex::new(0));
        let event = dummy_event("1", "2026-03-09T09:00:00+09:00", "2026-03-09T10:00:00+09:00");

        let inner = CounterBackend {
            calls: calls.clone(),
            events: vec![event],
        };

        let mut backend = CachingBackend::new(Box::new(inner), cache_path, false);

        // First call: 1 week (Default)
        let q1 = ListQuery {
            from: Some(ts("2026-03-09T00:00:00+09:00")),
            to: Some(ts("2026-03-16T00:00:00+09:00")),
        };
        let events1 = backend.list_events(q1)?;
        assert_eq!(events1.len(), 1);
        assert_eq!(*calls.lock().unwrap(), 1);

        // Second call: today only (sub-range)
        let q2 = ListQuery {
            from: Some(ts("2026-03-09T00:00:00+09:00")),
            to: Some(ts("2026-03-10T00:00:00+09:00")),
        };
        let events2 = backend.list_events(q2)?;
        assert_eq!(events2.len(), 1);
        assert_eq!(*calls.lock().unwrap(), 1); // Cache hit!

        // Third call: tomorrow only (sub-range, but no events overlap)
        let q3 = ListQuery {
            from: Some(ts("2026-03-10T00:00:00+09:00")),
            to: Some(ts("2026-03-11T00:00:00+09:00")),
        };
        let events3 = backend.list_events(q3)?;
        assert_eq!(events3.len(), 0);
        assert_eq!(*calls.lock().unwrap(), 1); // Cache hit!

        // Fourth call: Yesterday (out of range)
        let q4 = ListQuery {
            from: Some(ts("2026-03-08T00:00:00+09:00")),
            to: Some(ts("2026-03-09T00:00:00+09:00")),
        };
        let _ = backend.list_events(q4)?;
        assert_eq!(*calls.lock().unwrap(), 2); // Cache miss

        Ok(())
    }
}
