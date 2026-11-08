import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
	AlertTriangle,
	AppWindow,
	ChevronDown,
	ChevronLeft,
	ChevronRight,
	Clock3,
	Hourglass,
	Minus,
	Plus,
	TrendingUp,
	X
} from "lucide-react";
import { SectionHeader } from "./SectionHeader";
import "./SectionPanels.css";

interface ScreenTimeApp {
	name: string;
	/** Unique ledger key; unlike `path`, it separates UWP apps. */
	key: string;
	path: string;
	seconds: number;
}

interface ScreenTimeDay {
	date: string;
	seconds: number;
	apps: ScreenTimeApp[];
}

interface ScreenTimeSnapshot {
	today: string;
	today_seconds: number;
	limit_seconds: number;
	/** Oldest first, ending today. */
	days: ScreenTimeDay[];
}

interface NetworkState {
	kind: "wifi" | "lan" | "none";
	ssid: string | null;
	name: string | null;
}

interface ScreenTimePanelProps {
	batteryLevel: number;
	isCharging: boolean;
	network: NetworkState | null;
	onPrev: () => void;
	onNext: () => void;
	onOpenSettings: () => void;
	onClose: () => void;
	/** Extra height the notch needs while the app list is expanded. */
	onHeightChange: (extraPx: number) => void;
}

/** Height of the collapsed dashboard. The app list adds to this. */
export const SCREEN_TIME_PANEL_HEIGHT = 206;
const DEFAULT_LIMIT_SECONDS = 8 * 60 * 60;
/** Rows in the app list and their height; past this the list scrolls. */
const MAX_VISIBLE_APP_ROWS = 6;
const APP_ROW_HEIGHT = 22;
const ALERT_STORAGE_KEY = "bloom-screen-time-limit-alert";

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const MONTHS_LONG = [
	"January",
	"February",
	"March",
	"April",
	"May",
	"June",
	"July",
	"August",
	"September",
	"October",
	"November",
	"December"
];
const WEEKDAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const WEEKDAYS_LONG = [
	"Sunday",
	"Monday",
	"Tuesday",
	"Wednesday",
	"Thursday",
	"Friday",
	"Saturday"
];

/** A compact screen-time duration: "5h 18m", "42m", "0m". */
function duration(seconds: number): string {
	const totalMinutes = Math.max(0, Math.round(seconds / 60));
	const hours = Math.floor(totalMinutes / 60);
	const minutes = totalMinutes % 60;
	if (hours === 0) return `${minutes}m`;
	return minutes === 0 ? `${hours}h` : `${hours}h ${minutes}m`;
}

function parseDate(value: string): Date {
	const [year, month, day] = value.split("-").map(Number);
	return new Date(year, month - 1, day);
}

function dateKey(date: Date): string {
	const month = `${date.getMonth() + 1}`.padStart(2, "0");
	const day = `${date.getDate()}`.padStart(2, "0");
	return `${date.getFullYear()}-${month}-${day}`;
}

function addDays(date: Date, days: number): Date {
	const next = new Date(date.getFullYear(), date.getMonth(), date.getDate());
	next.setDate(next.getDate() + days);
	return next;
}

/** Monday of the week containing `date`. Weeks in the dashboard run Mon–Sun. */
function mondayOf(date: Date): Date {
	const day = new Date(date.getFullYear(), date.getMonth(), date.getDate());
	return addDays(day, -((day.getDay() + 6) % 7));
}

function shortDate(value: string): string {
	const date = parseDate(value);
	return `${WEEKDAYS[date.getDay()]}, ${date.getDate()} ${MONTHS[date.getMonth()]}`;
}

function fullDate(value: string): string {
	const date = parseDate(value);
	return `${WEEKDAYS_LONG[date.getDay()]}, ${MONTHS_LONG[date.getMonth()]} ${date.getDate()}, ${date.getFullYear()}`;
}

