//! Pure helpers for the spend pages: money and token text, dates, bar heights and shares.

use crate::api::DayCost;

/// "$12.40", "$1,234.50".
pub fn money(usd: f64) -> String {
    let cents = (usd.max(0.) * 100.).round() as u64;
    format!("${}.{:02}", group(cents / 100), cents % 100)
}

/// Thousands separated by commas: 1234567 → "1,234,567".
fn group(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Short token count: 950, "12.3K", "1.2M", "4B".
pub fn tokens(n: i64) -> String {
    let n = n.max(0) as f64;
    let (v, unit) = match n {
        x if x >= 1e9 => (x / 1e9, "B"),
        x if x >= 1e6 => (x / 1e6, "M"),
        x if x >= 1e3 => (x / 1e3, "K"),
        x => return format!("{x}"),
    };
    let text = format!("{v:.1}");
    format!("{}{unit}", text.strip_suffix(".0").unwrap_or(&text))
}

/// Header totals: "$12.40 · 1.2M tokens".
pub fn totals_line(usd: f64, toks: i64) -> String {
    format!("{} \u{b7} {} tokens", money(usd), tokens(toks))
}

/// Bar heights scaled so the largest value fills `max_h`; all zero when nothing was spent.
pub fn bar_heights(values: &[f64], max_h: f32) -> Vec<f32> {
    let top = values.iter().copied().fold(0., f64::max);
    if top <= 0. {
        return vec![0.; values.len()];
    }
    values.iter().map(|v| (v.max(0.) / top) as f32 * max_h).collect()
}

/// Share of `part` in `total`, from 0 to 1.
pub fn share(part: f64, total: f64) -> f32 {
    if total <= 0. { 0. } else { (part / total).clamp(0., 1.) as f32 }
}

/// Days since 1970-01-01 for a civil date (proleptic Gregorian).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

/// The civil date (year, month, day) of a day number.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Day number of "YYYY-MM-DD".
fn parse_day(date: &str) -> Option<i64> {
    let mut it = date.get(..10)?.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, d) = (it.next()??, it.next()??, it.next()??);
    Some(days_from_civil(y, m, d))
}

/// "Nov 18" for a day number.
pub fn short_date(day: i64) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let (_, m, d) = civil_from_days(day);
    format!("{} {d}", MONTHS[(m - 1) as usize])
}

/// Today's day number in UTC, the zone the daemon groups days in.
pub fn today() -> i64 {
    crate::store::now_secs().div_euclid(86_400)
}

/// One (day, usd) per day of the window ending `today`, zero on days with no spend.
pub fn fill_days(by_day: &[DayCost], today: i64, days: u32) -> Vec<(i64, f64)> {
    let first = today - i64::from(days) + 1;
    let usd = |day: i64| by_day.iter().filter(|d| parse_day(&d.date) == Some(day)).map(|d| d.usd).sum();
    (first..=today).map(|day| (day, usd(day))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_has_cents_and_commas() {
        assert_eq!(money(12.4), "$12.40");
        assert_eq!(money(0.004), "$0.00");
        assert_eq!(money(1234.5), "$1,234.50");
        assert_eq!(money(1_234_567.0), "$1,234,567.00");
    }

    #[test]
    fn tokens_are_short() {
        assert_eq!(tokens(950), "950");
        assert_eq!(tokens(12_340), "12.3K");
        assert_eq!(tokens(1_200_000), "1.2M");
        assert_eq!(tokens(2_000_000), "2M");
        assert_eq!(totals_line(12.4, 1_200_000), "$12.40 \u{b7} 1.2M tokens");
    }

    #[test]
    fn bars_scale_to_the_largest() {
        assert_eq!(bar_heights(&[1., 2., 4.], 80.), vec![20., 40., 80.]);
        assert_eq!(bar_heights(&[0., 0.], 80.), vec![0., 0.]);
        assert!(bar_heights(&[], 80.).is_empty());
    }

    #[test]
    fn share_is_bounded() {
        assert_eq!(share(1., 4.), 0.25);
        assert_eq!(share(1., 0.), 0.);
    }

    #[test]
    fn dates_round_trip() {
        let day = parse_day("2026-11-18").unwrap();
        assert_eq!(civil_from_days(day), (2026, 11, 18));
        assert_eq!(short_date(day), "Nov 18");
        assert_eq!(parse_day("1970-01-01"), Some(0));
        assert_eq!(short_date(parse_day("2024-02-29").unwrap()), "Feb 29");
    }

    #[test]
    fn window_fills_missing_days_with_zero() {
        let today = parse_day("2026-11-18").unwrap();
        let by_day = vec![DayCost { date: "2026-11-17".into(), usd: 2., tokens: 5 }];
        let days = fill_days(&by_day, today, 3);
        assert_eq!(days.iter().map(|d| d.1).collect::<Vec<_>>(), vec![0., 2., 0.]);
        assert_eq!(short_date(days[0].0), "Nov 16");
    }
}
