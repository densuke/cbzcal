use anyhow::{Result, bail};
use chrono::{DateTime, Duration, FixedOffset};
use serde::{Deserialize, Serialize};

use crate::backend::id::short_id_from_event_id;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EventVisibility {
    #[default]
    Public,
    Private,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarEvent {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub starts_at: DateTime<FixedOffset>,
    pub ends_at: DateTime<FixedOffset>,
    pub attendees: Vec<String>,
    pub facility: Option<String>,
    pub calendar: Option<String>,
    #[serde(default)]
    pub visibility: EventVisibility,
    pub version: u64,
}

impl CalendarEvent {
    pub fn short_id(&self) -> String {
        short_id_from_event_id(&self.id)
    }

    pub fn apply_patch(&self, patch: &EventPatch) -> Result<Self> {
        let mut updated = self.clone();

        if let Some(title) = &patch.title {
            updated.title = title.clone();
        }
        if let Some(description) = &patch.description {
            updated.description = description.clone();
        }
        if let Some(starts_at) = patch.starts_at {
            updated.starts_at = starts_at;
        }
        if let Some(ends_at) = patch.ends_at {
            updated.ends_at = ends_at;
        }
        if let Some(attendees) = &patch.attendees {
            updated.attendees = attendees.clone();
        }
        if let Some(facility) = &patch.facility {
            updated.facility = facility.clone();
        }
        if let Some(calendar) = &patch.calendar {
            updated.calendar = calendar.clone();
        }

        validate_time_range(updated.starts_at, updated.ends_at)?;
        updated.version += 1;
        Ok(updated)
    }

    pub fn clone_with_overrides(&self, overrides: &CloneOverrides, new_id: String) -> Result<Self> {
        let duration = self.ends_at - self.starts_at;

        let starts_at = match (overrides.starts_at, overrides.ends_at) {
            (Some(starts_at), Some(ends_at)) => {
                validate_time_range(starts_at, ends_at)?;
                starts_at
            }
            (Some(starts_at), None) => starts_at,
            (None, Some(ends_at)) => ends_at - duration,
            (None, None) => self.starts_at,
        };

        let ends_at = match (overrides.starts_at, overrides.ends_at) {
            (Some(_), Some(ends_at)) => ends_at,
            (Some(starts_at), None) => starts_at + duration,
            (None, Some(ends_at)) => ends_at,
            (None, None) => self.ends_at,
        };

        validate_time_range(starts_at, ends_at)?;

        let title = if let Some(title) = &overrides.title {
            title.clone()
        } else if let Some(suffix) = &overrides.title_suffix {
            format!("{}{}", self.title, suffix)
        } else {
            self.title.clone()
        };

        Ok(Self {
            id: new_id,
            title,
            description: self.description.clone(),
            starts_at,
            ends_at,
            attendees: self.attendees.clone(),
            facility: self.facility.clone(),
            calendar: self.calendar.clone(),
            visibility: self.visibility,
            version: 1,
        })
    }

    pub fn duration(&self) -> Duration {
        self.ends_at - self.starts_at
    }

    pub fn overlaps(
        &self,
        from: Option<DateTime<FixedOffset>>,
        to: Option<DateTime<FixedOffset>>,
    ) -> bool {
        let start_ok = match to {
            None => true,
            Some(to) => self.starts_at < to,
        };
        let end_ok = match from {
            None => true,
            Some(from) => self.ends_at > from,
        };
        start_ok && end_ok
    }

    pub fn is_ongoing(&self, now: DateTime<FixedOffset>) -> bool {
        self.starts_at <= now && now < self.ends_at
    }

    pub fn is_passed(&self, now: DateTime<FixedOffset>) -> bool {
        self.ends_at <= now
    }
}

#[derive(Debug, Clone)]
pub struct NewEvent {
    pub title: String,
    pub description: Option<String>,
    pub starts_at: DateTime<FixedOffset>,
    pub ends_at: DateTime<FixedOffset>,
    pub attendees: Vec<String>,
    pub facility: Option<String>,
    pub calendar: Option<String>,
    pub visibility: EventVisibility,
}

impl NewEvent {
    pub fn validate(&self) -> Result<()> {
        validate_time_range(self.starts_at, self.ends_at)
    }
}

#[derive(Debug, Clone, Default)]
pub struct EventPatch {
    pub title: Option<String>,
    pub description: Option<Option<String>>,
    pub starts_at: Option<DateTime<FixedOffset>>,
    pub ends_at: Option<DateTime<FixedOffset>>,
    pub attendees: Option<Vec<String>>,
    pub facility: Option<Option<String>>,
    pub calendar: Option<Option<String>>,
}

impl EventPatch {
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.description.is_none()
            && self.starts_at.is_none()
            && self.ends_at.is_none()
            && self.attendees.is_none()
            && self.facility.is_none()
            && self.calendar.is_none()
    }
}

#[derive(Debug, Clone, Default)]
pub struct CloneOverrides {
    pub title: Option<String>,
    pub title_suffix: Option<String>,
    pub starts_at: Option<DateTime<FixedOffset>>,
    pub ends_at: Option<DateTime<FixedOffset>>,
}

