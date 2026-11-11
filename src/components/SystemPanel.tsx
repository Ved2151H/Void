import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
	ArrowDown,
	ArrowUp,
	BatteryCharging,
	ChevronLeft,
	ChevronRight,
	Clock3,
	Cpu,
	Triangle,
	Wifi,
	WifiOff,
	EthernetPort
} from "lucide-react";
import { formatDuration, type BatteryDetails } from "../batteryTooltip";
import { SectionHeader } from "./SectionHeader";
import "./SectionPanels.css";

interface DiskUsage {
	free_gb: number;
	total_gb: number;
	used_percent: number;
}

interface NetworkState {
	kind: "wifi" | "lan" | "none";
	ssid: string | null;
	name: string | null;
}

interface ScreenTimeDay {
	date: string;
	seconds: number;
}

interface ScreenTimeSnapshot {
	today: string;
	days: ScreenTimeDay[];
}

interface SystemPanelProps {
	batteryLevel: number;
	isCharging: boolean;
	battery: BatteryDetails | null;
	cpuUsage: number;
	ramUsage: number;
	diskUsage: DiskUsage | null;
	netUpSpeed: number;
	netDownSpeed: number;
	network: NetworkState | null;

	onRefreshBattery: () => void;
	onPrev: () => void;
	onNext: () => void;
	onOpenSettings: () => void;
	onClose: () => void;
}

/**
 * Header + the four system components (Power, Network, Performance and the
 * full-width Time summary) from the reference, sized to fit them compactly
 * without leftover empty space.
 */
export const SYSTEM_PANEL_HEIGHT = 350;

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const WEEKDAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

function splitDuration(seconds: number): { hours: number; minutes: number } {
	const totalMinutes = Math.max(0, Math.round(seconds / 60));
	return { hours: Math.floor(totalMinutes / 60), minutes: totalMinutes % 60 };
}

