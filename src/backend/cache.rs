use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::{ApplyScope, CalendarBackend, ListQuery};
use crate::model::{CalendarEvent, CloneOverrides, EventPatch, NewEvent};

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
    deferred_notices: Vec<String>,
}

impl CachingBackend {
    pub fn new(inner: Box<dyn CalendarBackend>, cache_path: PathBuf, disabled: bool) -> Self {
        Self {
            inner,
            cache_path,
            ttl: Duration::from_secs(3600), // 1 hour
            disabled,
            deferred_notices: Vec::new(),
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
        fs::write(&self.cache_path, content).with_context(|| {
            format!("キャッシュを保存できません: {}", self.cache_path.display())
        })?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(metadata) = fs::metadata(&self.cache_path) {
                let mut perms = metadata.permissions();
                perms.set_mode(0o600);
                let _ = fs::set_permissions(&self.cache_path, perms);
            }
        }

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
        self.deferred_notices.extend(self.inner.drain_notices());
        self.save_cache(&query, &events)?;
        Ok(events)
    }

    fn add_event(&mut self, input: NewEvent) -> Result<CalendarEvent> {
        let result = self.inner.add_event(input);
        self.deferred_notices.extend(self.inner.drain_notices());
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
        self.deferred_notices.extend(self.inner.drain_notices());
        if result.is_ok() {
            self.invalidate_cache();
        }
        result
    }

    fn clone_event(&mut self, id: &str, overrides: CloneOverrides) -> Result<CalendarEvent> {
        let result = self.inner.clone_event(id, overrides);
        self.deferred_notices.extend(self.inner.drain_notices());
        if result.is_ok() {
            self.invalidate_cache();
        }
        result
    }

    fn delete_event(&mut self, id: &str, scope: Option<ApplyScope>) -> Result<CalendarEvent> {
        let result = self.inner.delete_event(id, scope);
        self.deferred_notices.extend(self.inner.drain_notices());
        if result.is_ok() {
            self.invalidate_cache();
        }
        result
    }

    fn event_web_url(&mut self, id: &str) -> Result<String> {
        let result = self.inner.event_web_url(id);
        self.deferred_notices.extend(self.inner.drain_notices());
        result
    }

    fn drain_notices(&mut self) -> Vec<String> {
        std::mem::take(&mut self.deferred_notices)
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
        notices: Vec<String>,
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
            std::mem::take(&mut self.notices)
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
        let event = dummy_event(
            "1",
            "2026-03-09T09:00:00+09:00",
            "2026-03-09T10:00:00+09:00",
        );

        let inner = CounterBackend {
            calls: calls.clone(),
            events: vec![event],
            notices: vec!["backend initialized".to_string()],
        };

        let mut backend = CachingBackend::new(Box::new(inner), cache_path, false);

        // First call: 1 week (Default) -> Cache MISS, backend called
        let q1 = ListQuery {
            from: Some(ts("2026-03-09T00:00:00+09:00")),
            to: Some(ts("2026-03-16T00:00:00+09:00")),
        };
        let events1 = backend.list_events(q1)?;
        assert_eq!(events1.len(), 1);
        assert_eq!(*calls.lock().unwrap(), 1);
        assert_eq!(backend.drain_notices(), vec!["backend initialized"]);

        // Second call: today only (sub-range) -> Cache HIT, backend NOT called
        let q2 = ListQuery {
            from: Some(ts("2026-03-09T00:00:00+09:00")),
            to: Some(ts("2026-03-10T00:00:00+09:00")),
        };
        let events2 = backend.list_events(q2)?;
        assert_eq!(events2.len(), 1);
        assert_eq!(*calls.lock().unwrap(), 1);
        assert!(backend.drain_notices().is_empty()); // No notice from inner backend

        Ok(())
    }

    #[test]
    fn cache_disabled_always_calls_backend() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let cache_path = temp.path().join("cache.json");

        let calls = Arc::new(Mutex::new(0));
        let inner = CounterBackend {
            calls: calls.clone(),
            events: Vec::new(),
            notices: Vec::new(),
        };
        let mut backend = CachingBackend::new(Box::new(inner), cache_path, true); // disabled=true

