import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
	BatteryCharging,
	Cpu,
	ExternalLink,
	HardDrive,
	MemoryStick,
	MonitorSpeaker,
	Volume2,
	VolumeX,
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
	volume: number;
	isMuted: boolean;
	onVolumeChange: (volume: number) => void;
	onToggleMute: () => void;
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
	netUpSpeed,
	netDownSpeed,
	network,
	volume,
	isMuted,
	onVolumeChange,
	onToggleMute,
	onRefreshBattery,
	onPrev,
	onNext
}: SystemPanelProps) {
	const [output, setOutput] = useState<string | null>(null);
	const [outputReady, setOutputReady] = useState(false);
	const mounted = useRef(true);

	const [netHistory, setNetHistory] = useState<Array<{ down: number; up: number }>>([
		{ down: netDownSpeed, up: netUpSpeed }
	]);

	useEffect(() => {
		setNetHistory((history) => [...history.slice(-23), { down: netDownSpeed, up: netUpSpeed }]);
	}, [netDownSpeed, netUpSpeed]);

	const loadOutput = async () => {
		try {
			const name = await invoke<string | null>("get_audio_output");
			if (mounted.current) {
				setOutput(name);
				setOutputReady(true);
			}
		} catch (error) {
			console.error("Failed to read audio output:", error);
			if (mounted.current) {
				setOutput(null);
				setOutputReady(true);
			}
		}
	};

	useEffect(() => {
		mounted.current = true;
		void loadOutput();
		return () => {
			mounted.current = false;
		};
		// loadOutput only reads state setters.
		// eslint-disable-next-line react-hooks/exhaustive-deps
	}, []);

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
	const outputName = outputReady ? output ?? "No output device" : "Reading output device…";
	const displayVolume = isMuted ? 0 : volume;
	const volumePercent = Math.round(displayVolume * 100);

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
								<span className="sys-metric-name">
									<Cpu size={8} strokeWidth={2.2} />
									CPU
								</span>
								<div className="sys-metric-center">
									<span className="sys-metric-value">{Math.round(cpuUsage)}%</span>
									<span className="sys-metric-track">
										<span style={{ width: `${Math.min(100, Math.max(0, cpuUsage))}%` }} />
									</span>
								</div>
							</div>

							<div className="sys-metric">
								<span className="sys-metric-name">
									<MemoryStick size={8} strokeWidth={2.2} />
									RAM
								</span>
								<div className="sys-metric-center">
									<span className="sys-metric-value">{Math.round(ramUsage)}%</span>
									<span className="sys-metric-track">
										<span style={{ width: `${Math.min(100, Math.max(0, ramUsage))}%` }} />
									</span>
								</div>
							</div>

							<div className="sys-metric">
								<span className="sys-metric-name">
									<HardDrive size={8} strokeWidth={2.2} />
									Disk
								</span>
								<div className="sys-metric-center">
									<span className="sys-metric-value">
										{diskPercent === null ? "—" : `${diskPercent}%`}
									</span>
									<span className="sys-metric-track">
										<span style={{ width: `${diskPercent ?? 0}%` }} />
									</span>
									<span className="sys-disk-free">{diskFree}</span>
								</div>
							</div>
						</div>
					</section>

					<section className="sp-card sys-card sys-audio">
						<div className="sys-audio-head">
							<span className="sys-card-title">
								<Volume2 size={12} strokeWidth={2.2} />
								Audio
							</span>
						</div>
						<div className="sys-audio-name" title={outputName}>
							{outputName}
						</div>
						<div className="sys-volume-row">
							<span className="sys-volume-value">{volumePercent}%</span>
							<button
								type="button"
								className={`sys-mute${isMuted ? " active" : ""}`}
								onClick={onToggleMute}
								aria-label={isMuted ? "Unmute" : "Mute"}
								title={isMuted ? "Unmute" : "Mute"}
							>
								{isMuted ? <VolumeX size={13} /> : <Volume2 size={13} />}
							</button>
							<div className="sys-volume-track">
								<span className="sys-volume-fill" style={{ width: `${volumePercent}%` }} />
								<span className="sys-volume-knob" style={{ left: `${volumePercent}%` }} />
								<input
									type="range"
									min="0"
									max="1"
									step="0.01"
									value={displayVolume}
									onChange={(event) => onVolumeChange(parseFloat(event.target.value))}
									aria-label="Output volume"
								/>
							</div>
							<Volume2 size={13} strokeWidth={2} className="sys-speaker-end" />
						</div>
						<div className="sys-audio-bottom">
							<span>
								<MonitorSpeaker size={10} strokeWidth={2} />
								Default output device
							</span>
							<span>
								Windows volume
								<ExternalLink size={9} strokeWidth={2} />
							</span>
						</div>
					</section>
				</div>
			</div>
		</div>
	);
}
