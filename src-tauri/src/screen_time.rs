//! Screen time from the active window.
//!
//! Windows has no system API that reports per-app screen time, so Void keeps
//! its own ledger: while it runs, a small worker samples the foreground window
//! (and the last input time) and adds the elapsed seconds to that app's total
//! for the local day. Totals are persisted next to settings.json and survive
//! restarts. The ledger only covers time while Void is running; this is
//! reported in the panel rather than filled with made-up history.

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

/// The panel's daily limit. Eight hours matches the standard screen-time
/// guidance shown by most wellbeing tools.
const DAILY_LIMIT_SECONDS: u64 = 8 * 60 * 60;
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

#[derive(Serialize, Deserialize, Default)]
struct Ledger {
    days: BTreeMap<String, DayUsage>,
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
    pub percent: u32,
}

#[derive(Clone, Serialize)]
pub struct ScreenTimeDay {
    pub date: String,
    pub seconds: u64,
}

#[derive(Clone, Serialize)]
pub struct ScreenTimeSnapshot {
    pub today_seconds: u64,
    pub limit_seconds: u64,
    pub apps_used: u32,
    pub apps: Vec<ScreenTimeApp>,
    /// Oldest first, ending today, always seven entries.
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

fn week_ending(year: u16, month: u16, day: u16) -> Vec<String> {
    let mut keys = Vec::with_capacity(7);
    let mut date = (year, month, day);
    for _ in 0..7 {
        keys.push(date_key(date));
        date = previous_day(date.0, date.1, date.2);
    }
    keys.reverse();
    keys
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
    let today = local_date();
    let keep = week_ending(today.0, today.1, today.2);
    let oldest = keep.first().cloned().unwrap_or_default();
    ledger.days.retain(|date, _| date.as_str() >= oldest.as_str());
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

/// Today's totals, the top apps and the last seven days, all from the ledger.
#[tauri::command]
pub fn get_screen_time() -> ScreenTimeSnapshot {
    snapshot()
}

fn snapshot() -> ScreenTimeSnapshot {
    let today = local_date();
    let today_key = date_key(today);
    let weeks = week_ending(today.0, today.1, today.2);

    let (today_seconds, apps_used, apps, days) = {
        let Ok(guard) = LEDGER.lock() else {
            return ScreenTimeSnapshot {
                today_seconds: 0,
                limit_seconds: DAILY_LIMIT_SECONDS,
                apps_used: 0,
                apps: Vec::new(),
                days: weeks
                    .iter()
                    .map(|date| ScreenTimeDay {
                        date: date.clone(),
                        seconds: 0,
                    })
                    .collect(),
            };
        };
        let ledger = guard.as_ref();
        let empty = DayUsage::default();
        let day = ledger
            .and_then(|ledger| ledger.days.get(&today_key))
            .unwrap_or(&empty);
        let today_seconds: u64 = day.apps.values().map(|app| app.seconds).sum();
        let apps_used = day.apps.values().filter(|app| app.seconds > 0).count() as u32;

        let mut apps: Vec<ScreenTimeApp> = day
            .apps
            .iter()
            .filter(|(_, app)| app.seconds > 0)
            .map(|(path, app)| ScreenTimeApp {
                name: app.name.clone(),
                key: path.clone(),
                path: path.split('|').next().unwrap_or(path).to_string(),
                seconds: app.seconds,
                percent: if today_seconds > 0 {
                    ((app.seconds as f64 / today_seconds as f64) * 100.0).round() as u32
                } else {
                    0
                },
            })
            .collect();
        apps.sort_by(|a, b| b.seconds.cmp(&a.seconds).then(a.name.cmp(&b.name)));

        let days = weeks
            .iter()
            .map(|date| ScreenTimeDay {
                date: date.clone(),
                seconds: ledger
                    .and_then(|ledger| ledger.days.get(date))
                    .map(|day| day.apps.values().map(|app| app.seconds).sum())
                    .unwrap_or(0),
            })
            .collect();

        (today_seconds, apps_used, apps, days)
    };

    ScreenTimeSnapshot {
        today_seconds,
        limit_seconds: DAILY_LIMIT_SECONDS,
        apps_used,
        apps,
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
    fn seven_day_window_ends_today() {
        let days = week_ending(2026, 11, 1);
        assert_eq!(days.len(), 7);
        assert_eq!(days.first().unwrap(), "2026-10-26");
        assert_eq!(days.last().unwrap(), "2026-11-01");
    }

    #[test]
    fn date_key_is_zero_padded() {
        assert_eq!(date_key((2026, 1, 9)), "2026-01-09");
    }
}
