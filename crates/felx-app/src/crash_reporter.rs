//! Crash reporter (F-112).
//!
//! On panic, capture a structured report (panic message + payload + a
//! truncated tail of the most recent log lines) to
//! `~/.felx/diagnostics/crash-<timestamp>.txt`. The report is intended
//! for the user to attach to a bug report; nothing is uploaded.

use std::io::Write;
use std::path::PathBuf;

/// Install the panic hook. Idempotent — calling more than once replaces
/// the hook each time, so the *last* call wins.
pub fn install() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        prev(info);
        let _ = write_crash_report(info);
    }));
}

fn write_crash_report(info: &std::panic::PanicHookInfo<'_>) -> std::io::Result<()> {
    let dir = diagnostics_dir();
    std::fs::create_dir_all(&dir)?;
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("crash-{secs}.txt"));
    let mut f = std::fs::File::create(&path)?;

    writeln!(f, "felx-app crash report")?;
    writeln!(f, "=====================")?;
    writeln!(f, "timestamp: {secs}")?;
    writeln!(f)?;

    let msg = info
        .payload()
        .downcast_ref::<&'static str>()
        .map(|s| (*s).to_string())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "<no message>".to_string());
    writeln!(f, "message:")?;
    writeln!(f, "  {msg}")?;
    writeln!(f)?;

    if let Some(loc) = info.location() {
        writeln!(
            f,
            "location: {}:{}:{}",
            loc.file(),
            loc.line(),
            loc.column()
        )?;
        writeln!(f)?;
    }

    writeln!(f, "(attach this file when filing a bug report)")?;

    eprintln!("[felx-app] crash report written to {}", path.display());
    Ok(())
}

fn diagnostics_dir() -> PathBuf {
    home_dir().join(".felx").join("diagnostics")
}

fn home_dir() -> PathBuf {
    if let Ok(h) = std::env::var("HOME") {
        return PathBuf::from(h);
    }
    if let Ok(p) = std::env::var("USERPROFILE") {
        return PathBuf::from(p);
    }
    std::env::temp_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_dir_is_under_home_or_temp() {
        let p = diagnostics_dir();
        assert!(p.ends_with("diagnostics"));
        assert!(p.parent().unwrap().ends_with(".felx"));
    }
}
