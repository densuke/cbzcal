use std::{fs, path::PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    backend::{ApplyScope, CalendarBackend, ListQuery},
    model::{CalendarEvent, CloneOverrides, EventPatch, NewEvent},
};

#[derive(Debug, Default, Serialize, Deserialize)]
struct FixtureStore {
    events: Vec<CalendarEvent>,
}

pub struct FixtureBackend {
    path: PathBuf,
    store: FixtureStore,
}

impl FixtureBackend {
    pub fn open(path: PathBuf) -> Result<Self> {
        let store = if path.exists() {
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("fixture を読み込めません: {}", path.display()))?;
            serde_json::from_str(&raw)
                .with_context(|| format!("fixture を解釈できません: {}", path.display()))?
        } else {
            FixtureStore::default()
        };

        Ok(Self { path, store })
    }

    fn persist(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("fixture ディレクトリを作成できません: {}", parent.display())
            })?;
        }

        let raw = serde_json::to_string_pretty(&self.store)?;
        fs::write(&self.path, raw)
            .with_context(|| format!("fixture を書き込めません: {}", self.path.display()))?;
        Ok(())
    }

    fn event_index(&self, id: &str) -> Result<usize> {
        self.store
            .events
            .iter()
            .position(|event| event.id == id)
            .ok_or_else(|| anyhow::anyhow!("イベントが見つかりません: {id}"))
    }
}

impl CalendarBackend for FixtureBackend {
    fn name(&self) -> &'static str {
        "fixture"
    }

    fn list_events(&mut self, query: ListQuery) -> Result<Vec<CalendarEvent>> {
        let mut events = self
            .store
            .events
            .iter()
            .filter(|event| {
                let starts_before_upper = query.to.is_none_or(|upper| event.starts_at < upper);
                let ends_after_lower = query.from.is_none_or(|lower| event.ends_at > lower);
                starts_before_upper && ends_after_lower
            })
            .cloned()
            .collect::<Vec<_>>();

        events.sort_by(|left, right| {
            left.starts_at
                .cmp(&right.starts_at)
                .then_with(|| left.title.cmp(&right.title))
        });

        Ok(events)
    }

    fn add_event(&mut self, input: NewEvent) -> Result<CalendarEvent> {
        input.validate()?;

        let event = CalendarEvent {
            id: Uuid::new_v4().to_string(),
            title: input.title,
            description: input.description,
            starts_at: input.starts_at,
            ends_at: input.ends_at,
            attendees: input.attendees,
            facility: input.facility,
            calendar: input.calendar,
            version: 1,
        };

        self.store.events.push(event.clone());
        self.persist()?;
        Ok(event)
    }

    fn update_event(
        &mut self,
        id: &str,
        patch: EventPatch,
        _scope: Option<ApplyScope>,
    ) -> Result<CalendarEvent> {
        if patch.is_empty() {
            bail!("更新対象がありません");
        }

        let index = self.event_index(id)?;
        let updated = self.store.events[index].apply_patch(&patch)?;
        self.store.events[index] = updated.clone();
        self.persist()?;
        Ok(updated)
    }

    fn clone_event(&mut self, id: &str, overrides: CloneOverrides) -> Result<CalendarEvent> {
        let index = self.event_index(id)?;
        let cloned = self.store.events[index]
            .clone_with_overrides(&overrides, Uuid::new_v4().to_string())?;
        self.store.events.push(cloned.clone());
        self.persist()?;
        Ok(cloned)
    }

    fn delete_event(&mut self, id: &str, _scope: Option<ApplyScope>) -> Result<CalendarEvent> {
        let index = self.event_index(id)?;
        let deleted = self.store.events.remove(index);
        self.persist()?;
        Ok(deleted)
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, FixedOffset};

    use super::*;

    fn ts(input: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(input).expect("timestamp")
    }

    #[test]
    fn fixture_backend_supports_crud_and_clone() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let fixture_path = tempdir.path().join("calendar.json");
        let mut backend = FixtureBackend::open(fixture_path).expect("open backend");

        let created = backend
            .add_event(NewEvent {
                title: "設計レビュー".to_string(),
                description: Some("CLI 基盤の確認".to_string()),
                starts_at: ts("2026-03-09T10:00:00+09:00"),
                ends_at: ts("2026-03-09T11:00:00+09:00"),
                attendees: vec!["alice".to_string(), "bob".to_string()],
                facility: Some("会議室A".to_string()),
                calendar: Some("開発".to_string()),
            })
            .expect("add");

        let listed = backend
            .list_events(ListQuery {
                from: Some(ts("2026-03-09T09:00:00+09:00")),
                to: Some(ts("2026-03-09T12:00:00+09:00")),
            })
            .expect("list");
        assert_eq!(listed.len(), 1);

        let updated = backend
            .update_event(
                &created.id,
                EventPatch {
                    title: Some("詳細設計レビュー".to_string()),
                    description: Some(None),
                    ..EventPatch::default()
                },
                None,
            )
            .expect("update");
        assert_eq!(updated.title, "詳細設計レビュー");
        assert_eq!(updated.description, None);
        assert_eq!(updated.version, 2);

        let cloned = backend
            .clone_event(
                &created.id,
                CloneOverrides {
                    title_suffix: Some(" (複製)".to_string()),
                    starts_at: Some(ts("2026-03-10T14:00:00+09:00")),
                    ends_at: None,
                    title: None,
                },
            )
            .expect("clone");
        assert_eq!(cloned.title, "詳細設計レビュー (複製)");
        assert_eq!(cloned.duration(), created.duration());
        assert_eq!(cloned.starts_at, ts("2026-03-10T14:00:00+09:00"));
        assert_eq!(cloned.ends_at, ts("2026-03-10T15:00:00+09:00"));

        let deleted = backend.delete_event(&created.id, None).expect("delete");
        assert_eq!(deleted.id, created.id);
        let remaining = backend
            .list_events(ListQuery {
                from: None,
                to: None,
            })
            .expect("list all");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, cloned.id);
    }
}
