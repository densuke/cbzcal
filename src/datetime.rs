use anyhow::{Result, bail};
use chrono::{
    DateTime, Datelike, Days, FixedOffset, NaiveDate, NaiveDateTime, TimeDelta, TimeZone, Utc,
};

pub const JST_OFFSET_SECONDS: i32 = 9 * 60 * 60;

pub fn jst_offset() -> FixedOffset {
    FixedOffset::east_opt(JST_OFFSET_SECONDS).expect("valid JST offset")
}

pub fn current_jst_date() -> NaiveDate {
    Utc::now().with_timezone(&jst_offset()).date_naive()
}

pub fn current_jst_midnight() -> DateTime<FixedOffset> {
    let offset = jst_offset();
    let today = current_jst_date();
    let midnight = today.and_hms_opt(0, 0, 0).expect("midnight");
    offset
        .from_local_datetime(&midnight)
        .single()
        .expect("valid local midnight")
}

pub fn to_jst_datetime(date: NaiveDate, hour: u32, minute: u32) -> Result<DateTime<FixedOffset>> {
    jst_offset()
        .with_ymd_and_hms(date.year(), date.month(), date.day(), hour, minute, 0)
        .single()
        .ok_or_else(|| anyhow::anyhow!("日時を構築できません"))
}

pub fn next_date(date: NaiveDate) -> Result<NaiveDate> {
    date.checked_add_days(Days::new(1))
        .ok_or_else(|| anyhow::anyhow!("翌日を計算できません"))
}

pub fn parse_timestamp(input: &str) -> Result<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(input)
        .map_err(|error| anyhow::anyhow!("RFC3339 形式の日時で指定してください: {error}"))
}

pub fn parse_time_of_day(input: &str) -> Result<(u32, u32)> {
    let normalized = input.trim();
    if let Some((hour, minute)) = normalized.split_once(':') {
        let hour = hour
            .parse::<u32>()
            .map_err(|_| anyhow::anyhow!("時刻を解釈できません: {input}"))?;
        let minute = minute
            .parse::<u32>()
            .map_err(|_| anyhow::anyhow!("時刻を解釈できません: {input}"))?;
        if hour > 23 || minute > 59 {
            bail!("時刻を解釈できません: {input}");
        }
        return Ok((hour, minute));
    }

    let hour = normalized
        .parse::<u32>()
        .map_err(|_| anyhow::anyhow!("時刻は 9 または 9:30 のように指定してください: {input}"))?;
    if hour > 23 {
        bail!("時刻を解釈できません: {input}");
    }
    Ok((hour, 0))
}

pub fn parse_duration(input: &str) -> Result<TimeDelta> {
    let value = input.trim().to_ascii_lowercase();
    if value.is_empty() {
        bail!("期間が空です");
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
            bail!("期間は 30m / 2h / 2h30m / 7d のように指定してください: {input}");
        }
        let amount = value[start..cursor]
            .parse::<i64>()
            .map_err(|_| anyhow::anyhow!("期間を解釈できません: {input}"))?;
        let unit = bytes[cursor] as char;
        cursor += 1;
        let delta = match unit {
            'm' => TimeDelta::minutes(amount),
            'h' => TimeDelta::hours(amount),
            'd' => TimeDelta::days(amount),
            _ => bail!("期間を解釈できません: {input}"),
        };
        total += delta;
    }

    if total <= TimeDelta::zero() {
        bail!("期間は 0 より大きい必要があります");
    }

    Ok(total)
}

pub fn parse_flexible_date(input: &str, anchor: NaiveDate) -> Result<NaiveDate> {
    let normalized = input.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "today" => return Ok(anchor),
        "tomorrow" => return next_date(anchor),
        "yesterday" => {
            return anchor
                .checked_sub_days(Days::new(1))
                .ok_or_else(|| anyhow::anyhow!("日付を計算できません"));
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
            anyhow::anyhow!(
                "日付は today/tomorrow/yesterday/+3d/3/10/2026-03-10 のいずれかで指定してください: {input}"
            )
        })
}

