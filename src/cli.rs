use std::path::PathBuf;

use anyhow::{Result, bail};
use chrono::{DateTime, Days, FixedOffset, NaiveDate, TimeDelta};
use clap::ArgAction;
use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::backend::ListQuery;
use crate::datetime::{
    current_jst_date, parse_duration, parse_flexible_date, parse_flexible_datetime,
    parse_time_of_day, parse_timestamp, to_jst_datetime,
};
use crate::model::{CloneOverrides, EventPatch, EventVisibility, NewEvent};

#[derive(Debug, Parser)]
#[command(
    name = "cbzcal",
    version,
    about = "サイボウズ Office の予定表操作に向けた CLI ベース"
)]
pub struct Cli {
    #[arg(
        short = 'v',
        long = "verbose",
        global = true,
        action = ArgAction::Count,
        help = "冗長出力。認証経路やセッション再利用の補助情報を stderr に出す"
    )]
    pub verbose: u8,
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
    #[allow(clippy::large_enum_variant)]
    Events(EventsArgs),
}

#[derive(Debug, Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct EventsArgs {
    #[arg(
        short = 'p',
        long = "prompt",
        help = "自然文から events の実行内容を組み立てる。実行前に必ず確認する"
    )]
    pub prompt: Option<String>,
    #[arg(
        short = 'y',
        long = "yes",
        requires = "prompt",
        help = "確認を省略して実行する。prompt モードの add/list/clone でのみ有効"
    )]
    pub yes: bool,
    #[command(subcommand)]
    pub command: Option<EventsCommand>,
    #[command(flatten)]
    pub list: ListArgs,
}

impl EventsArgs {
    pub fn resolve(self) -> Result<ResolvedEventsArgs> {
        if let Some(prompt) = self.prompt {
            if self.command.is_some() || !self.list.is_empty() {
                bail!("`--prompt` は通常の subcommand や list オプションと同時に使えません");
            }
            return Ok(ResolvedEventsArgs::Prompt(PromptArgs {
                prompt,
                yes: self.yes,
            }));
        }

        Ok(ResolvedEventsArgs::Command(
            self.command.unwrap_or(EventsCommand::List(self.list)),
        ))
    }
}

#[derive(Debug)]
pub enum ResolvedEventsArgs {
    #[allow(clippy::large_enum_variant)]
    Command(EventsCommand),
    Prompt(PromptArgs),
}

#[derive(Debug)]
pub struct PromptArgs {
    pub prompt: String,
    pub yes: bool,
}

#[derive(Debug, Subcommand)]
pub enum EventsCommand {
    List(ListArgs),
    Add(AddArgs),
    Update(UpdateArgs),
    Clone(CloneArgs),
    Delete(DeleteArgs),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ApplyScopeArg {
    This,
    After,
    All,
}

#[derive(Debug, Args, Clone)]
pub struct ListArgs {
    #[arg(long, help = "JSON 形式で出力する")]
    pub json: bool,
    #[arg(
        long,
        help = "開始日時。RFC3339 のほか、today/tomorrow/3/10/2026-03-10 を指定可能"
    )]
    pub from: Option<String>,
    #[arg(
        long,
        help = "終了日時。RFC3339 のほか、today/tomorrow/3/10/2026-03-10 を指定可能"
    )]
    pub to: Option<String>,
    #[arg(
        long,
        help = "日付単位で取得する対象日。today/tomorrow/3/10/2026-03-10 を指定可能"
    )]
    pub date: Option<String>,
    #[arg(long = "for", help = "期間。30m/2h/2h30m/7d 形式")]
    pub duration: Option<String>,
}

impl ListArgs {
    pub fn query(&self) -> Result<ListQuery> {
        self.query_from(current_jst_date())
    }

    fn query_from(&self, anchor: NaiveDate) -> Result<ListQuery> {
        resolve_list_query(self, anchor).map_err(|error: String| anyhow::anyhow!(error))
    }

    fn is_empty(&self) -> bool {
        !self.json
            && self.from.is_none()
            && self.to.is_none()
            && self.date.is_none()
            && self.duration.is_none()
    }
}

