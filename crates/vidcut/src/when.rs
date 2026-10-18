#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocalTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub weekday: u32,
}

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

impl LocalTime {
    pub fn full(&self) -> String {
        format!(
            "{}-{:02}-{:02} {:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }

    pub fn minutes(&self) -> String {
        format!(
            "{}-{:02}-{:02} {:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute
        )
    }

    fn clock(&self) -> String {
        format!("{:02}:{:02}", self.hour, self.minute)
    }

    fn date(&self) -> (i32, u32, u32) {
        (self.year, self.month, self.day)
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

pub fn local_time(ms: u64) -> Option<LocalTime> {
    let secs = libc::time_t::try_from(ms / 1000).ok()?;
    // SAFETY: an all-zero `tm` is valid, and localtime_r (localtime_s) writes only the `tm` given.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    #[cfg(unix)]
    if unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
        return None;
    }
    #[cfg(windows)]
    if unsafe { libc::localtime_s(&mut tm, &secs) } != 0 {
        return None;
    }
    Some(LocalTime {
        year: tm.tm_year + 1900,
        month: u32::try_from(tm.tm_mon + 1).ok()?,
        day: u32::try_from(tm.tm_mday).ok()?,
        hour: u32::try_from(tm.tm_hour).ok()?,
        minute: u32::try_from(tm.tm_min).ok()?,
        second: u32::try_from(tm.tm_sec).ok()?,
        weekday: u32::try_from(tm.tm_wday).ok()?,
    })
}

pub fn ago(ms: u64, now: u64) -> String {
    match (local_time(ms), local_time(now)) {
        (Some(then), Some(today)) => ago_text(&then, &today, now.saturating_sub(ms) / 1000),
        _ => String::new(),
    }
}

pub fn stamp(ms: u64, now: u64) -> String {
    match (local_time(ms), local_time(now)) {
        (Some(then), Some(today)) => stamp_text(&then, &today, now.saturating_sub(ms) / 1000),
        _ => String::new(),
    }
}

pub fn ago_text(then: &LocalTime, now: &LocalTime, secs: u64) -> String {
    match secs {
        0..45 => "just now".into(),
        45..3600 => format!("{} min ago", (secs / 60).max(1)),
        _ => stamp_text(then, now, secs),
    }
}

pub fn stamp_text(then: &LocalTime, now: &LocalTime, secs: u64) -> String {
    let days = secs / 86_400;
    if then.date() == now.date() {
        format!("today {}", then.clock())
    } else if days < 2 && (now.weekday + 6) % 7 == then.weekday {
        format!("yesterday {}", then.clock())
    } else if days < 6 {
        format!("{} {}", WEEKDAYS[then.weekday as usize % 7], then.clock())
    } else {
        then.minutes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(day: u32, hour: u32, minute: u32, weekday: u32) -> LocalTime {
        LocalTime {
            year: 2026,
            month: 10,
            day,
            hour,
            minute,
            second: 0,
            weekday,
        }
    }

    #[test]
    fn two_minutes_back_is_minutes_ago() {
        assert_eq!(
            ago_text(&at(18, 14, 0, 0), &at(18, 14, 2, 0), 120),
            "2 min ago"
        );
    }

    #[test]
    fn earlier_today_is_a_clock_time() {
        assert_eq!(
            ago_text(&at(18, 9, 5, 0), &at(18, 14, 2, 0), 17_820),
            "today 09:05"
        );
    }

    #[test]
    fn the_day_before_is_yesterday() {
        assert_eq!(
            stamp_text(&at(17, 22, 30, 6), &at(18, 8, 0, 0), 34_200),
            "yesterday 22:30"
        );
    }

    #[test]
    fn this_week_is_a_weekday() {
        assert_eq!(
            stamp_text(&at(14, 14, 2, 3), &at(18, 8, 0, 0), 323_880),
            "Wed 14:02"
        );
    }

    #[test]
    fn older_is_a_date() {
        assert_eq!(
            stamp_text(&at(1, 14, 2, 4), &at(18, 8, 0, 0), 1_447_080),
            "2026-10-01 14:02"
        );
    }
}
