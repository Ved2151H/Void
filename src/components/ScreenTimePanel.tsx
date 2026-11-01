import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { AppWindow, ChevronDown, Clock3, Hourglass, TrendingUp } from "lucide-react";
import { SectionHeader } from "./SectionHeader";
import "./SectionPanels.css";

interface ScreenTimeApp {
	name: string;
	/** Unique ledger key; unlike `path`, it separates UWP apps. */
	key: string;
	path: string;
	seconds: number;
	percent: number;
}

interface ScreenTimeDay {
	date: string;
	seconds: number;
}

interface ScreenTimeSnapshot {
	today_seconds: number;
	limit_seconds: number;
	apps_used: number;
	apps: ScreenTimeApp[];
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
}

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const WEEKDAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

function headerDate(date: Date): string {
	return `${WEEKDAYS[date.getDay()]}, ${date.getDate()} ${MONTHS[date.getMonth()]}`;
}

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

function dayInitial(value: string): string {
	return WEEKDAYS[parseDate(value).getDay()].slice(0, 1);
}

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
	onNext
}: ScreenTimePanelProps) {
	const [data, setData] = useState<ScreenTimeSnapshot | null>(null);
	const [icons, setIcons] = useState<Record<string, string | null>>({});
	const [failed, setFailed] = useState(false);
	const iconCache = useRef<Record<string, string | null>>({});
	const mounted = useRef(true);
	const firstLoad = useRef(true);

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
		} finally {
			if (mounted.current) firstLoad.current = false;
		}
	}, []);

	useEffect(() => {
		mounted.current = true;
		void load();
		// The ledger advances every five seconds. Reading it at twice that cadence
		// keeps the panel current without another high-frequency poll.
		const interval = window.setInterval(() => void load(), 10_000);
		return () => {
			mounted.current = false;
			window.clearInterval(interval);
		};
	}, [load]);

	useEffect(() => {
		const apps = data?.apps.slice(0, 8) ?? [];
		const missing = apps.filter((app) => !(app.key in iconCache.current));
		if (missing.length === 0) return;

		let cancelled = false;
		void Promise.all(
			missing.map(async (app) => {
				try {
					const icon = await invoke<string | null>("get_app_icon", { path: app.path });
					iconCache.current[app.key] = icon ?? null;
				} catch {
					iconCache.current[app.key] = null;
				}
			})
		).then(() => {
			if (!cancelled) setIcons({ ...iconCache.current });
		});

		return () => {
			cancelled = true;
		};
	}, [data]);

	const networkKind = network?.kind ?? "none";
	const networkName =
		networkKind === "wifi"
			? network?.ssid ?? "Wi-Fi"
			: networkKind === "lan"
				? network?.name ?? "Ethernet"
				: "No network";

	const todaySeconds = data?.today_seconds ?? 0;
	const limit = data?.limit_seconds ?? 8 * 60 * 60;
	const remaining = Math.max(0, limit - todaySeconds);
	const overLimit = data !== null && todaySeconds >= limit;
	const maxDay = Math.max(1, ...(data?.days.map((day) => day.seconds) ?? [0]));
	const weekTotal = data?.days.reduce((sum, day) => sum + day.seconds, 0) ?? 0;
	const lastDayDate = data?.days.length ? data.days[data.days.length - 1].date : "";
	const topApps = data?.apps.slice(0, 8) ?? [];

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
								Today
							</div>
							<div
								className={`st-big-time${duration(todaySeconds).length > 6 ? " long" : ""}`}
							>
								{data === null ? "—" : duration(todaySeconds)}
							</div>
							<div className="st-sub">
								of {duration(limit)}{" "}
								<span>({data === null ? "—" : overLimit ? "limit reached" : `${duration(remaining)} left`})</span>
							</div>
						</div>

						<div className="st-stat">
							<AppWindow size={12} strokeWidth={2} />
							<span className="st-stat-label">Apps</span>
							<strong className="st-stat-value">{data === null ? "—" : data.apps_used}</strong>
						</div>

						<div className="st-stat">
							<Clock3 size={12} strokeWidth={2} />
							<span className="st-stat-label">Daily limit</span>
							<strong className="st-stat-value">{duration(limit)}</strong>
						</div>
					</div>

					<div className="sp-card st-week">
						<div className="st-week-head">
							<div className="st-title">
								<TrendingUp size={11} strokeWidth={2.2} />
								Last 7 days
							</div>
							<span className="st-avg">{data === null ? "—" : `${duration(weekTotal / 7)} avg`}</span>
						</div>
						<div className="st-chart" aria-label="Daily screen time for the last seven days">
							{(data?.days ?? Array.from({ length: 7 }, () => ({ date: "", seconds: 0 }))).map(
								(day, index) => {
									const isToday = day.date !== "" && day.date === lastDayDate;
									const height =
										day.seconds > 0 ? Math.max(8, Math.round((day.seconds / maxDay) * 100)) : 3;
									return (
										<div className={`st-day${isToday ? " active" : ""}`} key={day.date || index}>
											<span className="st-bar-wrap">
												<span
													className="st-bar"
													style={{ height: `${height}%` }}
													title={
														day.date
															? `${headerDate(parseDate(day.date))} — ${duration(day.seconds)}`
															: undefined
													}
												/>
											</span>
											<span className="st-day-label">
												{day.date ? dayInitial(day.date) : "·"}
											</span>
										</div>
									);
								}
							)}
						</div>
					</div>
				</div>

				<div className="sp-card st-apps">
					<div className="st-apps-head">
						<span className="st-apps-title">By app</span>
						<span className="st-apps-mode">
							while Void runs
							<ChevronDown size={9} strokeWidth={2.2} />
						</span>
					</div>
					<div className="st-app-list">
						{topApps.length === 0 ? (
							<div className="st-empty">
								{failed
									? "Screen time is unavailable right now."
									: firstLoad.current
										? "Reading screen time…"
										: "No app usage recorded yet today."}
							</div>
						) : (
							topApps.map((app) => (
								<div className="st-app-row" key={app.key}>
									<div className="st-app-icon">
										{icons[app.key] ? (
											<img src={icons[app.key] as string} alt="" draggable={false} />
										) : (
											<span>{(app.name || "?").slice(0, 1).toUpperCase()}</span>
										)}
									</div>
									<span className="st-app-name" title={app.name}>
										{app.name}
									</span>
									<span className="st-app-track">
										<span style={{ width: `${Math.max(2, app.percent)}%` }} />
									</span>
									<span className="st-app-time">{duration(app.seconds)}</span>
								</div>
							))
						)}
					</div>
				</div>
			</div>
		</div>
	);
}