/** "Monday, November 2": the card heading for a selected past day. */
function headingDate(value: string): string {
	const date = parseDate(value);
	return `${WEEKDAYS_LONG[date.getDay()]}, ${MONTHS_LONG[date.getMonth()]} ${date.getDate()}`;
}

function rangeLabel(start: Date, end: Date): string {
	const sameMonth =
		start.getMonth() === end.getMonth() && start.getFullYear() === end.getFullYear();
	const left = `${MONTHS[start.getMonth()]} ${start.getDate()}`;
	const right = sameMonth ? `${end.getDate()}` : `${MONTHS[end.getMonth()]} ${end.getDate()}`;
	return `${left} – ${right}`;
}

/** Icons are immutable per executable, so one module-level cache is enough. */
const iconCache = new Map<string, string | null>();

/**
 * Screen Time reads Void's own real foreground-app ledger (see
 * src-tauri/src/screen_time.rs). It has no invented history: days without
 * tracking stay empty, and the card says the ledger only runs while Void does.
 */
export function ScreenTimePanel({
	batteryLevel,
	isCharging,
	network,
	onPrev,
	onNext,
	onHeightChange
}: ScreenTimePanelProps) {
	const [data, setData] = useState<ScreenTimeSnapshot | null>(null);
	const [failed, setFailed] = useState(false);
	const [menuOpen, setMenuOpen] = useState(false);
	const [hovered, setHovered] = useState<number | null>(null);
	/** `null` means the viewed week's default day (today in the current week). */
	const [selected, setSelected] = useState<string | null>(null);
	const [weekOffset, setWeekOffset] = useState(0);
	const [limitOverride, setLimitOverride] = useState<number | null>(null);
	const [limitOpen, setLimitOpen] = useState(false);
	const [draftHour, setDraftHour] = useState("8");
	const [draftMinute, setDraftMinute] = useState("0");
	const [alertVisible, setAlertVisible] = useState(false);
	const [icons, setIcons] = useState<Record<string, string | null>>({});
	const menuRef = useRef<HTMLDivElement>(null);
	const mounted = useRef(true);

	const load = useCallback(async () => {
		try {
			const snapshot = await invoke<ScreenTimeSnapshot>("get_screen_time");
			if (!mounted.current) return;
			setData(snapshot);
			setFailed(false);
		} catch (error) {
			if (!mounted.current) return;
			console.error("Failed to read screen time:", error);
			setFailed(true);
		}
	}, []);

	useEffect(() => {
		mounted.current = true;
		void load();
		// The ledger advances every five seconds. Reading it at a slower cadence
		// keeps the panel current without another high-frequency poll.
		const interval = window.setInterval(() => void load(), 15_000);
		return () => {
			mounted.current = false;
			window.clearInterval(interval);
		};
	}, [load]);

	const todayKey = data?.today ?? dateKey(new Date());
	const limit = limitOverride ?? data?.limit_seconds ?? DEFAULT_LIMIT_SECONDS;
	const byDate = new Map((data?.days ?? []).map((day) => [day.date, day]));
	const todayDay = byDate.get(todayKey);
	const todaySeconds = data?.today_seconds ?? todayDay?.seconds ?? 0;

	// Week view. `weekOffset` is 0 for the current week and negative before it.
	const currentWeekStart = mondayOf(parseDate(todayKey));
	const earliest = data?.days[0]?.date ?? todayKey;
	const minWeekOffset = -Math.round(
		(currentWeekStart.getTime() - mondayOf(parseDate(earliest)).getTime()) / 604_800_000
	);
	const weekStart = addDays(currentWeekStart, weekOffset * 7);
	const weekDates = Array.from({ length: 7 }, (_, index) => addDays(weekStart, index));
	const weekKeys = weekDates.map(dateKey);
	const weekTotal = weekKeys.reduce((sum, key) => sum + (byDate.get(key)?.seconds ?? 0), 0);
	const maxSeconds = Math.max(1, ...weekKeys.map((key) => byDate.get(key)?.seconds ?? 0));
	const hasPrevWeek = weekOffset > minWeekOffset;
	const hasNextWeek = weekOffset < 0;

	const defaultDay = weekKeys.includes(todayKey)
		? todayKey
		: ([...weekKeys].reverse().find((key) => (byDate.get(key)?.seconds ?? 0) > 0) ?? weekKeys[6]);
	const appsDate = selected ?? defaultDay;
	const appsDay = byDate.get(appsDate);
	const apps = appsDay?.apps ?? [];
	const appsLabel = appsDate === todayKey ? "Today" : shortDate(appsDate);
	// One record drives the card, the graph highlight and the app list.
	const selectedSeconds = appsDay?.seconds ?? 0;
	const selectedRemaining = Math.max(0, limit - selectedSeconds);
	const overLimit = data !== null && selectedSeconds >= limit;
	const warnLevel = overLimit
		? 100
		: selectedSeconds >= limit * 0.9
			? 90
			: selectedSeconds >= limit * 0.8
				? 80
				: 0;

	const shiftWeek = (delta: number) => {
		const next = Math.max(minWeekOffset, Math.min(0, weekOffset + delta));
		if (next === weekOffset) return;
		setWeekOffset(next);
		setSelected(null);
		setHovered(null);
	};

	const toggleDay = (key: string) => {
		setSelected((current) => (current === key ? null : key));
		setMenuOpen(true);
	};

	// A new day moves the current week on its own; drop the stale selection so
	// the week view and the Used Apps list stay on the same date.
	useEffect(() => {
		setSelected(null);
		setWeekOffset(0);
		setHovered(null);
	}, [todayKey]);

	// Close the list on an outside click or Escape, like any other menu.
	useEffect(() => {
		if (!menuOpen) return;
		const onPointerDown = (event: PointerEvent) => {
			if (menuRef.current && !menuRef.current.contains(event.target as Node)) setMenuOpen(false);
		};
		const onKeyDown = (event: KeyboardEvent) => {
			if (event.key === "Escape") setMenuOpen(false);
		};
		window.addEventListener("pointerdown", onPointerDown, true);
		window.addEventListener("keydown", onKeyDown);
		return () => {
			window.removeEventListener("pointerdown", onPointerDown, true);
			window.removeEventListener("keydown", onKeyDown);
		};
	}, [menuOpen]);

	// Escape closes the limit editor as well as the app list.
	useEffect(() => {
		if (!limitOpen) return;
		const onKeyDown = (event: KeyboardEvent) => {
			if (event.key === "Escape") setLimitOpen(false);
		};
		window.addEventListener("keydown", onKeyDown);
		return () => window.removeEventListener("keydown", onKeyDown);
	}, [limitOpen]);

	// The notch grows downwards with the list instead of overlaying the week.
	const menuRows = menuOpen ? Math.max(2, Math.min(apps.length, MAX_VISIBLE_APP_ROWS)) : 0;
	const extraHeight = menuRows === 0 ? 0 : menuRows * APP_ROW_HEIGHT + 12;
	useEffect(() => {
		onHeightChange(extraHeight);
	}, [extraHeight, onHeightChange]);

	// Real per-app icons, resolved lazily only for the visible list.
	const appPaths = apps
		.slice(0, MAX_VISIBLE_APP_ROWS)
		.map((app) => app.path)
		.join("|");
	useEffect(() => {
		if (!menuOpen) return;
		const paths = appPaths.split("|").filter(Boolean);
		if (paths.every((path) => iconCache.has(path))) {
			setIcons(Object.fromEntries(iconCache));
			return;
		}
		let alive = true;
		Promise.all(
			paths.map(async (path) => {
				if (iconCache.has(path)) return;
				try {
					iconCache.set(
						path,
						await invoke<string | null>("get_app_icon", {
							path,
							name: null,
							hwnd: null
						})
					);
				} catch {
					iconCache.set(path, null);
				}
			})
		).then(() => {
			if (alive) setIcons(Object.fromEntries(iconCache));
		});
		return () => {
			alive = false;
		};
	}, [menuOpen, appPaths]);

	// Once per day, when real usage first crosses the configured limit.
	useEffect(() => {
		if (!data || todaySeconds < limit) return;
		try {
			const stored = JSON.parse(localStorage.getItem(ALERT_STORAGE_KEY) ?? "null") as {
				date?: string;
			} | null;
			if (stored?.date === data.today) return;
			localStorage.setItem(ALERT_STORAGE_KEY, JSON.stringify({ date: data.today }));
			setAlertVisible(true);
		} catch {
			setAlertVisible(true);
		}
	}, [data, todaySeconds, limit]);

	useEffect(() => {
		if (!alertVisible) return;
		const timer = window.setTimeout(() => setAlertVisible(false), 9000);
		return () => window.clearTimeout(timer);
	}, [alertVisible]);

	/** Keep a typed draft inside its dropdown-era range: digits only, two
	 *  characters, no more than the old select offered. */
	const clampDraft = (raw: string, max: number) => {
		const digits = raw.replace(/\D+/g, "").slice(0, 2);
		if (!digits) return "";
		return String(Math.min(max, parseInt(digits, 10)));
	};

	const openLimitEditor = () => {
		const minutes = Math.round(limit / 60);
		setDraftHour(String(Math.min(12, Math.floor(minutes / 60))));
		setDraftMinute(String(minutes % 60));
		setLimitOpen(true);
	};

	const adjustLimit = async (deltaSeconds: number) => {
		const next = Math.max(15 * 60, Math.min(24 * 60 * 60, limit + deltaSeconds));
		if (next === limit) return;
		try {
			const saved = await invoke<number>("set_screen_time_limit", { seconds: next });
			setLimitOverride(saved);
		} catch (error) {
			console.error("Failed to adjust the daily limit:", error);
		}
	};

	const saveLimit = async () => {
		const hour = parseInt(draftHour, 10) || 0;
		const minute = parseInt(draftMinute, 10) || 0;
		const seconds = Math.max(15 * 60, Math.min(24 * 60 * 60, hour * 3600 + minute * 60));
		try {
			const saved = await invoke<number>("set_screen_time_limit", { seconds });
			setLimitOverride(saved);
		} catch (error) {
			console.error("Failed to save the daily limit:", error);
		}
		setLimitOpen(false);
	};

	const networkKind = network?.kind ?? "none";
	const networkName =
		networkKind === "wifi"
			? (network?.ssid ?? "Wi-Fi")
			: networkKind === "lan"
				? (network?.name ?? "Ethernet")
				: "No network";

	return (
		<div className="sp-root" onClick={(event) => event.stopPropagation()}>
			<SectionHeader
				networkName={networkName}
				networkKind={networkKind}
				batteryLevel={batteryLevel}
				isCharging={isCharging}
				onPrev={onPrev}
				onNext={onNext}
			/>

			<div className="sp-body st-body">
				<div className="st-top">
					<div className="sp-card st-today">
						<div className="st-today-left">
							<div className="st-title">
								<Hourglass size={11} strokeWidth={2.2} />
								{appsDate === todayKey ? "Today" : headingDate(appsDate)}
							</div>
							<div
								className={`st-big-time${duration(selectedSeconds).length > 6 ? " long" : ""}`}
							>
								{data === null ? "—" : duration(selectedSeconds)}
							</div>
							<div className="st-sub">
								of {duration(limit)}{" "}
								<span className={warnLevel > 0 ? "warn" : undefined}>
									(
									{data === null
										? "—"
										: overLimit
											? `exceeded by ${duration(selectedSeconds - limit)}`
											: `${duration(selectedRemaining)} left`}
									)
								</span>
								{warnLevel > 0 && warnLevel < 100 && (
									<AlertTriangle size={8} strokeWidth={2.4} className="st-warn-icon" />
								)}
							</div>
						</div>

						<div className="st-stat">
							<AppWindow size={12} strokeWidth={2} />
							<span className="st-stat-label">Apps</span>
							<strong className="st-stat-value">
								{data === null ? "—" : apps.length}
							</strong>
						</div>

						<div className="st-stat st-stat-limit">
							<button
								type="button"
								className="st-stat st-stat-button st-stat-limit-main"
								onClick={openLimitEditor}
								title="Change the daily limit"
								aria-label={`Daily limit ${duration(limit)}. Change it`}
							>
								<Clock3 size={12} strokeWidth={2} />
								<span className="st-stat-label">Daily limit</span>
								<strong className="st-stat-value">{duration(limit)}</strong>
							</button>
							<div className="st-limit-steps">
								<button
									type="button"
									className="st-limit-step"
									onClick={() => void adjustLimit(-30 * 60)}
									aria-label="Decrease daily limit"
									title="Decrease by 30 minutes"
								>
									<Minus size={10} strokeWidth={2.4} />
								</button>
								<button
									type="button"
									className="st-limit-step"
									onClick={() => void adjustLimit(30 * 60)}
									aria-label="Increase daily limit"
									title="Increase by 30 minutes"
								>
									<Plus size={10} strokeWidth={2.4} />
								</button>
							</div>
						</div>
					</div>

					<div className="sp-card st-week">
						<div className="st-week-head">
							<div className="st-title">
								<TrendingUp size={11} strokeWidth={2.2} />
								Last 7 days
							</div>
							<span className="st-avg">
								{data === null ? "—" : `${duration(weekTotal / 7)} avg`}
							</span>
						</div>

						<div className="st-week-nav">
							<button
								type="button"
								className="st-week-arrow"
								onClick={() => shiftWeek(-1)}
								disabled={!hasPrevWeek}
								aria-label="Previous week"
							>
								<ChevronLeft size={10} strokeWidth={2.6} />
							</button>
							<span className="st-week-range">{rangeLabel(weekStart, weekDates[6])}</span>
							<button
								type="button"
								className="st-week-arrow"
								onClick={() => shiftWeek(1)}
								disabled={!hasNextWeek}
								aria-label="Next week"
							>
								<ChevronRight size={10} strokeWidth={2.6} />
							</button>
						</div>

						<div
							className="st-chart"
							key={dateKey(weekStart)}
							aria-label={`Daily screen time, ${rangeLabel(weekStart, weekDates[6])}`}
						>
							{weekDates.map((date, index) => {
								const key = weekKeys[index];
								const day = byDate.get(key);
								const seconds = day?.seconds ?? 0;
								const isToday = key === todayKey;
								const isSelected = key === appsDate;
								const isFuture = key > todayKey;
								// Capped at 88%, not 100%: the hover duration sits above the bar tip.
								const height =
									seconds > 0 ? Math.min(88, Math.max(8, Math.round((seconds / maxSeconds) * 100))) : 3;
								return (
									<button
										type="button"
										className={`st-day${isToday ? " today" : ""}${isSelected ? " selected" : ""}${seconds > 0 ? "" : " empty"}${isFuture ? " future" : ""}`}
										key={key}
										onPointerEnter={() => setHovered(index)}
										onPointerLeave={() =>
											setHovered((current) => (current === index ? null : current))
										}
										onFocus={() => setHovered(index)}
										onBlur={() => setHovered(null)}
										onClick={() => toggleDay(key)}
										aria-label={`${fullDate(key)}: ${duration(seconds)}`}
										aria-pressed={isSelected}
									>
										<span className="st-bar-wrap">
											<span
												className="st-bar"
												style={{ height: seconds > 0 ? `${height}%` : undefined }}
											>
												{hovered === index && seconds > 0 && (
													<span className="st-bar-time">{duration(seconds)}</span>
												)}
											</span>
										</span>
										<span className="st-day-label">{WEEKDAYS[date.getDay()]}</span>
									</button>
								);
							})}
						</div>
					</div>
				</div>

				<div className="sp-card st-apps" ref={menuRef}>
					<button
						type="button"
						className="st-apps-head"
						onClick={() => setMenuOpen((open) => !open)}
						aria-expanded={menuOpen}
						aria-label={`Used apps on ${appsLabel}`}
					>
						<span className="st-apps-title">
							Used Apps <span className="st-apps-date">· {appsLabel}</span>
						</span>
						<span className={`st-app-trigger${menuOpen ? " open" : ""}`}>
							{apps.length === 0 ? "No apps" : `${apps.length} apps`}
							<ChevronDown size={9} strokeWidth={2.2} className="st-app-caret" />
						</span>
					</button>

					<div
						className={`st-app-menu${menuOpen ? " open" : ""}`}
						style={{ maxHeight: menuOpen ? `${menuRows * APP_ROW_HEIGHT}px` : 0 }}
						role="listbox"
						aria-label={`Apps used on ${appsLabel}`}
					>
						{apps.length === 0 ? (
							<div className="st-app-menu-empty">
								{failed
									? "Screen time is unavailable right now."
									: data === null
										? "Reading screen time…"
										: "No app usage recorded for this day."}
							</div>
						) : (
							apps.map((app) => (
								<div className="st-app-menu-row" key={app.key} role="option" aria-selected={false}>
									<span className="st-app-icon">
										{icons[app.path] ? (
											<img src={icons[app.path] as string} alt="" draggable={false} />
										) : (
											<AppWindow size={10} strokeWidth={2} />
										)}
									</span>
									<span className="st-app-menu-name" title={app.name}>
										{app.name}
									</span>
									<span className="st-app-menu-time">{duration(app.seconds)}</span>
								</div>
							))
						)}
					</div>
				</div>
			</div>

			{limitOpen && (
				<div
					className="st-modal"
					role="presentation"
					onPointerDown={(event) => {
						if (event.target === event.currentTarget) setLimitOpen(false);
					}}
				>
					<div className="st-modal-card" role="dialog" aria-modal="true" aria-label="Daily limit">
						<div className="st-modal-title">Daily limit</div>
						<div className="st-modal-controls">
							<input
								className="st-select"
								type="text"
								inputMode="numeric"
								value={draftHour}
								onChange={(event) => setDraftHour(clampDraft(event.target.value, 12))}
								onBlur={(event) => {
									if (!event.target.value) setDraftHour("0");
								}}
								aria-label="Hours"
								placeholder="h"
							/>
							<input
								className="st-select"
								type="text"
								inputMode="numeric"
								value={draftMinute}
								onChange={(event) => setDraftMinute(clampDraft(event.target.value, 59))}
								onBlur={(event) => {
									if (!event.target.value) setDraftMinute("0");
								}}
								aria-label="Minutes"
								placeholder="m"
							/>
						</div>
						<div className="st-modal-actions">
							<button type="button" className="st-modal-btn" onClick={() => setLimitOpen(false)}>
								Cancel
							</button>
							<button
								type="button"
								className="st-modal-btn primary"
								onClick={() => void saveLimit()}
							>
								Save
							</button>
						</div>
					</div>
				</div>
			)}

			{alertVisible && (
				<div className="st-alert" role="alert">
					<AlertTriangle size={14} strokeWidth={2.2} className="st-alert-icon" />
					<div className="st-alert-text">
						<strong>Daily Screen Time Limit Reached</strong>
						<span>You have used {duration(todaySeconds)} today.</span>
					</div>
					<button
						type="button"
						className="st-alert-close"
						onClick={() => setAlertVisible(false)}
						aria-label="Dismiss"
					>
						<X size={10} strokeWidth={2.4} />
					</button>
				</div>
			)}
		</div>
	);
}
