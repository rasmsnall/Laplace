//! public holidays and business days in sweden and finland

use chrono::{Datelike, NaiveDate, Weekday};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Country {
    Sweden,
    Finland,
}

impl Country {
    pub fn parse(code: &str) -> Option<Self> {
        match code.trim().to_ascii_lowercase().as_str() {
            "se" => Some(Country::Sweden),
            "fi" => Some(Country::Finland),
            _ => None,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Country::Sweden => "se",
            Country::Finland => "fi",
        }
    }
}

/// a weekday that is not a public holiday, or a day off in practice, in any of the countries
pub fn is_business_day(date: NaiveDate, countries: &[Country]) -> bool {
    !matches!(date.weekday(), Weekday::Sat | Weekday::Sun)
        && !countries.iter().any(|&country| is_holiday(date, country))
}

pub fn is_holiday(date: NaiveDate, country: Country) -> bool {
    holidays(date.year(), country).contains(&date)
}

/// includes midsummer eve, christmas eve and (in sweden) new year's eve: not public
/// holidays by law, but days when offices and banks are closed
fn holidays(year: i32, country: Country) -> Vec<NaiveDate> {
    let day = |month, day| NaiveDate::from_ymd_opt(year, month, day).expect("valid date");
    let easter = easter_sunday(year);
    let midsummer_eve = first(Weekday::Fri, day(6, 19));
    let all_saints = first(Weekday::Sat, day(10, 31));

    let mut days = vec![
        day(1, 1),
        day(1, 6),
        easter - chrono::Days::new(2),
        easter,
        easter + chrono::Days::new(1),
        day(5, 1),
        easter + chrono::Days::new(39),
        easter + chrono::Days::new(49),
        midsummer_eve,
        midsummer_eve + chrono::Days::new(1),
        all_saints,
        day(12, 24),
        day(12, 25),
        day(12, 26),
    ];
    match country {
        Country::Sweden => days.extend([day(6, 6), day(12, 31)]),
        Country::Finland => days.push(day(12, 6)),
    }
    days
}

/// the first `weekday` on or after `from`
fn first(weekday: Weekday, from: NaiveDate) -> NaiveDate {
    let ahead = (7 + weekday.num_days_from_monday() - from.weekday().num_days_from_monday()) % 7;
    from + chrono::Days::new(u64::from(ahead))
}

/// the anonymous gregorian algorithm
fn easter_sunday(year: i32) -> NaiveDate {
    let a = year % 19;
    let b = year / 100;
    let c = year % 100;
    let d = b / 4;
    let e = b % 4;
    let f = (b + 8) / 25;
    let g = (b - f + 1) / 3;
    let h = (19 * a + b - d - g + 15) % 30;
    let i = c / 4;
    let k = c % 4;
    let l = (32 + 2 * e + 2 * i - h - k) % 7;
    let m = (a + 11 * h + 22 * l) / 451;
    let month = (h + l - 7 * m + 114) / 31;
    let day = (h + l - 7 * m + 114) % 31 + 1;
    NaiveDate::from_ymd_opt(year, month as u32, day as u32).expect("easter is a valid date")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn easter_matches_published_dates() {
        assert_eq!(easter_sunday(2025), date(2025, 4, 20));
        assert_eq!(easter_sunday(2026), date(2026, 4, 5));
        assert_eq!(easter_sunday(2027), date(2027, 3, 28));
    }

    #[test]
    fn swedish_holidays_2026() {
        let se = Country::Sweden;
        for (m, d) in [
            (1, 6),
            (4, 3),
            (4, 6),
            (5, 14),
            (6, 6),
            (6, 19),
            (6, 20),
            (10, 31),
            (12, 24),
            (12, 31),
        ] {
            assert!(
                is_holiday(date(2026, m, d), se),
                "2026-{m}-{d} should be a swedish holiday"
            );
        }
        assert!(
            !is_holiday(date(2026, 12, 6), se),
            "finnish independence day is a normal day in sweden"
        );
    }

    #[test]
    fn finnish_holidays_2026() {
        let fi = Country::Finland;
        for (m, d) in [
            (1, 6),
            (4, 3),
            (4, 6),
            (5, 1),
            (5, 14),
            (6, 19),
            (6, 20),
            (10, 31),
            (12, 6),
            (12, 26),
        ] {
            assert!(
                is_holiday(date(2026, m, d), fi),
                "2026-{m}-{d} should be a finnish holiday"
            );
        }
        assert!(
            !is_holiday(date(2026, 6, 6), fi),
            "swedish national day is a normal day in finland"
        );
        assert!(!is_holiday(date(2026, 12, 31), fi));
    }

    #[test]
    fn business_days_skip_weekends_and_either_countrys_holidays() {
        let both = [Country::Sweden, Country::Finland];
        assert!(
            is_business_day(date(2026, 9, 25), &both),
            "an ordinary friday"
        );
        assert!(!is_business_day(date(2026, 9, 26), &both), "saturday");
        assert!(
            !is_business_day(date(2027, 12, 6), &both),
            "monday, finnish independence day"
        );
        assert!(
            is_business_day(date(2027, 12, 6), &[Country::Sweden]),
            "only a holiday in finland"
        );
        assert!(
            !is_business_day(date(2026, 12, 31), &both),
            "swedish new year's eve"
        );
    }
}