#[derive(Debug, Args, Clone)]
pub struct AddArgs {
    #[arg(long, help = "JSON 形式で出力する")]
    pub json: bool,
    #[arg(long)]
    pub title: String,
    #[arg(
        long = "public",
        conflicts_with = "private",
        help = "予定を公開予定として登録する。既定値"
    )]
    pub public: bool,
    #[arg(
        long = "private",
        conflicts_with = "public",
        help = "予定を非公開予定として登録する"
    )]
    pub private: bool,
    #[arg(long, help = "厳密な開始日時。RFC3339 形式")]
    pub start: Option<DateTime<FixedOffset>>,
    #[arg(long, help = "厳密な終了日時。RFC3339 形式")]
    pub end: Option<DateTime<FixedOffset>>,
    #[arg(
        long,
        help = "対象日。today/tomorrow/3/10/2026-03-10 を指定可能。未指定時は --start/--end を使用"
    )]
    pub date: Option<String>,
    #[arg(long, help = "開始時刻。9/09:00/9:30 形式")]
    pub at: Option<String>,
    #[arg(long, help = "終了時刻。11/11:00/11:30 形式")]
    pub until: Option<String>,
    #[arg(long = "for", help = "所要時間。30m/2h/2h30m/7d 形式")]
    pub duration: Option<String>,
    #[arg(long, help = "日付のみを指定した全日予定として扱う")]
    pub all_day: bool,
    #[arg(long)]
    pub description: Option<String>,
    #[arg(long = "attendee")]
    pub attendees: Vec<String>,
    #[arg(long)]
    pub facility: Option<String>,
    #[arg(long)]
    pub calendar: Option<String>,
}

impl AddArgs {
    pub fn new_event(&self) -> Result<NewEvent> {
        self.new_event_from(current_jst_date())
    }

    fn new_event_from(&self, anchor: NaiveDate) -> Result<NewEvent> {
        resolve_add_event(self, anchor).map_err(|error: String| anyhow::anyhow!(error))
    }
}

#[derive(Debug, Args)]
pub struct UpdateArgs {
    #[arg(long, help = "JSON 形式で出力する")]
    pub json: bool,
    #[arg(long, help = "GUI で追加入力するため対象予定の画面をブラウザで開く")]
    pub web: bool,
    #[arg(long)]
    pub id: String,
    #[arg(long, value_enum, help = "繰り返し予定の更新範囲。this/after/all")]
    pub scope: Option<ApplyScopeArg>,
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

        Ok(patch)
    }
}

#[derive(Debug, Args)]
pub struct CloneArgs {
    #[arg(long, help = "JSON 形式で出力する")]
    pub json: bool,
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
    #[arg(long, help = "JSON 形式で出力する")]
    pub json: bool,
    #[arg(long)]
    pub id: String,
    #[arg(long, value_enum, help = "繰り返し予定の削除範囲。this/after/all")]
    pub scope: Option<ApplyScopeArg>,
}

fn resolve_list_query(args: &ListArgs, anchor: NaiveDate) -> Result<ListQuery, String> {
    if args.date.is_some() && (args.from.is_some() || args.to.is_some()) {
        return Err("`--date` と `--from` / `--to` は同時に使えません".to_string());
    }
    if args.date.is_some() && args.to.is_some() {
        return Err("`--date` と `--to` は同時に使えません".to_string());
    }
    if args.to.is_some() && args.duration.is_some() {
        return Err("`--to` と `--for` は同時に使えません".to_string());
    }

    if let Some(date) = &args.date {
        let date = parse_flexible_date(date, anchor).map_err(|e| e.to_string())?;
        let from = to_jst_datetime(date, 0, 0).map_err(|e| e.to_string())?;
        let to = if let Some(duration) = &args.duration {
            from + parse_duration(duration).map_err(|e| e.to_string())?
        } else {
            from + TimeDelta::days(1)
        };
        return Ok(ListQuery {
            from: Some(from),
            to: Some(to),
        });
    }

    let from = args
        .from
        .as_deref()
        .map(|value| parse_flexible_datetime(value, anchor).map_err(|e| e.to_string()))
        .transpose()?;
    let to = if let Some(duration) = &args.duration {
        let from =
            from.ok_or_else(|| "`--for` を使う場合は `--from` か `--date` が必要です".to_string())?;
        Some(from + parse_duration(duration).map_err(|e| e.to_string())?)
    } else {
        args.to
            .as_deref()
            .map(|value| parse_flexible_datetime(value, anchor).map_err(|e| e.to_string()))
            .transpose()?
    };

    Ok(ListQuery { from, to })
}

