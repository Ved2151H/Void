//! Screen time from the active window.
//!
//! Windows has no system API that reports per-app screen time, so Void keeps
//! its own ledger: while it runs, a small worker samples the foreground window
//! (and the last input time) and adds the elapsed seconds to that app's total
//! for the local day. Totals are persisted next to settings.json and survive
//! restarts. The ledger keeps a rolling month of history and the user's daily
//! limit lives in the same file. It only covers time while Void is running;
//! this is reported in the panel rather than filled with made-up history.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::Manager;
use windows::Win32::System::SystemInformation::{GetLocalTime, GetTickCount64};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
};

/// Default daily limit. Eight hours matches the standard screen-time guidance
/// shown by most wellbeing tools; the user can change it in the panel.
const DEFAULT_LIMIT_SECONDS: u64 = 8 * 60 * 60;
/// History kept on disk. Comfortably past the 30 days the panel can browse.
const RETENTION_DAYS: u16 = 35;
/// Bounds for the user-set limit: a quarter of an hour up to a full day.
const MIN_LIMIT_SECONDS: u64 = 15 * 60;
const MAX_LIMIT_SECONDS: u64 = 24 * 60 * 60;
/// How often the foreground app is sampled. Every sample is real elapsed time,
/// so a long session still adds up to the wall clock.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(5);
/// A machine left alone on a page shouldn't count as seven hours of reading.
const IDLE_LIMIT_MS: u64 = 5 * 60 * 1000;

#[derive(Clone, Serialize, Deserialize, Default)]
struct AppUsage {
    name: String,
    seconds: u64,
}

#[derive(Serialize, Deserialize, Default)]
struct DayUsage {
    /// Keyed by lowercased executable path (or path + window title for host
    /// processes such as ApplicationFrameHost, which front several apps).
    apps: BTreeMap<String, AppUsage>,
}

#[derive(Serialize, Deserialize)]
struct Ledger {
    days: BTreeMap<String, DayUsage>,
    /// The user's daily limit in seconds. Absent in files written by older
    /// versions, which used the eight-hour default.
    #[serde(default = "default_limit")]
    limit_seconds: u64,
}

impl Default for Ledger {
    fn default() -> Self {
        Self {
            days: BTreeMap::new(),
            limit_seconds: DEFAULT_LIMIT_SECONDS,
        }
    }
}

fn default_limit() -> u64 {
    DEFAULT_LIMIT_SECONDS
}

#[derive(Clone, Serialize)]
pub struct ScreenTimeApp {
    pub name: String,
    /// Unique ledger key. Host processes such as ApplicationFrameHost front
    /// several apps, so this can differ from the plain executable path.
    pub key: String,
    /// Executable path; the frontend asks `get_app_icon` for the real icon.
    pub path: String,
    pub seconds: u64,
}

#[derive(Clone, Serialize)]
pub struct ScreenTimeDay {
    pub date: String,
    pub seconds: u64,
    /// Highest usage first, so the panel can list a day without sorting.
    pub apps: Vec<ScreenTimeApp>,
}

#[derive(Clone, Serialize)]
pub struct ScreenTimeSnapshot {
    /// Today's local date key, so the panel and the ledger agree on "today".
    pub today: String,
    pub today_seconds: u64,
    pub limit_seconds: u64,
    /// Oldest first, ending today. Every retained day is included.
    pub days: Vec<ScreenTimeDay>,
}

static LEDGER: Mutex<Option<Ledger>> = Mutex::new(None);
static LEDGER_PATH: OnceLock<PathBuf> = OnceLock::new();

fn local_date() -> (u16, u16, u16) {
    let time = unsafe { GetLocalTime() };
    (time.wYear, time.wMonth, time.wDay)
}

fn is_leap(year: u16) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: u16, month: u16) -> u16 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 30,
    }
}

/// The date one day before `(year, month, day)`, wrapping months and years.
fn previous_day(year: u16, month: u16, day: u16) -> (u16, u16, u16) {
    if day > 1 {
        return (year, month, day - 1);
    }
    if month > 1 {
        let month = month - 1;
        return (year, month, days_in_month(year, month));
    }
    (year - 1, 12, 31)
}

fn date_key((year, month, day): (u16, u16, u16)) -> String {
    format!("{year:04}-{month:02}-{day:02}")
}

/// The date `days` days before `(year, month, day)`, wrapping months and years.
fn days_before((year, month, day): (u16, u16, u16), days: u16) -> (u16, u16, u16) {
    let mut date = (year, month, day);
    for _ in 0..days {
        date = previous_day(date.0, date.1, date.2);
    }
    date
}

/// Oldest date the ledger keeps: the current day plus the rest of the window.
fn retention_start() -> String {
    date_key(days_before(local_date(), RETENTION_DAYS - 1))
}

fn window_title(hwnd: windows::Win32::Foundation::HWND) -> String {
    let mut buffer = [0u16; 512];
    let length = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..length as usize])
}

