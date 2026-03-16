use anyhow::{Result, bail};

use crate::{
    backend::{ApplyScope, CalendarBackend, ListQuery},
    browser::open_in_browser,
    cli::{ApplyScopeArg, EventsCommand},
    datetime::current_jst_now,
    view::{EventEnvelope, render_event_list, render_event_result, render_events, render_json},
};

pub fn execute_events_command(
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
                render_event_list(&events, Some(current_jst_now()))
            }
        }
        EventsCommand::Add(args) => {
            let event = backend.add_event(args.new_event()?)?;
            render_event_result(
                "追加しました",
                backend.name(),
                &event,
                args.json,
                Some(current_jst_now()),
            )
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
                render_event_result(
                    "更新しました",
                    backend.name(),
                    &event,
                    args.json,
                    Some(current_jst_now()),
                )
            }
        }
        EventsCommand::Clone(args) => {
            let overrides = args.overrides()?;
            let event = backend.clone_event(&args.id, overrides)?;
            render_event_result(
                "複製しました",
                backend.name(),
                &event,
                args.json,
                Some(current_jst_now()),
            )
        }
        EventsCommand::Delete(args) => {
            if args.id.is_empty() {
                bail!("削除対象の ID が空です");
            }
            let event = backend.delete_event(&args.id, args.scope.map(into_apply_scope))?;
            render_event_result(
                "削除しました",
                backend.name(),
                &event,
                args.json,
                Some(current_jst_now()),
            )
        }
    }
}