fn resolve_add_event(args: &AddArgs, anchor: NaiveDate) -> Result<NewEvent, String> {
    let uses_strict = args.start.is_some() || args.end.is_some();
    let uses_friendly = args.date.is_some()
        || args.at.is_some()
        || args.until.is_some()
        || args.duration.is_some()
        || args.all_day;

    if uses_strict && uses_friendly {
        return Err(
            "`--start` / `--end` と `--date` / `--at` / `--until` / `--for` / `--all-day` は同時に使えません"
                .to_string(),
        );
    }

    let (starts_at, ends_at) = if uses_strict {
        let starts_at = args
            .start
            .ok_or_else(|| "`--start` を指定した場合は `--end` も必要です".to_string())?;
        let ends_at = args
            .end
            .ok_or_else(|| "`--end` を指定した場合は `--start` も必要です".to_string())?;
        (starts_at, ends_at)
    } else {
        let date = args.date.as_deref().ok_or_else(|| {
            "`--date` を指定するか、`--start` / `--end` を使ってください".to_string()
        })?;
        let date = parse_flexible_date(date, anchor).map_err(|e| e.to_string())?;

        if args.all_day || (args.at.is_none() && args.until.is_none() && args.duration.is_none()) {
            if args.at.is_some() || args.until.is_some() || args.duration.is_some() {
                return Err(
                    "`--all-day` を使う場合は `--at` / `--until` / `--for` を指定できません"
                        .to_string(),
                );
            }
            (
                to_jst_datetime(date, 0, 0).map_err(|e| e.to_string())?,
                to_jst_datetime(
                    date.checked_add_days(Days::new(1))
                        .ok_or_else(|| "翌日を計算できません".to_string())?,
                    0,
                    0,
                )
                .map_err(|e| e.to_string())?,
            )
        } else {
            let at = args
                .at
                .as_deref()
                .ok_or_else(|| "`--at` を指定してください".to_string())?;
            if args.until.is_some() && args.duration.is_some() {
                return Err("`--until` と `--for` はどちらか一方だけ指定してください".to_string());
            }
            let (start_hour, start_minute) = parse_time_of_day(at).map_err(|e| e.to_string())?;
            let starts_at =
                to_jst_datetime(date, start_hour, start_minute).map_err(|e| e.to_string())?;
            let ends_at = if let Some(until) = &args.until {
                let (end_hour, end_minute) = parse_time_of_day(until).map_err(|e| e.to_string())?;
                to_jst_datetime(date, end_hour, end_minute).map_err(|e| e.to_string())?
            } else if let Some(duration) = &args.duration {
                starts_at + parse_duration(duration).map_err(|e| e.to_string())?
            } else {
                return Err(
                    "`--at` を使う場合は `--until` か `--for` を指定してください".to_string(),
                );
            };
            (starts_at, ends_at)
        }
    };

    let event = NewEvent {
        title: args.title.clone(),
        description: args.description.clone(),
        starts_at,
        ends_at,
        attendees: args.attendees.clone(),
        facility: args.facility.clone(),
        calendar: args.calendar.clone(),
        visibility: if args.private {
            EventVisibility::Private
        } else {
            EventVisibility::Public
        },
    };
    event.validate().map_err(|error| error.to_string())?;

    Ok(event)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 3, 9).expect("date")
    }

    fn ts(input: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(input).expect("timestamp")
    }

    #[test]
    fn add_supports_date_and_until() {
        let args = AddArgs {
            json: false,
            title: "打合せ".to_string(),
            public: false,
            private: false,
            start: None,
            end: None,
            date: Some("3/10".to_string()),
            at: Some("9".to_string()),
            until: Some("11:00".to_string()),
            duration: None,
            all_day: false,
            description: None,
            attendees: Vec::new(),
            facility: None,
            calendar: None,
        };

        let event = args.new_event_from(anchor()).expect("event");
        assert_eq!(event.starts_at, ts("2026-03-10T09:00:00+09:00"));
        assert_eq!(event.ends_at, ts("2026-03-10T11:00:00+09:00"));
    }

    #[test]
    fn add_supports_date_and_duration() {
        let args = AddArgs {
            json: false,
            title: "作業".to_string(),
            public: false,
            private: false,
            start: None,
            end: None,
            date: Some("3/11".to_string()),
            at: Some("9:00".to_string()),
            until: None,
            duration: Some("2h".to_string()),
            all_day: false,
            description: None,
            attendees: Vec::new(),
            facility: None,
            calendar: None,
        };

        let event = args.new_event_from(anchor()).expect("event");
        assert_eq!(event.starts_at, ts("2026-03-11T09:00:00+09:00"));
        assert_eq!(event.ends_at, ts("2026-03-11T11:00:00+09:00"));
    }

    #[test]
    fn add_defaults_date_only_to_all_day() {
        let args = AddArgs {
            json: false,
            title: "休み".to_string(),
            public: false,
            private: false,
            start: None,
            end: None,
            date: Some("today".to_string()),
            at: None,
            until: None,
            duration: None,
            all_day: false,
            description: None,
            attendees: Vec::new(),
            facility: None,
            calendar: None,
        };

        let event = args.new_event_from(anchor()).expect("event");
        assert_eq!(event.starts_at, ts("2026-03-09T00:00:00+09:00"));
        assert_eq!(event.ends_at, ts("2026-03-10T00:00:00+09:00"));
    }

    #[test]
    fn list_supports_date_shortcut() {
        let args = ListArgs {
            json: false,
            from: None,
            to: None,
            date: Some("today".to_string()),
            duration: None,
        };

        let query = args.query_from(anchor()).expect("query");
        assert_eq!(query.from, Some(ts("2026-03-09T00:00:00+09:00")));
        assert_eq!(query.to, Some(ts("2026-03-10T00:00:00+09:00")));
    }

    #[test]
    fn list_supports_relative_range_from_today() {
        let args = ListArgs {
            json: false,
            from: Some("today".to_string()),
            to: None,
            date: None,
            duration: Some("7d".to_string()),
        };

        let query = args.query_from(anchor()).expect("query");
        assert_eq!(query.from, Some(ts("2026-03-09T00:00:00+09:00")));
        assert_eq!(query.to, Some(ts("2026-03-16T00:00:00+09:00")));
    }

    #[test]
    fn parses_compound_duration_test() {
        assert_eq!(
            parse_duration("2h30m").expect("duration"),
            TimeDelta::minutes(150)
        );
    }

    #[test]
    fn parses_relative_dates_test() {
        assert_eq!(
            parse_flexible_date("+3d", anchor()).expect("date"),
            NaiveDate::from_ymd_opt(2026, 3, 12).expect("date")
        );
        assert_eq!(
            parse_flexible_date("-1w", anchor()).expect("date"),
            NaiveDate::from_ymd_opt(2026, 3, 2).expect("date")
        );
    }

    #[test]
    fn rejects_mixing_strict_and_friendly_add_options() {
        let args = AddArgs {
            json: false,
            title: "混在".to_string(),
            public: false,
            private: false,
            start: Some(ts("2026-03-10T09:00:00+09:00")),
            end: Some(ts("2026-03-10T10:00:00+09:00")),
            date: Some("3/10".to_string()),
            at: None,
            until: None,
            duration: None,
            all_day: false,
            description: None,
            attendees: Vec::new(),
            facility: None,
            calendar: None,
        };

        let error = args.new_event_from(anchor()).expect_err("should fail");
        assert!(error.to_string().contains("同時に使えません"));
    }

    #[test]
    fn add_supports_private_visibility() {
        let args = AddArgs {
            json: false,
            title: "非公開打合せ".to_string(),
            public: false,
            private: true,
            start: None,
            end: None,
            date: Some("today".to_string()),
            at: Some("9".to_string()),
            until: Some("10".to_string()),
            duration: None,
            all_day: false,
            description: None,
            attendees: Vec::new(),
            facility: None,
            calendar: None,
        };

        let event = args.new_event_from(anchor()).expect("event");
        assert_eq!(event.visibility, EventVisibility::Private);
    }

    #[test]
    fn parses_global_verbose_count() {
        let cli = Cli::try_parse_from(["cbzcal", "-vv", "events", "list"]).expect("parse");
        assert_eq!(cli.verbose, 2);
    }

    #[test]
    fn events_without_subcommand_default_to_list_args() {
        let cli = Cli::try_parse_from(["cbzcal", "events", "--date", "today"]).expect("parse");
        let Command::Events(events) = cli.command else {
            panic!("events command");
        };
        assert!(events.command.is_none());
        assert_eq!(events.list.date.as_deref(), Some("today"));
    }

    #[test]
    fn prompt_mode_uses_long_option_prompt() {
        let cli =
            Cli::try_parse_from(["cbzcal", "events", "--prompt", "明日の予定"]).expect("parse");
        let Command::Events(events) = cli.command else {
            panic!("events command");
        };
        let ResolvedEventsArgs::Prompt(prompt) = events.resolve().expect("resolve") else {
            panic!("prompt mode");
        };
        assert_eq!(prompt.prompt, "明日の予定");
        assert!(!prompt.yes);
    }

    #[test]
    fn prompt_mode_rejects_list_filters() {
        let cli = Cli::try_parse_from([
            "cbzcal",
            "events",
            "--prompt",
            "明日の予定",
            "--date",
            "today",
        ])
        .expect("parse");
        let Command::Events(events) = cli.command else {
            panic!("events command");
        };
        let error = events.resolve().expect_err("should fail");
        assert!(error.to_string().contains("同時に使えません"));
    }
}