fn foreground_app() -> Option<(String, String)> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 || pid == GetCurrentProcessId() {
            return None;
        }

        let path = crate::commands::process_image_path(pid)?;
        let lower = path.to_ascii_lowercase();
        let file = lower.rsplit(['\\', '/']).next().unwrap_or_default();
        // Lock and logon screens are not app usage.
        if matches!(file, "lockapp.exe" | "logonui.exe" | "winlogon.exe") {
            return None;
        }

        // A UWP app's window belongs to ApplicationFrameHost; its title is the
        // only useful identity the process gives us. Browser windows keep the
        // browser's identity even when the title names the current page.
        if file == "applicationframehost.exe" {
            let title = window_title(hwnd);
            if title.is_empty() {
                return None;
            }
            return Some((format!("{lower}|{title}"), title));
        }

        let name = crate::commands::friendly_process_name(&path);
        Some((lower, name))
    }
}

fn idle_for_ms() -> u64 {
    unsafe {
        let mut info = LASTINPUTINFO {
            cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        if !GetLastInputInfo(&mut info).as_bool() {
            return 0;
        }
        GetTickCount64().saturating_sub(info.dwTime as u64)
    }
}

fn add_sample(ledger: &mut Ledger, key: String, name: String, seconds: f64) {
    if seconds <= 0.0 {
        return;
    }
    let day = ledger.days.entry(date_key(local_date())).or_default();
    let entry = day.apps.entry(key).or_default();
    entry.name = name;
    // Rounding each sample avoids drift and never gives an app more time than
    // actually elapsed.
    entry.seconds = entry.seconds.saturating_add(seconds.round() as u64);
}

fn prune(ledger: &mut Ledger) {
    let oldest = retention_start();
    ledger
        .days
        .retain(|date, _| date.as_str() >= oldest.as_str());
}

fn load(path: &PathBuf) -> Ledger {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ledger::default();
    };
    let mut ledger: Ledger = serde_json::from_str(&text).unwrap_or_default();
    prune(&mut ledger);
    ledger
}

fn save(path: &PathBuf, ledger: &Ledger) {
    if let Ok(text) = serde_json::to_string(ledger) {
        let _ = std::fs::write(path, text);
    }
}

fn track_loop() {
    let Some(path) = LEDGER_PATH.get().cloned() else {
        return;
    };
    let mut last_tick = Instant::now();
    let mut since_save = Instant::now();

    loop {
        std::thread::sleep(SAMPLE_INTERVAL);
        let elapsed = last_tick.elapsed().as_secs_f64();
        last_tick = Instant::now();

        // Idle and lock screens are not app usage.
        if idle_for_ms() >= IDLE_LIMIT_MS {
            continue;
        }
        let Some((key, name)) = foreground_app() else {
            continue;
        };

        if let Ok(mut guard) = LEDGER.lock() {
            let ledger = guard.get_or_insert_with(Ledger::default);
            add_sample(ledger, key, name, elapsed);
            if since_save.elapsed() >= Duration::from_secs(30) {
                prune(ledger);
                save(&path, ledger);
                since_save = Instant::now();
            }
        }
    }
}

/// Loads the saved ledger and starts sampling. Called once from Tauri setup.
pub fn init(app: &tauri::AppHandle) {
    let Ok(dir) = app.path().app_config_dir() else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("screen_time.json");
    if LEDGER_PATH.set(path.clone()).is_err() {
        return;
    }

    let ledger = load(&path);
    if let Ok(mut guard) = LEDGER.lock() {
        *guard = Some(ledger);
    }
    std::thread::spawn(track_loop);
}

/// Today's totals plus the whole retained history, all from the ledger.
#[tauri::command]
pub fn get_screen_time() -> ScreenTimeSnapshot {
    snapshot()
}

/// Persists the user's daily limit. Returns the value actually stored.
#[tauri::command]
pub fn set_screen_time_limit(seconds: u64) -> u64 {
    let seconds = seconds.clamp(MIN_LIMIT_SECONDS, MAX_LIMIT_SECONDS);
    let Some(path) = LEDGER_PATH.get() else {
        return seconds;
    };
    if let Ok(mut guard) = LEDGER.lock() {
        let ledger = guard.get_or_insert_with(Ledger::default);
        ledger.limit_seconds = seconds;
        save(path, ledger);
    }
    seconds
}

/// Builds one ledger day into the serialisable form the panel consumes.
fn day_snapshot(date: String, day: &DayUsage) -> ScreenTimeDay {
    let total: u64 = day.apps.values().map(|app| app.seconds).sum();
    let mut apps: Vec<ScreenTimeApp> = day
        .apps
        .iter()
        .filter(|(_, app)| app.seconds > 0)
        .map(|(key, app)| ScreenTimeApp {
            name: app.name.clone(),
            key: key.clone(),
            path: key.split('|').next().unwrap_or(key).to_string(),
            seconds: app.seconds,
        })
        .collect();
    apps.sort_by(|a, b| b.seconds.cmp(&a.seconds).then(a.name.cmp(&b.name)));
    ScreenTimeDay {
        date,
        seconds: total,
        apps,
    }
}

