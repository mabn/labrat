//! A port of `gitlab_chronic_duration` 0.12 as `Gitlab::TimeTrackingFormatter` uses it: 8-hour
//! days, 20-day months, `default_unit: 'hours'`, `raise_exceptions = true` (an unknown word
//! makes the parse fail), and the `:short` output with weeks.

/// A parsed duration: Ruby keeps integers exact (Bignum) and turns the sum into a Float as
/// soon as one number has a fraction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Num {
    Int(i128),
    Float(f64),
}

impl Num {
    pub fn is_zero(self) -> bool {
        match self {
            Num::Int(i) => i == 0,
            Num::Float(f) => f == 0.0,
        }
    }

    pub fn is_negative(self) -> bool {
        match self {
            Num::Int(i) => i < 0,
            Num::Float(f) => f < 0.0,
        }
    }

    fn neg(self) -> Num {
        match self {
            Num::Int(i) => Num::Int(i.checked_neg().unwrap_or(i128::MAX)),
            Num::Float(f) => Num::Float(-f),
        }
    }

    /// Ruby's `to_i`, saturated to 64 bits (what a bigint column can hold).
    pub fn to_i64(self) -> i64 {
        match self {
            Num::Int(i) => i.clamp(i64::MIN as i128, i64::MAX as i128) as i64,
            Num::Float(f) => f as i64,
        }
    }

    /// As JSON: an integer if it fits 64 bits, otherwise a float (as GitLab renders Bignums).
    pub fn to_json(self) -> serde_json::Value {
        match self {
            Num::Int(i) if i64::try_from(i).is_ok() => serde_json::json!(i as i64),
            Num::Int(i) => serde_json::json!(i as f64),
            Num::Float(f) => serde_json::json!(f),
        }
    }
}

const HOURS_PER_DAY: i128 = 8;
const DAYS_PER_MONTH: i128 = 20;
const FULL_WEEKS_PER_MONTH: i128 = 4;
const YEAR: i128 = 31_557_600;

fn unit_seconds(unit: &str) -> i128 {
    let days_per_week = DAYS_PER_MONTH / FULL_WEEKS_PER_MONTH;
    match unit {
        "years" => YEAR,
        "months" => 3600 * HOURS_PER_DAY * DAYS_PER_MONTH,
        "weeks" => 3600 * HOURS_PER_DAY * days_per_week,
        "days" => 3600 * HOURS_PER_DAY,
        "hours" => 3600,
        "minutes" => 60,
        "seconds" => 1,
        _ => 0,
    }
}

fn mapping(word: &str) -> Option<&'static str> {
    Some(match word {
        "seconds" | "second" | "secs" | "sec" | "s" => "seconds",
        "minutes" | "minute" | "mins" | "min" | "m" => "minutes",
        "hours" | "hour" | "hrs" | "hr" | "h" => "hours",
        "days" | "day" | "dy" | "d" => "days",
        "weeks" | "week" | "wks" | "wk" | "w" => "weeks",
        "months" | "mo" | "mos" | "month" => "months",
        "years" | "year" | "yrs" | "yr" | "y" => "years",
        _ => return None,
    })
}

fn is_number(word: &str) -> bool {
    // `float_matcher` (/[0-9]*\.?[0-9]+/) matches anywhere in the word.
    word.chars().any(|c| c.is_ascii_digit())
}

/// `filter_by_type`: `1:30:00` style input becomes units, right to left (no weeks).
fn filter_by_type(s: &str) -> String {
    let compact: String = s.chars().filter(|c| *c != ' ').collect();
    let (main, frac) = match compact.find('.') {
        Some(i) => (&compact[..i], Some(&compact[i..])),
        None => (&compact[..], None),
    };
    let parts: Vec<&str> = main.split(':').collect();
    let time = parts.len() >= 2
        && parts.len() <= 6
        && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        && frac.is_none_or(|f| f[1..].chars().all(|c| c.is_ascii_digit()));
    if !time {
        return s.to_string();
    }
    let units = ["seconds", "minutes", "hours", "days", "months", "years"];
    let mut all: Vec<String> = compact.split(':').map(str::to_string).collect();
    all.reverse();
    let mut res: Vec<String> = all.iter().enumerate().map(|(k, v)| format!("{v} {}", units[k])).collect();
    res.reverse();
    res.join(" ")
}

