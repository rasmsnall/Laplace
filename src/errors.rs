use std::sync::LazyLock;

use regex::Regex;

/// the end of the output is where the error usually is, so a longer one keeps its tail
pub const MAX_BYTES: usize = 8 * 1024;
const MAX_HEADLINE: usize = 200;

static URL_CREDENTIALS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b([a-z][a-z0-9+.-]*://)[^\s/@:]*:[^\s/@]*@").unwrap());
static BEARER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(bearer|basic)\s+[a-z0-9._~+/=-]{8,}").unwrap());
static KEY_VALUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)\b([a-z0-9_.-]*(?:password|passwd|pwd|secret|token|api[_-]?key|access[_-]?key|account[_-]?key|credential)[a-z0-9_.-]*|sig)(\s*[=:]\s*)("[^"]*"|'[^']*'|[^\s;&,'"]+)"#,
    )
    .unwrap()
});
static SECRET_SHAPED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:AKIA[0-9A-Z]{16}|dapi[0-9a-f]{32}|gh[pousr]_[A-Za-z0-9]{36,}|xox[abpr]-[A-Za-z0-9-]{10,}|eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,})")
        .unwrap()
});

/// what a job sent as its error, made safe to store and show: secrets masked, control
/// characters dropped, and at most MAX_BYTES from the end
pub fn clean(text: &str) -> Option<String> {
    let printable: String = text
        .replace("\r\n", "\n")
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect();
    let masked = URL_CREDENTIALS.replace_all(&printable, "$1***@");
    let masked = BEARER.replace_all(&masked, "$1 ***");
    let masked = KEY_VALUE.replace_all(&masked, "$1$2***");
    let masked = SECRET_SHAPED.replace_all(&masked, "***");
    let trimmed = masked.trim();
    (!trimmed.is_empty()).then(|| tail(trimmed, MAX_BYTES).to_string())
}

/// the line that names the error: the last one that is not part of a stack trace, which
/// is where python, .net, java and node all put the exception
pub fn headline(error: &str) -> Option<String> {
    let line = error
        .lines()
        .rev()
        .map(str::trim_end)
        .find(|line| !line.is_empty() && !line.starts_with(char::is_whitespace))
        .or_else(|| error.lines().map(str::trim).rfind(|line| !line.is_empty()))?;
    Some(match line.char_indices().nth(MAX_HEADLINE) {
        Some((cut, _)) => format!("{}...", &line[..cut]),
        None => line.to_string(),
    })
}

fn tail(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    // start on a whole line when there is one
    match text[start..].find('\n') {
        Some(newline) if newline + 1 < text.len() - start => &text[start + newline + 1..],
        _ => &text[start..],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_masked() {
        // built here so the secret scan does not take the made-up key for a real one
        let databricks_key = format!("dapi{}", "0a".repeat(16));
        let cleaned = clean(&format!(
            "connecting to postgres://etl:hunter2@db.internal/dw\n\
             Authorization: Bearer abcdefghijklmnop\n\
             password=hunter2; user=etl\n\
             api_key: \"xyz 123\"\n\
             token {databricks_key}"
        ))
        .unwrap();
        assert!(!cleaned.contains("hunter2"), "{cleaned}");
        assert!(!cleaned.contains("abcdefghijklmnop"), "{cleaned}");
        assert!(!cleaned.contains("xyz 123"), "{cleaned}");
        assert!(!cleaned.contains(&databricks_key), "{cleaned}");
        assert!(
            cleaned.contains("postgres://***@db.internal/dw"),
            "{cleaned}"
        );
        assert!(cleaned.contains("user=etl"), "{cleaned}");
    }

    #[test]
    fn ordinary_text_is_kept() {
        let text = "FileNotFoundError: statement_2026-09-28.xml not found in /out";
        assert_eq!(clean(text).as_deref(), Some(text));
    }

    #[test]
    fn empty_output_is_no_error() {
        assert_eq!(clean(" \r\n\t"), None);
    }

    #[test]
    fn a_long_error_keeps_its_end_from_a_line_start() {
        let text = format!("{}\nthe real error", "x".repeat(MAX_BYTES * 2));
        let cleaned = clean(&text).unwrap();
        assert!(cleaned.len() <= MAX_BYTES);
        assert!(cleaned.ends_with("the real error"));

        let many = (0..2000)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(clean(&many).unwrap().starts_with("line "));
    }

    #[test]
    fn the_headline_skips_stack_frames() {
        let python = "Traceback (most recent call last):\n  File \"job.py\", line 3, in <module>\n    load()\nValueError: bad row 17\n";
        assert_eq!(headline(python).as_deref(), Some("ValueError: bad row 17"));

        let dotnet = "Unhandled exception. System.IO.IOException: disk full\n   at Job.Main()\n   at Job.Run()";
        assert_eq!(
            headline(dotnet).as_deref(),
            Some("Unhandled exception. System.IO.IOException: disk full")
        );

        assert_eq!(
            headline("   only indented").as_deref(),
            Some("only indented")
        );
    }
}
