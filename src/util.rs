use std::time::SystemTime;

const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

// SystemTime as an HTTP date (RFC 7231).
pub fn format_http_date(time: SystemTime) -> String {
    let duration = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs() as i64;

    let (year, month, day, hour, min, sec, wday) = unix_to_datetime(secs);

    format!(
        "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
        DAYS[wday as usize], day, MONTHS[month as usize], year, hour, min, sec
    )
}

// RFC 7231 / RFC 850 / asctime into SystemTime.
pub fn parse_http_date(s: &str) -> Option<SystemTime> {
    let s = s.trim();
    let parts: Vec<&str> = s.split_whitespace().collect();

    if parts.len() >= 6 {
        let day: u32 = parts[1].parse().ok()?;
        let month = month_from_str(parts[2])?;
        let year: i64 = parts[3].parse().ok()?;
        let time_parts: Vec<&str> = parts[4].split(':').collect();
        if time_parts.len() != 3 {
            return None;
        }
        let hour: u32 = time_parts[0].parse().ok()?;
        let min: u32 = time_parts[1].parse().ok()?;
        let sec: u32 = time_parts[2].parse().ok()?;

        let unix = datetime_to_unix(year, month, day, hour, min, sec);
        return Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(unix as u64));
    }

    None
}

fn month_from_str(s: &str) -> Option<u32> {
    match s {
        "Jan" => Some(0),
        "Feb" => Some(1),
        "Mar" => Some(2),
        "Apr" => Some(3),
        "May" => Some(4),
        "Jun" => Some(5),
        "Jul" => Some(6),
        "Aug" => Some(7),
        "Sep" => Some(8),
        "Oct" => Some(9),
        "Nov" => Some(10),
        "Dec" => Some(11),
        _ => None,
    }
}

fn is_leap_year(y: i64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        0 => 31,  
        1 => if is_leap_year(y) { 29 } else { 28 },
        2 => 31,  
        3 => 30, 
        4 => 31, 
        5 => 30,  
        6 => 31, 
        7 => 31,  
        8 => 30, 
        9 => 31,  
        10 => 30, 
        11 => 31, 
        _ => 30,
    }
}

fn unix_to_datetime(secs: i64) -> (i64, u32, u32, u32, u32, u32, u32) {
    let sec = ((secs % 60) + 60) % 60;
    let min = (((secs / 60) % 60) + 60) % 60;
    let hour = (((secs / 3600) % 24) + 24) % 24;
    let mut days = secs / 86400;
    let wday = ((days % 7 + 4) % 7 + 7) % 7;

    let mut year: i64 = 1970;
    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }

    let mut month: u32 = 0;
    while month < 12 {
        let dim = days_in_month(year, month) as i64;
        if days < dim {
            break;
        }
        days -= dim;
        month += 1;
    }

    let day = days + 1;

    (year, month, day as u32, hour as u32, min as u32, sec as u32, wday as u32)
}

// convert datetime to unix timestamp
fn datetime_to_unix(year: i64, month: u32, day: u32, hour: u32, min: u32, sec: u32) -> i64 {
    let mut days: i64 = 0;
    for y in 1970..year {
        days += if is_leap_year(y) { 366 } else { 365 };
    }
    for m in 0..month {
        days += days_in_month(year, m) as i64;
    }
    days += (day as i64) - 1;
    days * 86400 + (hour as i64) * 3600 + (min as i64) * 60 + (sec as i64)
}