/// Splits numbers from letters (`1h30m` -> `1 h 30 m`), like `gsub(float_matcher) { " #{n} " }`.
fn space_numbers(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let start = i;
        let mut j = i;
        while j < chars.len() && chars[j].is_ascii_digit() {
            j += 1;
        }
        if j < chars.len() && chars[j] == '.' && j + 1 < chars.len() && chars[j + 1].is_ascii_digit() {
            j += 1;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
        }
        if j > start && chars[start..j].iter().any(|c| c.is_ascii_digit()) {
            out.push(' ');
            out.extend(&chars[start..j]);
            out.push(' ');
            i = j;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out.split(' ').filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")
}

fn convert(word: &str) -> Option<Num> {
    // `string.to_f % 1 > 0 ? string.to_f : string.to_i`
    let digits: String = word.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let f: f64 = digits.parse().ok()?;
    if f % 1.0 > 0.0 {
        return Some(Num::Float(f));
    }
    let int_part = digits.split('.').next().unwrap_or("0");
    Some(match int_part.parse::<i128>() {
        Ok(i) => Num::Int(i),
        Err(_) => Num::Float(f),
    })
}

fn add(a: Num, b: Num) -> Num {
    match (a, b) {
        (Num::Int(x), Num::Int(y)) => x.checked_add(y).map(Num::Int).unwrap_or(Num::Float(x as f64 + y as f64)),
        (x, y) => Num::Float(as_f64(x) + as_f64(y)),
    }
}

fn mul(a: Num, m: i128) -> Num {
    match a {
        Num::Int(x) => x.checked_mul(m).map(Num::Int).unwrap_or(Num::Float(x as f64 * m as f64)),
        Num::Float(f) => Num::Float(f * m as f64),
    }
}

fn as_f64(n: Num) -> f64 {
    match n {
        Num::Int(i) => i as f64,
        Num::Float(f) => f,
    }
}

/// `ChronicDuration.parse(string, hours_per_day: 8, days_per_month: 20, default_unit: 'hours')`
/// with exceptions raised: `None` for an unknown word, or for zero unless `keep_zero`.
pub fn parse(s: &str, keep_zero: bool) -> Option<Num> {
    let lower = s.to_lowercase();
    let typed = filter_by_type(&lower);
    let spaced = space_numbers(&typed);
    let mut words: Vec<String> = vec![];
    for word in spaced.split(' ').filter(|w| !w.is_empty()) {
        if is_number(word) {
            words.push(word.trim().to_string());
            continue;
        }
        let stripped = word.trim().trim_start_matches(',').trim_end_matches(',');
        match mapping(stripped) {
            Some(u) => words.push(u.to_string()),
            None if matches!(stripped, "and" | "with" | "plus") => {}
            None => return None,
        }
    }
    if words.first().is_some_and(|w| mapping(w).is_some()) {
        words.insert(0, "1".into());
    }
    let mut val = Num::Int(0);
    for (k, w) in words.iter().enumerate() {
        if !is_number(w) {
            continue;
        }
        let n = convert(w)?;
        let unit = words.get(k + 1).map(String::as_str).unwrap_or("hours");
        val = add(val, mul(n, unit_seconds(unit)));
    }
    if val.is_zero() && !keep_zero {
        return None;
    }
    Some(val)
}

/// `Gitlab::TimeTrackingFormatter.parse`: a leading `-` negates.
pub fn parse_time(s: &str, keep_zero: bool) -> Option<Num> {
    let (neg, rest) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s),
    };
    let v = parse(rest, keep_zero)?;
    Some(if neg { v.neg() } else { v })
}

