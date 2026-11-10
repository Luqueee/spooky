//! A small append-only server log.
//!
//! The server runs detached with no terminal, so this file is the only place its
//! lifecycle events and failures can be read. It lives in the private state
//! directory, is capped, and keeps exactly one previous generation.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

/// Size at which the log rotates to `server.log.1`.
const LIMIT: u64 = 1024 * 1024;
const NAME: &str = "server.log";

struct Sink {
    path: PathBuf,
    file: File,
    written: u64,
    limit: u64,
}

static SINK: OnceLock<Mutex<Sink>> = OnceLock::new();

/// Records one line in the server log. Never fails the caller: a log that
/// cannot be written must not take a terminal session down with it.
macro_rules! info {
    ($($arg:tt)*) => {
        crate::log::write(&format!($($arg)*))
    };
}

/// Opens the log in `dir`. Calling it again keeps the first log.
pub fn init(dir: &Path) -> io::Result<()> {
    let sink = Sink::open(dir.join(NAME), LIMIT)?;
    let _ = SINK.set(Mutex::new(sink));
    Ok(())
}

/// Appends `message` as one timestamped line, if the log is open.
pub fn write(message: &str) {
    if let Some(sink) = SINK.get()
        && let Ok(mut sink) = sink.lock()
    {
        sink.append(&line(unix_time(), message));
    }
}

impl Sink {
    fn open(path: PathBuf, limit: u64) -> io::Result<Self> {
        let (file, written) = open_file(&path)?;
        Ok(Self {
            path,
            file,
            written,
            limit,
        })
    }

    fn append(&mut self, line: &str) {
        if self.written + line.len() as u64 > self.limit {
            let mut previous = self.path.clone().into_os_string();
            previous.push(".1");
            if fs::rename(&self.path, PathBuf::from(previous)).is_ok()
                && let Ok((file, written)) = open_file(&self.path)
            {
                self.file = file;
                self.written = written;
            }
        }
        if self.file.write_all(line.as_bytes()).is_ok() {
            self.written += line.len() as u64;
        }
    }
}

fn open_file(path: &Path) -> io::Result<(File, u64)> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let written = file.metadata()?.len();
    Ok((file, written))
}

fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// One log line. Control characters become spaces so that text that reached the
/// log from a terminal or a client cannot forge further lines.
fn line(seconds: u64, message: &str) -> String {
    let message: String = message
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    format!("{} {message}\n", utc(seconds))
}

/// `YYYY-MM-DDTHH:MM:SSZ` for a Unix time (proleptic Gregorian, UTC).
fn utc(seconds: u64) -> String {
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    let shifted = days as i64 + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use super::{Sink, line, utc};
    use std::{
        fs,
        sync::atomic::{AtomicU32, Ordering},
    };

    fn scratch() -> std::path::PathBuf {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "spooky-log-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).expect("scratch directory");
        dir
    }

    #[test]
    fn times_are_utc_calendar_dates_including_leap_days() {
        assert_eq!(utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(utc(1_000_000_000), "2001-09-09T01:46:40Z");
        assert_eq!(utc(4_107_542_399), "2100-02-28T23:59:59Z");
    }

    #[test]
    fn text_from_outside_cannot_forge_a_second_line() {
        let text = line(0, "client said\n1970-01-01T00:00:00Z forged\r");
        assert_eq!(text.matches('\n').count(), 1);
        assert!(text.ends_with("forged \n"));
    }

    #[test]
    fn a_full_log_rotates_and_keeps_one_generation() {
        let dir = scratch();
        let path = dir.join("server.log");
        let mut sink = Sink::open(path.clone(), 40).expect("log");
        sink.append("first line of the log\n");
        sink.append("second line of the log\n");
        sink.append("third line of the log\n");
        let current = fs::read_to_string(&path).expect("current");
        let previous = fs::read_to_string(dir.join("server.log.1")).expect("previous");
        assert_eq!(current, "third line of the log\n");
        assert_eq!(previous, "second line of the log\n");
        fs::remove_dir_all(dir).expect("cleanup");
    }
}