fn parse_relative_date(input: &str, anchor: NaiveDate) -> Result<Option<NaiveDate>> {
    let Some(sign) = input.chars().next().filter(|ch| *ch == '+' || *ch == '-') else {
        return Ok(None);
    };
    let unit = input
        .chars()
        .last()
        .ok_or_else(|| anyhow::anyhow!("相対日付を解釈できません"))?;
    let magnitude = input[1..input.len().saturating_sub(1)]
        .parse::<u64>()
        .map_err(|_| anyhow::anyhow!("相対日付を解釈できません: {input}"))?;
    let days = match unit {
        'd' => magnitude,
        'w' => magnitude
            .checked_mul(7)
            .ok_or_else(|| anyhow::anyhow!("相対日付が大きすぎます: {input}"))?,
        _ => return Ok(None),
    };

    let date = if sign == '+' {
        anchor
            .checked_add_days(Days::new(days))
            .ok_or_else(|| anyhow::anyhow!("相対日付を計算できません: {input}"))?
    } else {
        anchor
            .checked_sub_days(Days::new(days))
            .ok_or_else(|| anyhow::anyhow!("相対日付を計算できません: {input}"))?
    };
    Ok(Some(date))
}

fn parse_month_day(input: &str, year: i32) -> Result<NaiveDate, chrono::ParseError> {
    NaiveDate::parse_from_str(&format!("{year}/{input}"), "%Y/%m/%d")
}

pub fn parse_flexible_datetime(input: &str, anchor: NaiveDate) -> Result<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(input).or_else(|_| {
        let date = parse_flexible_date(input, anchor)?;
        to_jst_datetime(date, 0, 0)
    })
}

// Prompt specific normalizations
pub fn normalize_prompt_time(input: &str) -> String {
    let mut normalized = input.trim().replace("時半", ":30");
    normalized = normalized.replace("時", ":");
    normalized = normalized.replace("分", "");
    normalized = normalized.replace('：', ":");
    if let Some(stripped) = normalized
        .strip_suffix('z')
        .or_else(|| normalized.strip_suffix('Z'))
    {
        normalized = stripped.to_string();
    }
    normalized = strip_trailing_timezone_offset(&normalized).to_string();
    let parts = normalized.split(':').collect::<Vec<_>>();
    if parts.len() >= 3 {
        normalized = format!("{}:{}", parts[0], parts[1]);
    }
    if normalized.ends_with(':') {
        normalized.pop();
    }
    normalized
}

pub fn strip_trailing_timezone_offset(input: &str) -> &str {
    if input.len() < 6 {
        return input;
    }
    let suffix = &input[input.len() - 6..];
    let bytes = suffix.as_bytes();
    let is_offset = matches!(bytes[0], b'+' | b'-')
        && bytes[1].is_ascii_digit()
        && bytes[2].is_ascii_digit()
        && bytes[3] == b':'
        && bytes[4].is_ascii_digit()
        && bytes[5].is_ascii_digit();
    if is_offset {
        &input[..input.len() - 6]
    } else {
        input
    }
}

pub fn normalize_prompt_duration(input: &str) -> String {
    input
        .trim()
        .replace("時間", "h")
        .replace("時", "h")
        .replace("分", "m")
        .replace("日", "d")
        .replace('＋', "+")
        .replace('　', "")
}

pub fn jst_from_naive(naive: NaiveDateTime) -> Result<DateTime<FixedOffset>> {
    jst_offset()
        .from_local_datetime(&naive)
        .single()
        .ok_or_else(|| anyhow::anyhow!("日時を構築できません"))
}

pub fn weekday_abbr(weekday: chrono::Weekday) -> &'static str {
    match weekday {
        chrono::Weekday::Mon => "Mon",
        chrono::Weekday::Tue => "Tue",
        chrono::Weekday::Wed => "Wed",
        chrono::Weekday::Thu => "Thu",
        chrono::Weekday::Fri => "Fri",
        chrono::Weekday::Sat => "Sat",
        chrono::Weekday::Sun => "Sun",
    }
}

pub fn parse_prompt_timestamp(
    input: &str,
    anchor: NaiveDate,
    context_date: Option<&str>,
) -> Result<DateTime<FixedOffset>> {
    if let Ok(timestamp) = parse_timestamp(input) {
        return Ok(timestamp);
    }

    let normalized = input.trim().replace('　', " ");
    for format in [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%Y/%m/%d %H:%M:%S",
        "%Y/%m/%d %H:%M",
    ] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(&normalized, format) {
            return jst_from_naive(naive);
        }
    }

    if let Ok(date) = parse_flexible_date(&normalized, anchor) {
        return to_jst_datetime(date, 0, 0);
    }

    if let Some(context_date) = context_date {
        let date = parse_flexible_date(context_date, anchor)?;
        let (hour, minute) = parse_time_of_day(&normalize_prompt_time(&normalized))?;
        return to_jst_datetime(date, hour, minute);
    }

    bail!(
        "日時を解釈できませんでした: {input}. RFC3339 に加えて `2026-03-10 17:30` や `17:30` + `date` を受け付けます"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 3, 9).expect("date")
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
}