/// `Gitlab::TimeTrackingFormatter.output` (`:short`, weeks, not limited to hours); `None` for
/// zero.
pub fn output(seconds: i128) -> Option<String> {
    if seconds < 0 {
        return output(seconds.checked_neg().unwrap_or(i128::MAX)).map(|s| format!("-{s}"));
    }
    let minute = 60i128;
    let hour = 60 * minute;
    let day = HOURS_PER_DAY * hour;
    let month = DAYS_PER_MONTH * day;
    let days_per_week = DAYS_PER_MONTH / FULL_WEEKS_PER_MONTH;
    let (mut years, mut months, mut weeks, mut days, mut hours, mut minutes) = (0i128, 0i128, 0i128, 0i128, 0i128, 0i128);
    let mut secs = seconds;
    if secs >= YEAR && secs % YEAR < secs % month {
        years = secs / YEAR;
        months = secs % YEAR / month;
        days = secs % YEAR % month / day;
        hours = secs % YEAR % month % day / hour;
        minutes = secs % YEAR % month % day % hour / minute;
        secs = secs % YEAR % month % day % hour % minute;
    } else if secs >= 60 {
        minutes = secs / 60;
        secs %= 60;
        if minutes >= 60 {
            hours = minutes / 60;
            minutes %= 60;
            if hours >= HOURS_PER_DAY {
                days = hours / HOURS_PER_DAY;
                hours %= HOURS_PER_DAY;
                if days >= days_per_week {
                    weeks = days / days_per_week;
                    days %= days_per_week;
                    if weeks >= FULL_WEEKS_PER_MONTH {
                        months = weeks / FULL_WEEKS_PER_MONTH;
                        weeks %= FULL_WEEKS_PER_MONTH;
                    }
                }
            }
        }
    }
    let parts: Vec<String> = [(years, "y"), (months, "mo"), (weeks, "w"), (days, "d"), (hours, "h"), (minutes, "m"), (secs, "s")]
        .iter()
        .filter(|(n, _)| *n != 0)
        .map(|(n, u)| format!("{n}{u}"))
        .collect();
    if parts.is_empty() { None } else { Some(parts.join(" ")) }
}

/// A duration in a system note or quick-action message: the absolute value (`subtracted 1h`).
pub fn human(secs: i64) -> String {
    output(secs.unsigned_abs() as i128).unwrap_or_default()
}

/// A `human_*` API or webhook field: signed, `null` for zero.
pub fn human_value(secs: i64) -> serde_json::Value {
    output(secs as i128).map(serde_json::Value::String).unwrap_or(serde_json::Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_like_gitlab() {
        assert_eq!(parse_time("1y", false), Some(Num::Int(31_557_600)));
        assert_eq!(parse_time("1mo1mo1mo", false), Some(Num::Int(1_728_000)));
        assert_eq!(parse_time("3", false), Some(Num::Int(10_800)));
        assert_eq!(parse_time("1.5h", false), Some(Num::Float(5400.0)));
        assert_eq!(parse_time("-2h", false), Some(Num::Int(-7200)));
        assert_eq!(parse_time("1h 30m", false), Some(Num::Int(5400)));
        assert_eq!(parse_time("1:30", false), Some(Num::Int(90)));
        assert_eq!(parse_time("1x", false), None);
        assert_eq!(parse_time("0", false), None);
        assert_eq!(parse_time("999999999999w", false), Some(Num::Int(143_999_999_999_856_000)));
        assert_eq!(parse_time("333333333333333333333333333333m", false), Some(Num::Int(19_999_999_999_999_999_999_999_999_999_980)));
    }

    #[test]
    fn outputs_like_gitlab() {
        assert_eq!(output(31_557_600).as_deref(), Some("1y"));
        assert_eq!(output(35_013_600).as_deref(), Some("60mo 3w 6h"));
        assert_eq!(output(2_147_483_647).as_deref(), Some("3728mo 1w 3h 14m 7s"));
        assert_eq!(output(1_728_000).as_deref(), Some("3mo"));
        assert_eq!(output(9_223_372_036_854_775_807).as_deref(), Some("16012798675095mo 1d 7h 30m 7s"));
        assert_eq!(output(143_999_999_999_856_000).as_deref(), Some("249999999999mo 3w"));
        assert_eq!(output(19_999_999_999_999_999_999_999_999_999_980).as_deref(), Some("34722222222222222222222222mo 4d 3h 33m"));
        assert_eq!(output(66_571_200).as_deref(), Some("115mo 2w 1d 4h"));
        assert_eq!(output(0), None);
    }
}
