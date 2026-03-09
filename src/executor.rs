use anyhow::{Result, bail};

use crate::{
    backend::{ApplyScope, CalendarBackend, ListQuery},
    browser::open_in_browser,
    cli::{ApplyScopeArg, EventsCommand},
    datetime::current_jst_now,
    prompt::apply_scope_from_arg,
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
    apply_scope_from_arg(Some(scope)).expect("scope")
}
