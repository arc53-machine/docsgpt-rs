//! Small string helpers.

/// At most `max` characters, ending with `…` when cut.
pub fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// A file name from an outside source made safe to save or upload: path and
/// reserved characters removed, no leading dots, at most 120 characters, and
/// `fallback` when nothing is left.
pub fn safe_filename(name: &str, fallback: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| !matches!(c, '/' | '\\' | '\0' | ':' | '*' | '?' | '"' | '<' | '>' | '|') && !c.is_control())
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').trim();
    if cleaned.is_empty() {
        fallback.to_string()
    } else {
        cleaned.chars().take(120).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates() {
        assert_eq!(truncate_chars("hello", 10), "hello");
        assert_eq!(truncate_chars("hello world", 6), "hello…");
        assert_eq!(truncate_chars("ééééé", 3), "éé…");
    }

    #[test]
    fn safe_filenames() {
        assert_eq!(safe_filename("../../etc/passwd", "f"), "etcpasswd");
        assert_eq!(safe_filename("..hidden", "f"), "hidden");
        assert_eq!(safe_filename("a\nb:c.txt", "f"), "abc.txt");
        assert_eq!(safe_filename("///", "file.bin"), "file.bin");
        assert_eq!(safe_filename(&"x".repeat(200), "f").len(), 120);
    }
}
