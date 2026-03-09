use std::path::PathBuf;

use anyhow::{Result, bail};
use chrono::{DateTime, FixedOffset};
use clap::{Args, Parser, Subcommand};

use crate::backend::ListQuery;
use crate::model::{CloneOverrides, EventPatch, NewEvent};

#[derive(Debug, Parser)]
#[command(
    name = "cbzcal",
    version,
    about = "サイボウズ Office の予定表操作に向けた CLI ベース"
)]
pub struct Cli {
    #[arg(
        long,
        global = true,
        help = "設定ファイルのパス。未指定時は .cbzcal.toml を PWD -> XDG -> HOME の順で探索"
    )]
    pub config: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Doctor,
    ProbeLogin,
    Events {
        #[command(subcommand)]
        command: EventsCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum EventsCommand {
    List(ListArgs),
    Add(AddArgs),
    Update(UpdateArgs),
    Clone(CloneArgs),
    Delete(DeleteArgs),
}

#[derive(Debug, Args)]
pub struct ListArgs {
    #[arg(long, value_parser = parse_timestamp, help = "開始日時。未指定時は JST 当日 00:00 から 1 週間")]
    pub from: Option<DateTime<FixedOffset>>,
    #[arg(long, value_parser = parse_timestamp, help = "終了日時。未指定時は開始から 1 週間、両方未指定なら 1 週間")]
    pub to: Option<DateTime<FixedOffset>>,
}

impl From<ListArgs> for ListQuery {
    fn from(value: ListArgs) -> Self {
        Self {
            from: value.from,
            to: value.to,
        }
    }
}

#[derive(Debug, Args)]
pub struct AddArgs {
    #[arg(long)]
    pub title: String,
    #[arg(long, value_parser = parse_timestamp)]
    pub start: DateTime<FixedOffset>,
    #[arg(long, value_parser = parse_timestamp)]
    pub end: DateTime<FixedOffset>,
    #[arg(long)]
    pub description: Option<String>,
    #[arg(long = "attendee")]
    pub attendees: Vec<String>,
    #[arg(long)]
    pub facility: Option<String>,
    #[arg(long)]
    pub calendar: Option<String>,
}

impl From<AddArgs> for NewEvent {
    fn from(value: AddArgs) -> Self {
        Self {
            title: value.title,
            description: value.description,
            starts_at: value.start,
            ends_at: value.end,
            attendees: value.attendees,
            facility: value.facility,
            calendar: value.calendar,
        }
    }
}

#[derive(Debug, Args)]
pub struct UpdateArgs {
    #[arg(long)]
    pub id: String,
    #[arg(long)]
    pub title: Option<String>,
    #[arg(long, value_parser = parse_timestamp)]
    pub start: Option<DateTime<FixedOffset>>,
    #[arg(long, value_parser = parse_timestamp)]
    pub end: Option<DateTime<FixedOffset>>,
    #[arg(long)]
    pub description: Option<String>,
    #[arg(long)]
    pub clear_description: bool,
    #[arg(long = "attendee")]
    pub attendees: Vec<String>,
    #[arg(long)]
    pub clear_attendees: bool,
    #[arg(long)]
    pub facility: Option<String>,
    #[arg(long)]
    pub clear_facility: bool,
    #[arg(long)]
    pub calendar: Option<String>,
    #[arg(long)]
    pub clear_calendar: bool,
}

impl UpdateArgs {
    pub fn patch(&self) -> Result<EventPatch> {
        if self.description.is_some() && self.clear_description {
            bail!("`--description` と `--clear-description` は同時に使えません");
        }
        if !self.attendees.is_empty() && self.clear_attendees {
            bail!("`--attendee` と `--clear-attendees` は同時に使えません");
        }
        if self.facility.is_some() && self.clear_facility {
            bail!("`--facility` と `--clear-facility` は同時に使えません");
        }
        if self.calendar.is_some() && self.clear_calendar {
            bail!("`--calendar` と `--clear-calendar` は同時に使えません");
        }

        let patch = EventPatch {
            title: self.title.clone(),
            description: if self.clear_description {
                Some(None)
            } else {
                self.description.clone().map(Some)
            },
            starts_at: self.start,
            ends_at: self.end,
            attendees: if self.clear_attendees {
                Some(Vec::new())
            } else if self.attendees.is_empty() {
                None
            } else {
                Some(self.attendees.clone())
            },
            facility: if self.clear_facility {
                Some(None)
            } else {
                self.facility.clone().map(Some)
            },
            calendar: if self.clear_calendar {
                Some(None)
            } else {
                self.calendar.clone().map(Some)
            },
        };

        if patch.is_empty() {
            bail!("更新対象がありません。少なくとも 1 つの変更オプションを指定してください");
        }

        Ok(patch)
    }
}

#[derive(Debug, Args)]
pub struct CloneArgs {
    #[arg(long)]
    pub id: String,
    #[arg(long)]
    pub title: Option<String>,
    #[arg(long)]
    pub title_suffix: Option<String>,
    #[arg(long, value_parser = parse_timestamp)]
    pub start: Option<DateTime<FixedOffset>>,
    #[arg(long, value_parser = parse_timestamp)]
    pub end: Option<DateTime<FixedOffset>>,
}

impl CloneArgs {
    pub fn overrides(&self) -> Result<CloneOverrides> {
        if self.title.is_some() && self.title_suffix.is_some() {
            bail!("`--title` と `--title-suffix` はどちらか一方だけ指定してください");
        }

        Ok(CloneOverrides {
            title: self.title.clone(),
            title_suffix: self.title_suffix.clone(),
            starts_at: self.start,
            ends_at: self.end,
        })
    }
}

#[derive(Debug, Args)]
pub struct DeleteArgs {
    #[arg(long)]
    pub id: String,
}

fn parse_timestamp(input: &str) -> Result<DateTime<FixedOffset>, String> {
    DateTime::parse_from_rfc3339(input)
        .map_err(|error| format!("RFC3339 形式の日時で指定してください: {error}"))
}
