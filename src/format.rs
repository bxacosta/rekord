//! Human-friendly formatting shared by the front ends.

use std::path::Path;
use std::time::Duration;

/// `00:04:07`
pub fn duration(d: Duration) -> String {
    let s = d.as_secs();
    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

/// `812 KB`, `14.2 MB`, `1.3 GB`
pub fn bytes(n: u64) -> String {
    const KB: f64 = 1024.0;
    let n = n as f64;
    if n < KB * KB {
        format!("{:.0} KB", n / KB)
    } else if n < KB * KB * KB {
        format!("{:.1} MB", n / (KB * KB))
    } else {
        format!("{:.2} GB", n / (KB * KB * KB))
    }
}

/// A path as the user would type it (without Windows' `\\?\` verbatim prefix).
pub fn path(p: &Path) -> String {
    let text = p.display().to_string();
    text.strip_prefix(r"\\?\").map(str::to_owned).unwrap_or(text)
}

/// `+12 dB`, `-4 dB`, `0 dB`
pub fn gain(db: f32) -> String {
    if db > 0.0 { format!("+{db:.0} dB") } else { format!("{db:.0} dB") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_values() {
        assert_eq!(duration(Duration::from_secs(3725)), "01:02:05");
        assert_eq!(bytes(2048), "2 KB");
        assert_eq!(bytes(15 * 1024 * 1024), "15.0 MB");
        assert_eq!(gain(12.0), "+12 dB");
        assert_eq!(gain(-4.0), "-4 dB");
        assert_eq!(path(Path::new(r"\\?\C:\Music")), r"C:\Music");
    }
}
