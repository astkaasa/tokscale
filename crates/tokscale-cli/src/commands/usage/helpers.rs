use anyhow::Result;
use chrono::{DateTime, Datelike, Duration, Local, Utc};

pub fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => s.to_string(),
    }
}

pub fn read_keychain(service: &str) -> Result<String> {
    if cfg!(not(target_os = "macos")) {
        anyhow::bail!("Keychain lookup is only available on macOS");
    }
    let out = std::process::Command::new("security")
        .args(["find-generic-password", "-s", service, "-w"])
        .output()?;
    if !out.status.success() {
        anyhow::bail!("Keychain lookup failed for service '{service}'");
    }
    Ok(String::from_utf8(out.stdout)?.trim_end().to_string())
}

pub fn format_reset_time(resets_at: &str) -> String {
    let dt = match DateTime::parse_from_rfc3339(resets_at) {
        Ok(d) => d.with_timezone(&Utc),
        Err(_) => return resets_at.into(),
    };
    let local_dt = dt.with_timezone(&Local);
    let now = Utc::now();
    let display_time = compact_reset_time(local_dt, now.with_timezone(&Local), dt - now);
    format_reset_time_with_now(dt, now, &display_time)
}

fn format_reset_time_with_now(
    reset_at: DateTime<Utc>,
    now: DateTime<Utc>,
    display_time: &str,
) -> String {
    let diff = reset_at - now;
    if diff <= Duration::zero() {
        return "resets now".into();
    }
    let total_mins = diff.num_minutes();
    if total_mins < 60 {
        format!("resets in {total_mins}m")
    } else if total_mins < 24 * 60 {
        let h = diff.num_hours();
        let m = (diff - Duration::hours(h)).num_minutes();
        if m > 0 {
            format!("resets in {h}h {m}m")
        } else {
            format!("resets in {h}h")
        }
    } else {
        format!("resets {display_time}")
    }
}

fn compact_reset_time(reset_at: DateTime<Local>, now: DateTime<Local>, diff: Duration) -> String {
    if reset_at.year() != now.year() {
        reset_at.format("%Y-%m-%d %H:%M").to_string()
    } else if diff.num_days() < 7 {
        reset_at.format("%a %b %-d %H:%M").to_string()
    } else {
        reset_at.format("%b %-d %H:%M").to_string()
    }
}

pub fn render_ascii_bar(remaining_percent: f64, width: usize) -> String {
    let filled = (remaining_percent.clamp(0.0, 100.0) / 100.0 * width as f64).round() as usize;
    format!("[{}{}]", "=".repeat(filled), "-".repeat(width - filled))
}

pub fn atomic_write_secret(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    let dir = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "path has no parent directory",
            )
        })?;
    tokscale_core::fs_atomic::ensure_private_dir(dir)?;
    tokscale_core::fs_atomic::atomic_write_private(path, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn reset_time_keeps_short_windows_relative() {
        let label = format_reset_time_with_now(
            utc("2026-06-25T02:45:00Z"),
            utc("2026-06-25T01:30:00Z"),
            "2026-06-25 10:45 +08:00",
        );

        assert_eq!(label, "resets in 1h 15m");
    }

    #[test]
    fn reset_time_shows_absolute_local_time_for_daily_or_longer_windows() {
        let label = format_reset_time_with_now(
            utc("2026-06-27T01:30:00Z"),
            utc("2026-06-25T01:30:00Z"),
            "Sat Jun 27 09:30",
        );

        assert_eq!(label, "resets Sat Jun 27 09:30");
    }

    #[test]
    fn reset_time_omits_weekday_for_long_windows() {
        let label = format_reset_time_with_now(
            utc("2026-07-18T00:43:00Z"),
            utc("2026-06-25T01:30:00Z"),
            "Jul 18 08:43",
        );

        assert_eq!(label, "resets Jul 18 08:43");
    }

    #[test]
    fn atomic_write_secret_replaces_existing_file_without_temp_leaks() {
        let temp = tempfile::TempDir::new().unwrap();
        let directory = temp.path().join("credentials");
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("auth.json");
        std::fs::write(&path, b"old secret").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        }

        atomic_write_secret(&path, b"new secret").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"new secret");
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                std::fs::metadata(&directory).unwrap().permissions().mode() & 0o7777,
                0o700
            );
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
                0o600
            );
        }
    }
}
