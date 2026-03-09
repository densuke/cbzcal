use chrono::NaiveDate;

pub fn short_id_from_event_id(id: &str) -> String {
    let url = format!("https://example.invalid/?{id}");
    let Some(parsed) = reqwest::Url::parse(&url).ok() else {
        return id.to_string();
    };
    let mut seid = None;
    let mut date = None;
    for (key, value) in parsed.query_pairs() {
        match key.as_ref() {
            "sEID" => seid = Some(value.into_owned()),
            "Date" => date = Some(value.into_owned()),
            _ => {}
        }
    }

    match (seid, date.and_then(|value| normalize_da_date(&value))) {
        (Some(seid), Some(date)) => format!("{seid}@{date}"),
        _ => id.to_string(),
    }
}

fn normalize_da_date(value: &str) -> Option<String> {
    let value = value.strip_prefix("da.")?;
    let mut parts = value.split('.');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    Some(format!("{year:04}-{month:02}-{day:02}"))
}

pub fn extract_date_from_event_identifier(id: &str) -> Option<NaiveDate> {
    if let Some((_, date)) = id.split_once('@') {
        return NaiveDate::parse_from_str(date, "%Y-%m-%d").ok();
    }

    let url = reqwest::Url::parse(&format!("https://example.invalid/?{id}")).ok()?;
    for (key, value) in url.query_pairs() {
        if key == "Date" {
            return parse_da_date(&value);
        }
    }
    None
}

fn parse_da_date(value: &str) -> Option<NaiveDate> {
    let value = value.strip_prefix("da.")?;
    let mut parts = value.split('.');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    NaiveDate::from_ymd_opt(year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_id_uses_seid_and_date_for_cybozu_ids() {
        assert_eq!(
            short_id_from_event_id(
                "sEID=3096804&UID=379&GID=183&Date=da.2099.1.7&BDate=da.2099.1.5"
            ),
            "3096804@2099-01-07"
        );
    }

    #[test]
    fn short_id_falls_back_to_original_id() {
        assert_eq!(short_id_from_event_id("fixture-123"), "fixture-123");
    }

    #[test]
    fn extracts_date_from_short_id() {
        assert_eq!(
            extract_date_from_event_identifier("3096840@2026-03-09"),
            Some(NaiveDate::from_ymd_opt(2026, 3, 9).expect("date"))
        );
    }
}