        let q = ListQuery {
            from: Some(ts("2026-03-09T00:00:00+09:00")),
            to: Some(ts("2026-03-16T00:00:00+09:00")),
        };
        backend.list_events(q.clone())?;
        backend.list_events(q)?;
        assert_eq!(*calls.lock().unwrap(), 2);
        Ok(())
    }

    struct MutBackend {
        list_calls: Arc<Mutex<usize>>,
        events: Vec<CalendarEvent>,
        notices: Vec<String>,
    }

    impl CalendarBackend for MutBackend {
        fn name(&self) -> &'static str {
            "mut"
        }
        fn list_events(&mut self, _query: ListQuery) -> Result<Vec<CalendarEvent>> {
            *self.list_calls.lock().unwrap() += 1;
            Ok(self.events.clone())
        }
        fn add_event(&mut self, _input: NewEvent) -> Result<CalendarEvent> {
            Ok(dummy_event(
                "new@2026-03-09",
                "2026-03-09T09:00:00+09:00",
                "2026-03-09T10:00:00+09:00",
            ))
        }
        fn update_event(
            &mut self,
            id: &str,
            _patch: EventPatch,
            _scope: Option<ApplyScope>,
        ) -> Result<CalendarEvent> {
            Ok(dummy_event(
                id,
                "2026-03-09T09:00:00+09:00",
                "2026-03-09T10:00:00+09:00",
            ))
        }
        fn clone_event(&mut self, id: &str, _overrides: CloneOverrides) -> Result<CalendarEvent> {
            Ok(dummy_event(
                &format!("{id}-clone"),
                "2026-03-09T09:00:00+09:00",
                "2026-03-09T10:00:00+09:00",
            ))
        }
        fn delete_event(&mut self, id: &str, _scope: Option<ApplyScope>) -> Result<CalendarEvent> {
            Ok(dummy_event(
                id,
                "2026-03-09T09:00:00+09:00",
                "2026-03-09T10:00:00+09:00",
            ))
        }
        fn drain_notices(&mut self) -> Vec<String> {
            std::mem::take(&mut self.notices)
        }
    }

    #[test]
    fn add_event_invalidates_cache() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let cache_path = temp.path().join("cache.json");
        let list_calls = Arc::new(Mutex::new(0));
        let inner = MutBackend {
            list_calls: list_calls.clone(),
            events: Vec::new(),
            notices: Vec::new(),
        };
        let mut backend = CachingBackend::new(Box::new(inner), cache_path, false);

        let q = ListQuery {
            from: Some(ts("2026-03-09T00:00:00+09:00")),
            to: Some(ts("2026-03-16T00:00:00+09:00")),
        };

        backend.list_events(q.clone())?; // miss → 1
        backend.list_events(q.clone())?; // hit → still 1
        assert_eq!(*list_calls.lock().unwrap(), 1);

        // add event should invalidate cache
        let new_event = NewEvent {
            title: "new".to_string(),
            description: None,
            starts_at: ts("2026-03-10T09:00:00+09:00"),
            ends_at: ts("2026-03-10T10:00:00+09:00"),
            attendees: Vec::new(),
            facility: None,
            calendar: None,
            visibility: crate::model::EventVisibility::Public,
        };
        backend.add_event(new_event)?;
        backend.list_events(q)?; // cache invalidated → 2
        assert_eq!(*list_calls.lock().unwrap(), 2);
        Ok(())
    }

    #[test]
    fn delete_event_invalidates_cache() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let cache_path = temp.path().join("cache.json");
        let list_calls = Arc::new(Mutex::new(0));
        let inner = MutBackend {
            list_calls: list_calls.clone(),
            events: Vec::new(),
            notices: Vec::new(),
        };
        let mut backend = CachingBackend::new(Box::new(inner), cache_path, false);
        let q = ListQuery {
            from: Some(ts("2026-03-09T00:00:00+09:00")),
            to: Some(ts("2026-03-16T00:00:00+09:00")),
        };
        backend.list_events(q.clone())?; // miss → 1
        backend.list_events(q.clone())?; // hit → 1
        backend.delete_event("some-id", None)?;
        backend.list_events(q)?; // miss after invalidation → 2
        assert_eq!(*list_calls.lock().unwrap(), 2);
        Ok(())
    }

    #[test]
    fn update_event_invalidates_cache() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let cache_path = temp.path().join("cache.json");
        let list_calls = Arc::new(Mutex::new(0));
        let inner = MutBackend {
            list_calls: list_calls.clone(),
            events: Vec::new(),
            notices: Vec::new(),
        };
        let mut backend = CachingBackend::new(Box::new(inner), cache_path, false);
        let q = ListQuery {
            from: Some(ts("2026-03-09T00:00:00+09:00")),
            to: Some(ts("2026-03-16T00:00:00+09:00")),
        };
        backend.list_events(q.clone())?;
        backend.update_event("some-id", crate::model::EventPatch::default(), None)?;
        backend.list_events(q)?;
        assert_eq!(*list_calls.lock().unwrap(), 2);
        Ok(())
    }

    #[test]
    fn clone_event_invalidates_cache() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let cache_path = temp.path().join("cache.json");
        let list_calls = Arc::new(Mutex::new(0));
        let inner = MutBackend {
            list_calls: list_calls.clone(),
            events: Vec::new(),
            notices: Vec::new(),
        };
        let mut backend = CachingBackend::new(Box::new(inner), cache_path, false);
        let q = ListQuery {
            from: Some(ts("2026-03-09T00:00:00+09:00")),
            to: Some(ts("2026-03-16T00:00:00+09:00")),
        };
        backend.list_events(q.clone())?;
        backend.clone_event(
            "some-id",
            crate::model::CloneOverrides {
                title: None,
                title_suffix: None,
                starts_at: None,
                ends_at: None,
            },
        )?;
        backend.list_events(q)?;
        assert_eq!(*list_calls.lock().unwrap(), 2);
        Ok(())
    }

    #[test]
    fn drain_notices_propagates_from_inner() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let cache_path = temp.path().join("cache.json");
        let list_calls = Arc::new(Mutex::new(0));
        let inner = MutBackend {
            list_calls: list_calls.clone(),
            events: Vec::new(),
            notices: vec!["初期化完了".to_string()],
        };
        let mut backend = CachingBackend::new(Box::new(inner), cache_path, false);
        let q = ListQuery {
            from: Some(ts("2026-03-09T00:00:00+09:00")),
            to: Some(ts("2026-03-16T00:00:00+09:00")),
        };
        backend.list_events(q)?;
        let notices = backend.drain_notices();
        assert_eq!(notices, vec!["初期化完了"]);
        assert!(backend.drain_notices().is_empty());
        Ok(())
    }
}
