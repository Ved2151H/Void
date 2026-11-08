import { useEffect, useState } from "react";
import {
	BatteryCharging,
	Cpu,
	HardDrive,
	MemoryStick,
	Music,
	Pause,
	Play,
	SkipBack,
	SkipForward,
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

interface SystemPanelMedia {
	title: string | null;
	artist: string | null;
	playing: boolean;
	active: boolean;
}

interface SystemPanelProps {
	batteryLevel: number;
	isCharging: boolean;
	battery: BatteryDetails | null;
	cpuUsage: number;
	ramUsage: number;
	diskUsage: DiskUsage | null;
	media: SystemPanelMedia;
	netUpSpeed: number;
	netDownSpeed: number;
	network: NetworkState | null;

	onMediaPrev: () => void;
	onMediaNext: () => void;
	onTogglePlayPause: () => void;
	onRefreshBattery: () => void;
	onPrev: () => void;
	onNext: () => void;
	onOpenSettings: () => void;
	onClose: () => void;
}

function formatRate(bytesPerSecond: number): string {
	if (bytesPerSecond < 1024) return `${Math.round(bytesPerSecond)} B/s`;
	if (bytesPerSecond < 1024 * 1024) return `${(bytesPerSecond / 1024).toFixed(1)} KB/s`;
	return `${(bytesPerSecond / (1024 * 1024)).toFixed(1)} MB/s`;
}

/** A tiny real-value history line. No synthetic motion, just the samples the
 *  existing 3-second metrics poll has already produced. */
function Sparkline({
	values,
	className,
	area = false,
	maxValue = 1
}: {
	values: number[];
	className?: string;
	area?: boolean;
	/** Fixed scale floor for percentage histories. */
	maxValue?: number;
}) {
	const width = 62;
	const height = 22;
	const samples = values.length === 0 ? [0, 0] : values.length === 1 ? [values[0], values[0]] : values;
	const max = Math.max(maxValue, ...samples);
	const step = width / Math.max(1, samples.length - 1);
	const points = samples
		.map((value, index) => {
			const x = index * step;
			const y = height - (Math.max(0, value) / max) * (height - 5) - 2.5;
			return `${x.toFixed(1)},${y.toFixed(1)}`;
		})
		.join(" ");
	const areaPoints = `0,${height} ${points} ${width},${height}`;
	return (
		<svg
			className={`sys-spark ${className ?? ""}`}
			viewBox={`0 0 ${width} ${height}`}
			preserveAspectRatio="none"
			aria-hidden="true"
		>
			{area && <polygon points={areaPoints} fill="currentColor" fillOpacity="0.12" />}
			<polyline
				points={points}
				fill="none"
				stroke="currentColor"
				strokeWidth="1.5"
				vectorEffect="non-scaling-stroke"
			/>
		</svg>
	);
}

/**
 * System shows the same live signals the notch already samples (battery,
 * network widget, 3-second CPU/RAM/network counters), plus a disk reading and
 * the default audio endpoint. Nothing here is a placeholder: a value the
 * machine does not expose is shown as unavailable.
 */
export function SystemPanel({
	batteryLevel,
	isCharging,
	battery,
	cpuUsage,
	ramUsage,
	diskUsage,
	media,
	netUpSpeed,
	netDownSpeed,
	network,
	onMediaPrev,
	onMediaNext,
	onTogglePlayPause,
	onRefreshBattery,
	onPrev,
	onNext
}: SystemPanelProps) {
	const [netHistory, setNetHistory] = useState<Array<{ down: number; up: number }>>([
		{ down: netDownSpeed, up: netUpSpeed }
	]);

	useEffect(() => {
		setNetHistory((history) => [...history.slice(-23), { down: netDownSpeed, up: netUpSpeed }]);
	}, [netDownSpeed, netUpSpeed]);

	// Battery details are read when the section opens and again only when the
	// webview reports a level or charging change, so there is no extra timer.
	useEffect(() => {
		onRefreshBattery();
	}, [batteryLevel, isCharging, onRefreshBattery]);

	const percent = battery?.present ? (battery.percent ?? batteryLevel) : battery ? null : batteryLevel;
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
			? network?.ssid ?? "Wi-Fi"
			: networkKind === "lan"
				? network?.name ?? "Ethernet"
				: "No network connection";
	const connected = networkKind !== "none";
	const netDown = netHistory.map((sample) => sample.down);
	const netUp = netHistory.map((sample) => sample.up);

	const diskPercent = diskUsage ? Math.round(diskUsage.used_percent) : null;
	const diskFree = diskUsage ? `${diskUsage.free_gb.toFixed(1)} GB free` : "Reading disk…";

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
								Power
							</span>
							<span className={`sys-charge-pill${charging ? " active" : ""}`}>{powerState}</span>
						</div>
						<div className="sys-power-main">
							<span className="sys-big">{percent === null ? "—" : `${percent}%`}</span>
							<span className="sys-power-full" title={batteryEstimate}>
								{batteryEstimate}
							</span>
						</div>
						<div className="sys-power-track">
							<span
								className={`sys-power-fill${charging ? " charging" : ""}${
									(percent ?? 100) <= 20 && !charging ? " low" : ""
								}`}
								style={{ width: `${Math.min(100, Math.max(0, hasBattery ? percent ?? 0 : 0))}%` }}
							/>
						</div>
					</section>

					<section className="sp-card sys-card sys-network">
						<div className="sys-net-top">
							<div className="sys-net-info">
								<span className="sys-card-title">
									{networkKind === "wifi" ? (
										<Wifi size={12} strokeWidth={2.2} />
									) : networkKind === "lan" ? (
										<EthernetPort size={12} strokeWidth={2.2} />
									) : (
										<WifiOff size={12} strokeWidth={2.2} />
									)}
									Network
								</span>
								<span className="sys-net-name" title={networkName}>
									{networkName}
								</span>
								<span className={`sys-connected${connected ? "" : " off"}`}>
									<span className="sys-dot" />
									{connected ? "Connected" : "Disconnected"}
								</span>
							</div>
							<div className="sys-net-graph" aria-label="Recent network activity">
								<Sparkline values={netDown} className="sys-graph-down" />
								<Sparkline values={netUp} className="sys-graph-up" />
							</div>
						</div>
						<div className="sys-net-stats">
							<div className="sys-net-stat">
								<span className="sys-arrow down">↓</span>
								<span className="sys-rate-text">
									<span>Download</span>
									<strong>{formatRate(netDownSpeed)}</strong>
								</span>
							</div>
							<div className="sys-net-stat">
								<span className="sys-arrow up">↑</span>
								<span className="sys-rate-text">
									<span>Upload</span>
									<strong>{formatRate(netUpSpeed)}</strong>
								</span>
							</div>
						</div>
					</section>

					<section className="sp-card sys-card sys-perf">
						<div className="sys-card-head">
							<span className="sys-card-title">
								<Cpu size={12} strokeWidth={2.2} />
								Performance
							</span>
						</div>
						<div className="sys-metrics">
							<div className="sys-metric">
								<div className="sys-metric-head">
									<span className="sys-metric-name">
										<Cpu size={8} strokeWidth={2.2} />
										CPU
									</span>
									<span className="sys-metric-value">{Math.round(cpuUsage)}%</span>
								</div>
								<span className="sys-metric-track">
									<span style={{ width: `${Math.min(100, Math.max(0, cpuUsage))}%` }} />
								</span>
							</div>

							<div className="sys-metric">
								<div className="sys-metric-head">
									<span className="sys-metric-name">
										<MemoryStick size={8} strokeWidth={2.2} />
										RAM
									</span>
									<span className="sys-metric-value">{Math.round(ramUsage)}%</span>
								</div>
								<span className="sys-metric-track">
									<span style={{ width: `${Math.min(100, Math.max(0, ramUsage))}%` }} />
								</span>
							</div>

							<div className="sys-metric">
								<div className="sys-metric-head">
									<span className="sys-metric-name">
										<HardDrive size={8} strokeWidth={2.2} />
										Disk
									</span>
									<span className="sys-metric-side">
										<span className="sys-disk-free">{diskFree}</span>
										<span className="sys-metric-value">
											{diskPercent === null ? "—" : `${diskPercent}%`}
										</span>
									</span>
								</div>
								<span className="sys-metric-track">
									<span style={{ width: `${diskPercent ?? 0}%` }} />
								</span>
							</div>
						</div>
					</section>

					<section className="sp-card sys-card sys-media">
						<div className="sys-card-head">
							<span className="sys-card-title">
								<Music size={12} strokeWidth={2.2} />
								Media
							</span>
						</div>
						<div className="sys-media-body">
							<span className="sys-media-title">
								{media.active ? media.title || "Unknown track" : "No media playing"}
							</span>
							<span className="sys-media-artist">
								{media.active
									? media.artist || "Unknown artist"
									: "Open a player app to control playback"}
							</span>
							<div className="sys-media-controls">
								<button
									type="button"
									className="sys-media-btn"
									onClick={onMediaPrev}
									disabled={!media.active}
									aria-label="Previous track"
								>
									<SkipBack size={14} />
								</button>
								<button
									type="button"
									className="sys-media-btn"
									onClick={onTogglePlayPause}
									disabled={!media.active}
									aria-label={media.playing ? "Pause" : "Play"}
								>
									{media.playing ? <Pause size={14} /> : <Play size={14} />}
								</button>
								<button
									type="button"
									className="sys-media-btn"
									onClick={onMediaNext}
									disabled={!media.active}
									aria-label="Next track"
								>
									<SkipForward size={14} />
								</button>
							</div>
						</div>
					</section>
				</div>
			</div>
		</div>
	);
}
