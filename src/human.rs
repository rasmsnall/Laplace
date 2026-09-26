use std::time::Duration;

pub fn duration(duration: Duration) -> String {
    match duration.as_secs() {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86400),
    }
}

pub fn since(then: chrono::DateTime<chrono::Utc>) -> String {
    duration((chrono::Utc::now() - then).to_std().unwrap_or_default())
}