/** "7h 50m" with the unit letters smaller, matching the reference card. */
function DurationText({ seconds }: { seconds: number }) {
	const { hours, minutes } = splitDuration(seconds);
	if (hours === 0) {
		return (
			<>
				<em>{minutes}</em>
				<small>m</small>
			</>
		);
	}
	return (
		<>
			<em>{hours}</em>
			<small>h</small>
			{minutes > 0 ? (
				<>
					{" "}
					<em>{minutes}</em>
					<small>m</small>
				</>
			) : null}
		</>
	);
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

function mondayOf(date: Date): Date {
	const day = new Date(date.getFullYear(), date.getMonth(), date.getDate());
	return addDays(day, -((day.getDay() + 6) % 7));
}

function shortDay(value: string): string {
	const date = parseDate(value);
	return `${date.getDate()} ${MONTHS[date.getMonth()]}`;
}

function weekRangeLabel(start: Date, end: Date): string {
	return `${shortDay(dateKey(start))} – ${shortDay(dateKey(end))}`;
}

function weekdayCaption(value: string, todayKey: string): string {
	const date = parseDate(value);
	const base = `${WEEKDAYS[date.getDay()]}, ${date.getDate()} ${MONTHS[date.getMonth()]}`;
	return value === todayKey ? `${base} · Today` : base;
}

function splitRate(bytesPerSecond: number): { value: string; unit: string } {
	if (bytesPerSecond < 1024) return { value: `${Math.round(bytesPerSecond)}`, unit: "B/s" };
	if (bytesPerSecond < 1024 * 1024)
		return { value: (bytesPerSecond / 1024).toFixed(1), unit: "KB/s" };
	return { value: (bytesPerSecond / (1024 * 1024)).toFixed(1), unit: "MB/s" };
}

/**
 * System shows the same live signals the notch already samples (battery,
 * network widget, 3-second CPU/RAM/network counters), plus a disk reading and
 * a weekly screen-time summary from Void's ledger. Nothing here is a
 * placeholder: a value the machine does not expose is shown as unavailable.
 */
export function SystemPanel({
	batteryLevel,
	isCharging,
	battery,
	cpuUsage,
	ramUsage,
	diskUsage,
	netUpSpeed,
	netDownSpeed,
	network,
	onRefreshBattery,
	onPrev,
	onNext
}: SystemPanelProps) {
	const [screenTime, setScreenTime] = useState<ScreenTimeSnapshot | null>(null);
	/** `null` means "now"; negative offsets step back through past weeks. */
	const [weekOffset, setWeekOffset] = useState(0);

	const loadScreenTime = useCallback(async () => {
		try {
			setScreenTime(await invoke<ScreenTimeSnapshot>("get_screen_time"));
		} catch (error) {
			console.error("Failed to read screen time:", error);
		}
	}, []);

	useEffect(() => {
		void loadScreenTime();
		const interval = window.setInterval(() => void loadScreenTime(), 15_000);
		return () => window.clearInterval(interval);
	}, [loadScreenTime]);

	// Battery details are read when the section opens and again only when the
	// webview reports a level or charging change, so there is no extra timer.
	useEffect(() => {
		onRefreshBattery();
	}, [batteryLevel, isCharging, onRefreshBattery]);

	const percent = battery?.present
		? (battery.percent ?? batteryLevel)
		: battery
			? null
			: batteryLevel;
	const pluggedIn = battery ? battery.plugged_in : isCharging;
	const charging = battery ? battery.charging : isCharging;
	const hasBattery = battery ? battery.present : true;
	const powerState =
		battery === null
			? "Reading battery…"
			: !hasBattery
				? "AC power"
				: charging
					? "Charging"
					: pluggedIn
						? "Plugged in"
						: "On battery";
	const batteryEstimate =
		battery === null
			? "Reading power state…"
			: !hasBattery
				? "Wall power"
				: charging
					? battery.seconds_to_full
						? `${formatDuration(battery.seconds_to_full)} to full`
						: "Charging"
					: !pluggedIn && battery.seconds_left
						? `${formatDuration(battery.seconds_left)} left`
						: pluggedIn && (battery.percent ?? 0) >= 100
							? "Fully charged"
							: pluggedIn
								? "Not charging"
								: "Estimate unavailable";

	const networkKind = network?.kind ?? "none";
	const networkName =
		networkKind === "wifi"
			? (network?.ssid ?? "Wi-Fi")
			: networkKind === "lan"
				? (network?.name ?? "Ethernet")
				: "No network connection";
	const connected = networkKind !== "none";

	const diskPercent = diskUsage ? Math.round(diskUsage.used_percent) : null;
	const diskFree = diskUsage ? `${diskUsage.free_gb.toFixed(1)} GB free` : "Reading disk…";

	const todayKey = screenTime?.today ?? dateKey(new Date());
	const byDate = new Map((screenTime?.days ?? []).map((day) => [day.date, day.seconds]));
	// Week view. `weekOffset` is 0 for the current week and negative before it.
	const currentWeekStart = mondayOf(parseDate(todayKey));
	const earliest = screenTime?.days[0]?.date ?? todayKey;
	const minWeekOffset = -Math.round(
		(currentWeekStart.getTime() - mondayOf(parseDate(earliest)).getTime()) / 604_800_000
	);
	const weekStart = addDays(currentWeekStart, weekOffset * 7);
	const weekEnd = addDays(weekStart, 6);
	const weekDays = Array.from({ length: 7 }, (_, index) => {
		const key = dateKey(addDays(weekStart, index));
		return { key, seconds: byDate.get(key) ?? 0, future: key > todayKey };
	});
	const hasPrevWeek = weekOffset > minWeekOffset;
	const hasNextWeek = weekOffset < 0;
	const shiftWeek = (delta: number) => {
		const next = Math.max(minWeekOffset, Math.min(0, weekOffset + delta));
		if (next === weekOffset) return;
		setWeekOffset(next);
	};
	const counted = weekDays.filter((day) => !day.future);
	const used = counted.filter((day) => day.seconds > 0);
	const weekTotal = used.reduce((sum, day) => sum + day.seconds, 0);
	const weekAverage = used.length === 0 ? 0 : weekTotal / used.length;
	const lowest = used.reduce<(typeof used)[number] | null>(
		(best, day) => (best === null || day.seconds < best.seconds ? day : best),
		null
	);
	const highest = used.reduce<(typeof used)[number] | null>(
		(best, day) => (best === null || day.seconds > best.seconds ? day : best),
		null
	);
	const hasUsage = used.length > 0;
	const range = lowest && highest ? Math.max(1, highest.seconds - lowest.seconds) : 1;
	const averagePct =
		hasUsage && lowest
			? Math.min(100, Math.max(0, ((weekAverage - lowest.seconds) / range) * 100))
			: 50;

	// A new day moves the current week on its own; drop the stale week selection.
	useEffect(() => {
		setWeekOffset(0);
	}, [todayKey]);

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

			<div className="sp-body sys-body">
				<div className="sys-grid">
					<section className="sp-card sys-card sys-power">
						<div className="sys-card-head">
							<span className="sys-card-title">
								<BatteryCharging size={12} strokeWidth={2.2} />
								Battery
							</span>
							<span className={`sys-pill${pluggedIn ? " active" : ""}`}>{powerState}</span>
						</div>
						<div className="sys-power-main">
							<span className="sys-big">{percent === null ? "—" : percent}<span className="sys-big-unit">%</span></span>
						</div>
						<div className="sys-power-segments">
							{Array.from({ length: 10 }).map((_, i) => (
								<span
									key={i}
									className={`sys-power-seg${
										i < Math.round((percent ?? 0) / 10) ? " on" : ""
									}${charging ? " charging" : ""}`}
								/>
							))}
						</div>
						<span className="sys-power-sub" title={batteryEstimate}>
							{batteryEstimate}
						</span>
					</section>

					<section className="sp-card sys-card sys-network">
						<div className="sys-card-head">
							<span className="sys-card-title">
								{networkKind === "wifi" ? (
									<Wifi size={12} strokeWidth={2.2} />
								) : networkKind === "lan" ? (
									<EthernetPort size={12} strokeWidth={2.2} />
								) : (
									<WifiOff size={12} strokeWidth={2.2} />
								)}
								<span className="sys-net-name" title={networkName}>
									{networkName}
								</span>
							</span>
							<span className={`sys-pill net${connected ? " connected" : ""}`}>
								<span className="sys-pill-dot" />
								{connected ? "Connected" : "Disconnected"}
							</span>
						</div>

						<div className="sys-net-row down">
							<div className="sys-net-left">
								<span className="sys-icon-box down">
									<ArrowDown size={16} strokeWidth={2.6} />
								</span>
								<span>Download</span>
							</div>
							<span className="sys-net-val">
								{splitRate(netDownSpeed).value}
								<small>{splitRate(netDownSpeed).unit}</small>
							</span>
						</div>
						<div className="sys-net-divider" />
						<div className="sys-net-row up">
							<div className="sys-net-left">
								<span className="sys-icon-box up">
									<ArrowUp size={16} strokeWidth={2.6} />
								</span>
								<span>Upload</span>
							</div>
							<span className="sys-net-val">
								{splitRate(netUpSpeed).value}
								<small>{splitRate(netUpSpeed).unit}</small>
							</span>
						</div>
					</section>

					<section className="sp-card sys-card sys-perf">
						<div className="sys-card-head">
							<span className="sys-card-title">
								<Cpu size={12} strokeWidth={2.2} />
								Performance
							</span>
							<span className="sys-disk-free">Disk: {diskFree}</span>
						</div>
						<div className="sys-rings">
							{(
								[
									{
										name: "CPU",
										value: cpuUsage,
										state:
											cpuUsage >= 85 ? "Busy" : cpuUsage >= 50 ? "Active" : "Light"
									},
									{
										name: "RAM",
										value: ramUsage,
										state:
											ramUsage >= 85 ? "High" : ramUsage >= 50 ? "In use" : "Low"
									},
									{
										name: "Disk",
										value: diskPercent ?? 0,
										state:
											(diskPercent ?? 0) >= 90
												? "Almost full"
												: (diskPercent ?? 0) >= 70
													? "In use"
													: "Plenty free"
									}
								] as const
							).map((metric) => {
								const value = Math.min(100, Math.max(0, metric.value));
								const r = 24;
								const circumference = 2 * Math.PI * r;
								return (
									<div className="sys-ring" key={metric.name}>
										<div className="sys-ring-visual">
											<svg viewBox="0 0 60 60" aria-hidden="true">
												<circle cx="30" cy="30" r={r} className="sys-ring-track" />
												<circle
													cx="30"
													cy="30"
													r={r}
													className="sys-ring-fill"
													strokeDasharray={`${(circumference * value) / 100} ${circumference}`}
													transform="rotate(-90 30 30)"
												/>
											</svg>
											<span className="sys-ring-value">{Math.round(metric.value)}%</span>
										</div>
										<span className="sys-ring-name">{metric.name}</span>
										<span className="sys-ring-state">{metric.state}</span>
									</div>
								);
							})}
						</div>
					</section>

					<section className="sp-card sys-card sys-time">
						<div className="sys-time-head">
							<span className="sys-time-head-group">
								<button
									type="button"
									className="sys-time-arrow"
									onClick={() => shiftWeek(-1)}
									disabled={!hasPrevWeek}
									aria-label="Previous week"
								>
									<ChevronLeft size={10} strokeWidth={2.6} />
								</button>
								<span className="sys-time-title">
									<span className="sys-time-icon" aria-hidden="true">
										<Clock3 size={13} strokeWidth={2.2} />
									</span>
									Screen time
								</span>
								<button
									type="button"
									className="sys-time-arrow"
									onClick={() => shiftWeek(1)}
									disabled={!hasNextWeek}
									aria-label="Next week"
								>
									<ChevronRight size={10} strokeWidth={2.6} />
								</button>
							</span>
							<span className="sys-time-range">{weekRangeLabel(weekStart, weekEnd)}</span>
						</div>

						<div className="sys-time-avg-block">
							<span className="sys-time-avg-label">Weekly average</span>
							<span className="sys-time-avg-value">
								{screenTime === null || !hasUsage ? "—" : <DurationText seconds={weekAverage} />}
							</span>
						</div>

						<div
							className="sys-time-scale"
							aria-hidden={!hasUsage}
							aria-label={
								hasUsage ? `Average between lowest and highest days this week` : undefined
							}
						>
							<div className="sys-time-track">
								<span className="sys-time-dot low" />
								<span
									className="sys-time-tick"
									style={{
										left: `calc(4px + (100% - 8px) * ${averagePct / 100})`
									}}
								/>
								<span className="sys-time-dot high" />
							</div>
							<div className="sys-time-scale-labels">
								<span>Lowest</span>
								<span>Highest</span>
							</div>
						</div>

						<div className="sys-time-extremes">
							<div className="sys-time-stat">
								<span className="sys-time-tag low">
									<Triangle size={7} fill="currentColor" strokeWidth={0} className="down" />
									Low
								</span>
								<span className="sys-time-stat-value">
									{screenTime === null || !lowest || !hasUsage ? (
										"—"
									) : (
										<DurationText seconds={lowest.seconds} />
									)}
								</span>
								<span className="sys-time-stat-day">
									{screenTime === null || !lowest || !hasUsage
										? "—"
										: weekdayCaption(lowest.key, todayKey)}
								</span>
							</div>
							<div className="sys-time-stat">
								<span className="sys-time-tag high">
									<Triangle size={7} fill="currentColor" strokeWidth={0} />
									High
								</span>
								<span className="sys-time-stat-value">
									{screenTime === null || !highest || !hasUsage ? (
										"—"
									) : (
										<DurationText seconds={highest.seconds} />
									)}
								</span>
								<span className="sys-time-stat-day">
									{screenTime === null || !highest || !hasUsage
										? "—"
										: weekdayCaption(highest.key, todayKey)}
								</span>
							</div>
						</div>
					</section>
				</div>
			</div>
		</div>
	);
}