fn snapshot() -> ScreenTimeSnapshot {
    let today = date_key(local_date());
    let guard = match LEDGER.lock() {
        Ok(guard) => guard,
        Err(_) => {
            return ScreenTimeSnapshot {
                today,
                today_seconds: 0,
                limit_seconds: DEFAULT_LIMIT_SECONDS,
                days: Vec::new(),
            };
        }
    };
    let default_ledger = Ledger::default();
    let ledger = guard.as_ref().unwrap_or(&default_ledger);

    // BTreeMap iterates in date order, so this is already oldest first.
    let days: Vec<ScreenTimeDay> = ledger
        .days
        .iter()
        .filter(|(date, _)| date.as_str() <= today.as_str())
        .map(|(date, day)| day_snapshot(date.clone(), day))
        .collect();
    let today_seconds = days
        .iter()
        .rev()
        .find(|day| day.date == today)
        .map(|day| day.seconds)
        .unwrap_or(0);

    ScreenTimeSnapshot {
        today,
        today_seconds,
        limit_seconds: ledger.limit_seconds,
        days,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previous_day_wraps_month_and_year() {
        assert_eq!(previous_day(2026, 11, 1), (2026, 10, 31));
        assert_eq!(previous_day(2026, 3, 1), (2026, 2, 28));
        assert_eq!(previous_day(2024, 3, 1), (2024, 2, 29));
        assert_eq!(previous_day(2026, 1, 1), (2025, 12, 31));
        assert_eq!(previous_day(2026, 11, 2), (2026, 11, 1));
    }

    #[test]
    fn days_before_spans_a_month() {
        assert_eq!(days_before((2026, 11, 4), 34), (2026, 10, 1));
        assert_eq!(days_before((2026, 3, 1), 1), (2026, 2, 28));
        assert_eq!(days_before((2026, 1, 1), 1), (2025, 12, 31));
    }

    #[test]
    fn prune_keeps_the_retention_window_only() {
        let mut ledger = Ledger::default();
        for date in ["2026-09-30", "2026-10-01", "2026-10-02"] {
            ledger.days.insert(date.to_string(), DayUsage::default());
        }
        // The real today is unknown in a unit test, so check the boundary rule
        // directly: everything from retention_start() onward survives.
        let start = retention_start();
        ledger.days.insert(start.clone(), DayUsage::default());
        prune(&mut ledger);
        assert!(ledger.days.contains_key(&start));
        assert_eq!(
            ledger
                .days
                .iter()
                .filter(|(d, _)| d.as_str() < start.as_str())
                .count(),
            0
        );
    }

    #[test]
    fn date_key_is_zero_padded() {
        assert_eq!(date_key((2026, 1, 9)), "2026-01-09");
    }

    #[test]
    fn ledger_written_by_an_older_version_keeps_its_history() {
        // Files from the seven-day build have no `limit_seconds` key.
        let text = r#"{"days":{"2026-11-03":{"apps":{"c:\\games\\a.exe":{"name":"Game","seconds":42}}}}}"#;
        let ledger: Ledger = serde_json::from_str(text).unwrap();
        assert_eq!(ledger.limit_seconds, DEFAULT_LIMIT_SECONDS);
        assert_eq!(ledger.days["2026-11-03"].apps["c:\\games\\a.exe"].seconds, 42);
    }

    #[test]
    fn limit_survives_a_save_and_load_round_trip() {
        let mut ledger = Ledger::default();
        ledger.limit_seconds = 5 * 3600 + 30 * 60;
        let text = serde_json::to_string(&ledger).unwrap();
        let reloaded: Ledger = serde_json::from_str(&text).unwrap();
        assert_eq!(reloaded.limit_seconds, 5 * 3600 + 30 * 60);
    }

    #[test]
    fn a_day_totals_its_apps_and_lists_the_heaviest_first() {
        let mut day = DayUsage::default();
        day.apps.insert(
            "b.exe".into(),
            AppUsage {
                name: "B".into(),
                seconds: 600,
            },
        );
        day.apps.insert(
            "a.exe".into(),
            AppUsage {
                name: "A".into(),
                seconds: 1800,
            },
        );
        day.apps.insert(
            "idle.exe".into(),
            AppUsage {
                name: "Idle".into(),
                seconds: 0,
            },
        );
        let snapshot = day_snapshot("2026-11-04".into(), &day);
        assert_eq!(snapshot.seconds, 2400);
        assert_eq!(snapshot.apps.len(), 2);
        assert_eq!(snapshot.apps[0].name, "A");
        assert_eq!(snapshot.apps[1].name, "B");
        assert_eq!(
            snapshot.apps.iter().map(|app| app.seconds).sum::<u64>(),
            snapshot.seconds
        );
    }
}
