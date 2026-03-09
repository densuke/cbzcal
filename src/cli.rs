use std::path::PathBuf;

use anyhow::{Result, bail};
use chrono::{DateTime, Datelike, Days, FixedOffset, NaiveDate, TimeDelta, TimeZone, Utc};
use clap::ArgAction;
use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::backend::ListQuery;
use crate::model::{CloneOverrides, EventPatch, NewEvent};

const JST_OFFSET_SECONDS: i32 = 9 * 60 * 60;

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
    Events(EventsArgs),
}

#[derive(Debug, Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct EventsArgs {
    #[command(subcommand)]
    pub command: Option<EventsCommand>,
    #[command(flatten)]
    pub list: ListArgs,
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
        resolve_list_query(self, anchor).map_err(|error| anyhow::anyhow!(error))
    }
}

#[derive(Debug, Args, Clone)]
pub struct AddArgs {
    #[arg(long, help = "JSON 形式で出力する")]
    pub json: bool,
    #[arg(long)]
    pub title: String,
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
        resolve_add_event(self, anchor).map_err(|error| anyhow::anyhow!(error))
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
        let date = parse_flexible_date(date, anchor)?;
        let from = to_jst_datetime(date, 0, 0)?;
        let to = if let Some(duration) = &args.duration {
            from + parse_duration(duration)?
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
        .map(|value| parse_flexible_datetime(value, anchor))
        .transpose()?;
    let to = if let Some(duration) = &args.duration {
        let from =
            from.ok_or_else(|| "`--for` を使う場合は `--from` か `--date` が必要です".to_string())?;
        Some(from + parse_duration(duration)?)
    } else {
        args.to
            .as_deref()
            .map(|value| parse_flexible_datetime(value, anchor))
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
        let date = parse_flexible_date(date, anchor)?;

        if args.all_day || (args.at.is_none() && args.until.is_none() && args.duration.is_none()) {
            if args.at.is_some() || args.until.is_some() || args.duration.is_some() {
                return Err(
                    "`--all-day` を使う場合は `--at` / `--until` / `--for` を指定できません"
                        .to_string(),
                );
            }
            (
                to_jst_datetime(date, 0, 0)?,
                to_jst_datetime(next_date(date)?, 0, 0)?,
            )
        } else {
            let at = args
                .at
                .as_deref()
                .ok_or_else(|| "`--at` を指定してください".to_string())?;
            if args.until.is_some() && args.duration.is_some() {
                return Err("`--until` と `--for` はどちらか一方だけ指定してください".to_string());
            }
            let (start_hour, start_minute) = parse_time_of_day(at)?;
            let starts_at = to_jst_datetime(date, start_hour, start_minute)?;
            let ends_at = if let Some(until) = &args.until {
                let (end_hour, end_minute) = parse_time_of_day(until)?;
                to_jst_datetime(date, end_hour, end_minute)?
            } else if let Some(duration) = &args.duration {
                starts_at + parse_duration(duration)?
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
    };
    event.validate().map_err(|error| error.to_string())?;

    Ok(event)
}

fn parse_flexible_datetime(
    input: &str,
    anchor: NaiveDate,
) -> Result<DateTime<FixedOffset>, String> {
    DateTime::parse_from_rfc3339(input).or_else(|_| {
        let date = parse_flexible_date(input, anchor)?;
        to_jst_datetime(date, 0, 0).map_err(|error| error.to_string())
    })
}

fn parse_flexible_date(input: &str, anchor: NaiveDate) -> Result<NaiveDate, String> {
    let normalized = input.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "today" => return Ok(anchor),
        "tomorrow" => return next_date(anchor).map_err(|error| error.to_string()),
        "yesterday" => {
            return anchor
                .checked_sub_days(Days::new(1))
                .ok_or_else(|| "日付を計算できません".to_string());
        }
        _ => {}
    }

    if let Some(date) = parse_relative_date(&normalized, anchor)? {
        return Ok(date);
    }

    NaiveDate::parse_from_str(&normalized, "%Y-%m-%d")
        .or_else(|_| NaiveDate::parse_from_str(&normalized, "%Y/%m/%d"))
        .or_else(|_| parse_month_day(&normalized, anchor.year()))
        .map_err(|_| {
            format!(
                "日付は today/tomorrow/yesterday/+3d/3/10/2026-03-10 のいずれかで指定してください: {input}"
            )
        })
}

fn parse_relative_date(input: &str, anchor: NaiveDate) -> Result<Option<NaiveDate>, String> {
    let Some(sign) = input.chars().next().filter(|ch| *ch == '+' || *ch == '-') else {
        return Ok(None);
    };
    let unit = input
        .chars()
        .last()
        .ok_or_else(|| "相対日付を解釈できません".to_string())?;
    let magnitude = input[1..input.len().saturating_sub(1)]
        .parse::<u64>()
        .map_err(|_| format!("相対日付を解釈できません: {input}"))?;
    let days = match unit {
        'd' => magnitude,
        'w' => magnitude
            .checked_mul(7)
            .ok_or_else(|| format!("相対日付が大きすぎます: {input}"))?,
        _ => return Ok(None),
    };

    let date = if sign == '+' {
        anchor
            .checked_add_days(Days::new(days))
            .ok_or_else(|| format!("相対日付を計算できません: {input}"))?
    } else {
        anchor
            .checked_sub_days(Days::new(days))
            .ok_or_else(|| format!("相対日付を計算できません: {input}"))?
    };
    Ok(Some(date))
}

fn parse_month_day(input: &str, year: i32) -> Result<NaiveDate, chrono::ParseError> {
    NaiveDate::parse_from_str(&format!("{year}/{input}"), "%Y/%m/%d")
}

fn parse_time_of_day(input: &str) -> Result<(u32, u32), String> {
    let normalized = input.trim();
    if let Some((hour, minute)) = normalized.split_once(':') {
        let hour = hour
            .parse::<u32>()
            .map_err(|_| format!("時刻を解釈できません: {input}"))?;
        let minute = minute
            .parse::<u32>()
            .map_err(|_| format!("時刻を解釈できません: {input}"))?;
        if hour > 23 || minute > 59 {
            return Err(format!("時刻を解釈できません: {input}"));
        }
        return Ok((hour, minute));
    }

    let hour = normalized
        .parse::<u32>()
        .map_err(|_| format!("時刻は 9 または 9:30 のように指定してください: {input}"))?;
    if hour > 23 {
        return Err(format!("時刻を解釈できません: {input}"));
    }
    Ok((hour, 0))
}

fn parse_duration(input: &str) -> Result<TimeDelta, String> {
    let value = input.trim().to_ascii_lowercase();
    if value.is_empty() {
        return Err("期間が空です".to_string());
    }

    let mut total = TimeDelta::zero();
    let mut cursor = 0usize;
    let bytes = value.as_bytes();
    while cursor < bytes.len() {
        let start = cursor;
        while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
            cursor += 1;
        }
        if start == cursor || cursor >= bytes.len() {
            return Err(format!(
                "期間は 30m / 2h / 2h30m / 7d のように指定してください: {input}"
            ));
        }
        let amount = value[start..cursor]
            .parse::<i64>()
            .map_err(|_| format!("期間を解釈できません: {input}"))?;
        let unit = bytes[cursor] as char;
        cursor += 1;
        let delta = match unit {
            'm' => TimeDelta::minutes(amount),
            'h' => TimeDelta::hours(amount),
            'd' => TimeDelta::days(amount),
            _ => return Err(format!("期間を解釈できません: {input}")),
        };
        total += delta;
    }

    if total <= TimeDelta::zero() {
        return Err("期間は 0 より大きい必要があります".to_string());
    }

    Ok(total)
}

fn to_jst_datetime(
    date: NaiveDate,
    hour: u32,
    minute: u32,
) -> Result<DateTime<FixedOffset>, String> {
    jst_offset()
        .with_ymd_and_hms(date.year(), date.month(), date.day(), hour, minute, 0)
        .single()
        .ok_or_else(|| "日時を構築できません".to_string())
}

fn next_date(date: NaiveDate) -> Result<NaiveDate, String> {
    date.checked_add_days(Days::new(1))
        .ok_or_else(|| "翌日を計算できません".to_string())
}

fn current_jst_date() -> NaiveDate {
    Utc::now().with_timezone(&jst_offset()).date_naive()
}

fn jst_offset() -> FixedOffset {
    FixedOffset::east_opt(JST_OFFSET_SECONDS).expect("valid JST offset")
}

fn parse_timestamp(input: &str) -> Result<DateTime<FixedOffset>, String> {
    DateTime::parse_from_rfc3339(input)
        .map_err(|error| format!("RFC3339 形式の日時で指定してください: {error}"))
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
    fn parses_compound_duration() {
        assert_eq!(
            parse_duration("2h30m").expect("duration"),
            TimeDelta::minutes(150)
        );
    }

    #[test]
    fn parses_relative_dates() {
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
}