pub fn validate_time_range(
    starts_at: DateTime<FixedOffset>,
    ends_at: DateTime<FixedOffset>,
) -> Result<()> {
    if ends_at <= starts_at {
        bail!("終了日時は開始日時より後である必要があります");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(input: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(input).expect("timestamp")
    }

    #[test]
    fn overlaps_test() {
        let event = CalendarEvent {
            id: "1".to_string(),
            title: "T1".to_string(),
            description: None,
            starts_at: ts("2026-03-09T09:00:00+09:00"),
            ends_at: ts("2026-03-09T10:00:00+09:00"),
            attendees: Vec::new(),
            facility: None,
            calendar: None,
            visibility: EventVisibility::Public,
            version: 1,
        };

        // Exactly same
        assert!(event.overlaps(
            Some(ts("2026-03-09T09:00:00+09:00")),
            Some(ts("2026-03-09T10:00:00+09:00"))
        ));

        // Contained
        assert!(event.overlaps(
            Some(ts("2026-03-09T09:15:00+09:00")),
            Some(ts("2026-03-09T09:45:00+09:00"))
        ));

        // Overlapping at start
        assert!(event.overlaps(
            Some(ts("2026-03-09T08:00:00+09:00")),
            Some(ts("2026-03-09T09:30:00+09:00"))
        ));

        // Overlapping at end
        assert!(event.overlaps(
            Some(ts("2026-03-09T09:30:00+09:00")),
            Some(ts("2026-03-09T11:00:00+09:00"))
        ));

        // Just touching at start (No overlap)
        assert!(!event.overlaps(
            Some(ts("2026-03-09T08:00:00+09:00")),
            Some(ts("2026-03-09T09:00:00+09:00"))
        ));

        // Just touching at end (No overlap)
        assert!(!event.overlaps(
            Some(ts("2026-03-09T10:00:00+09:00")),
            Some(ts("2026-03-09T11:00:00+09:00"))
        ));

        // Completely outside
        assert!(!event.overlaps(
            Some(ts("2026-03-10T09:00:00+09:00")),
            Some(ts("2026-03-10T10:00:00+09:00"))
        ));

        // Unbounded
        assert!(event.overlaps(None, None));
        assert!(event.overlaps(Some(ts("2026-03-09T00:00:00+09:00")), None));
        assert!(event.overlaps(None, Some(ts("2026-03-09T12:00:00+09:00"))));
    }

    #[test]
    fn apply_patch_updates_title_and_increments_version() {
        let event = CalendarEvent {
            id: "1".to_string(),
            title: "旧タイトル".to_string(),
            description: None,
            starts_at: ts("2026-03-09T09:00:00+09:00"),
            ends_at: ts("2026-03-09T10:00:00+09:00"),
            attendees: Vec::new(),
            facility: None,
            calendar: None,
            visibility: EventVisibility::Public,
            version: 1,
        };
        let patch = EventPatch {
            title: Some("新タイトル".to_string()),
            ..Default::default()
        };
        let updated = event.apply_patch(&patch).expect("patch");
        assert_eq!(updated.title, "新タイトル");
        assert_eq!(updated.version, 2);
    }

    #[test]
    fn apply_patch_rejects_inverted_time_range() {
        let event = CalendarEvent {
            id: "1".to_string(),
            title: "T".to_string(),
            description: None,
            starts_at: ts("2026-03-09T09:00:00+09:00"),
            ends_at: ts("2026-03-09T10:00:00+09:00"),
            attendees: Vec::new(),
            facility: None,
            calendar: None,
            visibility: EventVisibility::Public,
            version: 1,
        };
        let patch = EventPatch {
            ends_at: Some(ts("2026-03-09T08:00:00+09:00")),
            ..Default::default()
        };
        assert!(event.apply_patch(&patch).is_err());
    }

    #[test]
    fn is_ongoing_and_is_passed() {
        let event = CalendarEvent {
            id: "1".to_string(),
            title: "T".to_string(),
            description: None,
            starts_at: ts("2026-03-09T09:00:00+09:00"),
            ends_at: ts("2026-03-09T10:00:00+09:00"),
            attendees: Vec::new(),
            facility: None,
            calendar: None,
            visibility: EventVisibility::Public,
            version: 1,
        };

        let during = ts("2026-03-09T09:30:00+09:00");
        let before = ts("2026-03-09T08:00:00+09:00");
        let after = ts("2026-03-09T11:00:00+09:00");

        assert!(event.is_ongoing(during));
        assert!(!event.is_ongoing(before));
        assert!(!event.is_ongoing(after));
        assert!(event.is_passed(after));
        assert!(!event.is_passed(during));
    }

    #[test]
    fn clone_with_overrides_applies_title_suffix() {
        let event = CalendarEvent {
            id: "1".to_string(),
            title: "元タイトル".to_string(),
            description: None,
            starts_at: ts("2026-03-09T09:00:00+09:00"),
            ends_at: ts("2026-03-09T10:00:00+09:00"),
            attendees: Vec::new(),
            facility: None,
            calendar: None,
            visibility: EventVisibility::Public,
            version: 1,
        };
        let overrides = CloneOverrides {
            title_suffix: Some(" (複製)".to_string()),
            ..Default::default()
        };
        let cloned = event
            .clone_with_overrides(&overrides, "2".to_string())
            .expect("clone");
        assert_eq!(cloned.title, "元タイトル (複製)");
        assert_eq!(cloned.id, "2");
        assert_eq!(cloned.version, 1);
    }

    #[test]
    fn event_patch_is_empty_check() {
        let empty = EventPatch::default();
        assert!(empty.is_empty());
        let with_title = EventPatch {
            title: Some("T".to_string()),
            ..Default::default()
        };
        assert!(!with_title.is_empty());
    }
}
