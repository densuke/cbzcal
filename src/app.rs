use anyhow::{Result, bail};
use serde::Serialize;

use crate::{
    backend::{CybozuHtmlBackend, ListQuery, build_backend},
    cli::{Cli, Command, EventsCommand},
    config::AppConfig,
    model::CalendarEvent,
};

#[derive(Debug, Serialize)]
struct DeleteResult<'a> {
    deleted: bool,
    id: &'a str,
}

#[derive(Debug, Serialize)]
struct EventEnvelope<T: Serialize> {
    backend: &'static str,
    data: T,
}

#[derive(Debug, Serialize)]
struct ApiEvent<'a> {
    short_id: String,
    #[serde(flatten)]
    event: &'a CalendarEvent,
}

pub fn execute(cli: Cli) -> Result<String> {
    let loaded = AppConfig::load_with_resolution(cli.config.as_deref())?;

    match cli.command {
        Command::Doctor => render_json(&loaded.config.doctor_report(&loaded.path)),
        Command::ProbeLogin => {
            let cybozu = loaded
                .config
                .cybozu_html
                .clone()
                .ok_or_else(|| anyhow::anyhow!("[cybozu-html] セクションがありません"))?;
            render_json(&CybozuHtmlBackend::probe_login(cybozu)?)
        }
        Command::Events { command } => {
            let mut backend = build_backend(&loaded.config)?;
            match command {
                EventsCommand::List(args) => {
                    let query: ListQuery = args.query()?;
                    let events = backend.list_events(query.with_default_window())?;
                    render_json(&EventEnvelope {
                        backend: backend.name(),
                        data: render_events(&events),
                    })
                }
                EventsCommand::Add(args) => {
                    let event = backend.add_event(args.new_event()?)?;
                    render_json(&EventEnvelope {
                        backend: backend.name(),
                        data: render_event(&event),
                    })
                }
                EventsCommand::Update(args) => {
                    let patch = args.patch()?;
                    let event = backend.update_event(&args.id, patch)?;
                    render_json(&EventEnvelope {
                        backend: backend.name(),
                        data: render_event(&event),
                    })
                }
                EventsCommand::Clone(args) => {
                    let overrides = args.overrides()?;
                    let event = backend.clone_event(&args.id, overrides)?;
                    render_json(&EventEnvelope {
                        backend: backend.name(),
                        data: render_event(&event),
                    })
                }
                EventsCommand::Delete(args) => {
                    if args.id.is_empty() {
                        bail!("削除対象の ID が空です");
                    }
                    backend.delete_event(&args.id)?;
                    render_json(&EventEnvelope {
                        backend: backend.name(),
                        data: DeleteResult {
                            deleted: true,
                            id: &args.id,
                        },
                    })
                }
            }
        }
    }
}

fn render_json<T: Serialize>(value: &T) -> Result<String> {
    Ok(serde_json::to_string_pretty(value)?)
}

fn render_event(event: &CalendarEvent) -> ApiEvent<'_> {
    ApiEvent {
        short_id: event.short_id(),
        event,
    }
}

fn render_events(events: &[CalendarEvent]) -> Vec<ApiEvent<'_>> {
    events.iter().map(render_event).collect()
}