pub fn into_apply_scope(scope: ApplyScopeArg) -> ApplyScope {
    match scope {
        ApplyScopeArg::This => ApplyScope::This,
        ApplyScopeArg::After => ApplyScope::After,
        ApplyScopeArg::All => ApplyScope::All,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        backend::{ApplyScope, CalendarBackend, ListQuery},
        cli::{ApplyScopeArg, DeleteArgs, EventsCommand, ListArgs, UpdateArgs},
        model::{CalendarEvent, CloneOverrides, EventPatch, EventVisibility, NewEvent},
    };
    use anyhow::Result;
    use chrono::{DateTime, FixedOffset};

    struct StubBackend {
        events: Vec<CalendarEvent>,
    }

    fn ts(input: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(input).expect("timestamp")
    }

    fn make_event(id: &str) -> CalendarEvent {
        CalendarEvent {
            id: id.to_string(),
            title: "テスト予定".to_string(),
            description: None,
            starts_at: ts("2026-03-09T09:00:00+09:00"),
            ends_at: ts("2026-03-09T10:00:00+09:00"),
            attendees: Vec::new(),
            facility: None,
            calendar: None,
            visibility: EventVisibility::Public,
            version: 1,
        }
    }

    impl CalendarBackend for StubBackend {
        fn name(&self) -> &'static str {
            "stub"
        }
        fn list_events(&mut self, _query: ListQuery) -> Result<Vec<CalendarEvent>> {
            Ok(self.events.clone())
        }
        fn add_event(&mut self, input: NewEvent) -> Result<CalendarEvent> {
            Ok(make_event(&format!("new@{}", input.starts_at.date_naive())))
        }
        fn update_event(
            &mut self,
            id: &str,
            _patch: EventPatch,
            _scope: Option<ApplyScope>,
        ) -> Result<CalendarEvent> {
            Ok(make_event(id))
        }
        fn clone_event(&mut self, id: &str, _overrides: CloneOverrides) -> Result<CalendarEvent> {
            Ok(make_event(&format!("{id}-clone")))
        }
        fn delete_event(&mut self, id: &str, _scope: Option<ApplyScope>) -> Result<CalendarEvent> {
            Ok(make_event(id))
        }
    }

    fn make_add_args(date: &str, at: &str, duration: &str) -> EventsCommand {
        EventsCommand::Add(crate::cli::AddArgs {
            json: false,
            title: "テスト予定".to_string(),
            public: true,
            private: false,
            start: None,
            end: None,
            date: Some(date.to_string()),
            at: Some(at.to_string()),
            until: None,
            duration: Some(duration.to_string()),
            all_day: false,
            description: None,
            attendees: Vec::new(),
            facility: None,
            calendar: None,
        })
    }

    #[test]
    fn into_apply_scope_this() {
        assert_eq!(into_apply_scope(ApplyScopeArg::This), ApplyScope::This);
    }

    #[test]
    fn into_apply_scope_after() {
        assert_eq!(into_apply_scope(ApplyScopeArg::After), ApplyScope::After);
    }

    #[test]
    fn into_apply_scope_all() {
        assert_eq!(into_apply_scope(ApplyScopeArg::All), ApplyScope::All);
    }

    #[test]
    fn execute_delete_fails_on_empty_id() {
        let mut backend = StubBackend { events: Vec::new() };
        let command = EventsCommand::Delete(DeleteArgs {
            id: "".to_string(),
            scope: None,
            json: false,
        });
        let result = execute_events_command(&mut backend, command);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("空"));
    }

    #[test]
    fn execute_delete_returns_success() {
        let mut backend = StubBackend { events: Vec::new() };
        let command = EventsCommand::Delete(DeleteArgs {
            id: "3096804@2026-03-09".to_string(),
            scope: None,
            json: false,
        });
        let result = execute_events_command(&mut backend, command);
        assert!(result.is_ok());
        assert!(result.unwrap().contains("削除しました"));
    }

    #[test]
    fn execute_update_fails_with_empty_patch_and_no_web() {
        let mut backend = StubBackend { events: Vec::new() };
        let command = EventsCommand::Update(UpdateArgs {
            id: "3096804@2026-03-09".to_string(),
            title: None,
            description: None,
            start: None,
            end: None,
            scope: None,
            web: false,
            json: false,
            clear_description: false,
            attendees: Vec::new(),
            clear_attendees: false,
            facility: None,
            clear_facility: false,
            calendar: None,
            clear_calendar: false,
        });
        let result = execute_events_command(&mut backend, command);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("更新対象がありません")
        );
    }

    #[test]
    fn execute_list_returns_text_by_default() {
        let mut backend = StubBackend {
            events: vec![make_event(
                "sEID=100&UID=1&GID=1&Date=da.2026.3.9&BDate=da.2026.3.9",
            )],
        };
        let command = EventsCommand::List(ListArgs {
            from: None,
            to: None,
            date: None,
            duration: None,
            json: false,
        });
        let result = execute_events_command(&mut backend, command);
        assert!(result.is_ok());
    }

    #[test]
    fn execute_list_json_format() {
        let mut backend = StubBackend {
            events: vec![make_event(
                "sEID=100&UID=1&GID=1&Date=da.2026.3.9&BDate=da.2026.3.9",
            )],
        };
        let command = EventsCommand::List(ListArgs {
            from: None,
            to: None,
            date: None,
            duration: None,
            json: true,
        });
        let result = execute_events_command(&mut backend, command);
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.contains("stub"));
    }

    #[test]
    fn execute_add_returns_success() {
        let mut backend = StubBackend { events: Vec::new() };
        let result =
            execute_events_command(&mut backend, make_add_args("2026-03-15", "10:00", "1h"));
        assert!(result.is_ok(), "add failed: {:?}", result);
        assert!(result.unwrap().contains("追加しました"));
    }

    #[test]
    fn execute_add_json_format() {
        let mut backend = StubBackend { events: Vec::new() };
        let command = EventsCommand::Add(crate::cli::AddArgs {
            json: true,
            title: "JSON予定".to_string(),
            public: true,
            private: false,
            start: None,
            end: None,
            date: Some("2026-03-15".to_string()),
            at: Some("10:00".to_string()),
            until: None,
            duration: Some("1h".to_string()),
            all_day: false,
            description: None,
            attendees: Vec::new(),
            facility: None,
            calendar: None,
        });
        let result = execute_events_command(&mut backend, command);
        assert!(result.is_ok());
        assert!(result.unwrap().contains("stub"));
    }

    #[test]
    fn execute_clone_returns_success() {
        let mut backend = StubBackend { events: Vec::new() };
        let command = EventsCommand::Clone(crate::cli::CloneArgs {
            json: false,
            id: "3096804@2026-03-09".to_string(),
            title: Some("クローン".to_string()),
            title_suffix: None,
            start: None,
            end: None,
        });
        let result = execute_events_command(&mut backend, command);
        assert!(result.is_ok());
        assert!(result.unwrap().contains("複製しました"));
    }

    #[test]
    fn execute_update_with_title_patch_succeeds() {
        let mut backend = StubBackend { events: Vec::new() };
        let command = EventsCommand::Update(UpdateArgs {
            id: "3096804@2026-03-09".to_string(),
            title: Some("新タイトル".to_string()),
            description: None,
            start: None,
            end: None,
            scope: None,
            web: false,
            json: false,
            clear_description: false,
            attendees: Vec::new(),
            clear_attendees: false,
            facility: None,
            clear_facility: false,
            calendar: None,
            clear_calendar: false,
        });
        let result = execute_events_command(&mut backend, command);
        assert!(result.is_ok());
        assert!(result.unwrap().contains("更新しました"));
    }

    #[test]
    fn execute_delete_with_scope_succeeds() {
        use crate::cli::ApplyScopeArg;
        let mut backend = StubBackend { events: Vec::new() };
        let command = EventsCommand::Delete(DeleteArgs {
            id: "3096804@2026-03-09".to_string(),
            scope: Some(ApplyScopeArg::All),
            json: false,
        });
        let result = execute_events_command(&mut backend, command);
        assert!(result.is_ok());
        assert!(result.unwrap().contains("削除しました"));
    }
}
