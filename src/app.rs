use std::io::{self, Write};

use anyhow::{Result, bail};
use chrono::{Datelike, FixedOffset, TimeZone};

use clap::CommandFactory;
use crate::{
    backend::{
        CalendarBackend, CybozuHtmlBackend, ListQuery, build_backend,
        id::extract_date_from_event_identifier,
    },
    cli::{Cli, Command, ResolvedEventsArgs},
    config::AppConfig,
    doctor,
    executor::execute_events_command,
    model::CalendarEvent,
    prompt::{plan_prompt, render_preview},
    view::render_json,
};

pub fn execute(cli: Cli) -> Result<String> {
    let loaded = AppConfig::load_with_resolution(cli.config.as_deref())?;
    let verbose = cli.verbose;

    match cli.command {
        Command::Doctor => render_json(&doctor::generate_report(&loaded.config, &loaded.path)),
        Command::ProbeLogin => {
            let cybozu = loaded
                .config
                .cybozu_html
                .clone()
                .ok_or_else(|| anyhow::anyhow!("[cybozu-html] セクションがありません"))?;
            render_json(&CybozuHtmlBackend::probe_login(cybozu)?)
        }
        Command::Events(events) => {
            let mut backend = build_backend(&loaded.config)?;
            let output = match events.resolve()? {
                ResolvedEventsArgs::Prompt(prompt) => {
                    let existing_event = extract_short_id_hint(&prompt.prompt)
                        .map(|id| find_event_by_id(backend.as_mut(), &id))
                        .transpose()?;
                    let execution = plan_prompt(
                        &loaded.config,
                        &prompt.prompt,
                        None,
                        existing_event.as_ref(),
                    )?;
                    if prompt.yes && !execution.supports_yes() {
                        bail!(
                            "`--yes` は prompt モードの list/add/clone でのみ使えます。update/delete では確認が必須です"
                        );
                    }
                    let preview = render_preview(&execution);
                    println!("{preview}");
                    if !prompt.yes && !confirm_execution()? {
                        emit_verbose_notices(verbose, backend.drain_notices());
                        return Ok("キャンセルしました".to_string());
                    }

                    execute_events_command(backend.as_mut(), execution.command)?
                }
                ResolvedEventsArgs::Command(command) => {
                    execute_events_command(backend.as_mut(), command)?
                }
            };
            emit_verbose_notices(verbose, backend.drain_notices());
            Ok(output)
        }
        Command::Shell { shell } => {
            let mut cmd = Cli::command();
            clap_complete::generate(shell, &mut cmd, "cbzcal", &mut io::stdout());
            Ok("".to_string())
        }
    }
}

fn confirm_execution() -> Result<bool> {
    print!("実行しますか? [y/N] ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(matches!(
        input.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn emit_verbose_notices(verbose: u8, notices: Vec<String>) {
    if verbose == 0 {
        return;
    }
    for notice in notices {
        eprintln!("[verbose] {notice}");
    }
}

fn extract_short_id_hint(prompt: &str) -> Option<String> {
    prompt
        .split_whitespace()
        .find(|token| token.contains('@'))
        .map(|token| {
            token
                .trim_matches(|ch: char| matches!(ch, '"' | '\'' | '「' | '」' | '。' | '、'))
                .to_string()
        })
}

fn find_event_by_id(backend: &mut dyn CalendarBackend, id: &str) -> Result<CalendarEvent> {
    let date = extract_date_from_event_identifier(id)
        .ok_or_else(|| anyhow::anyhow!("ID から日付を解決できませんでした: {id}"))?;
    let jst = FixedOffset::east_opt(9 * 60 * 60).expect("jst");
    let from = jst
        .with_ymd_and_hms(date.year(), date.month(), date.day(), 0, 0, 0)
        .single()
        .expect("start of day");
    let to = from + chrono::TimeDelta::days(1);
    let events = backend.list_events(ListQuery {
        from: Some(from),
        to: Some(to),
    })?;
    events
        .into_iter()
        .find(|event| event.id == id || event.short_id() == id)
        .ok_or_else(|| anyhow::anyhow!("対象予定が見つかりませんでした: {id}"))
}
