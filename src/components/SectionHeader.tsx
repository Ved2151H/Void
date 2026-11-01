import { useEffect, useState } from "react";
import { ChevronLeft, ChevronRight, EthernetPort, Wifi, WifiOff } from "lucide-react";
import "./SectionPanels.css";

/** Battery glyph in the header, using the reference's green fill. */
function HeaderBattery({ level, charging }: { level: number; charging: boolean }) {
	const percentage = Math.min(Math.max(level, 0), 100);
	const low = percentage <= 20 && !charging;
	return (
		<svg className="sp-battery" width="24" height="11" viewBox="0 0 24 11" aria-hidden="true">
			<rect
				x="1"
				y="1"
				width="18"
				height="9"
				rx="2.6"
				fill="none"
				stroke="currentColor"
				strokeOpacity="0.55"
				strokeWidth="1.2"
			/>
			<path
				d="M21.4 3.7V7.3"
				stroke="currentColor"
				strokeOpacity="0.55"
				strokeWidth="1.4"
				strokeLinecap="round"
			/>
			<rect
				x="2.7"
				y="2.7"
				width={Math.max(1, (percentage / 100) * 14.6)}
				height="5.6"
				rx="1.1"
				fill={low ? "#ff453a" : "#72ef64"}
			/>
		</svg>
	);
}

function clockLabel(date: Date): string {
	return date
		.toLocaleTimeString([], {
			hour: "2-digit",
			minute: "2-digit",
			hour12: true
		})
		.toLowerCase();
}

interface SectionHeaderProps {
	networkName: string;
	networkKind: "wifi" | "lan" | "none";
	batteryLevel: number;
	isCharging: boolean;
	onPrev: () => void;
	onNext: () => void;
}

/**
 * The shared header for System and Screen Time. It follows the reference:
 * network identity on the left, section arrows and the clock in the centre,
 * and battery state on the right.
 */
export function SectionHeader({
	networkName,
	networkKind,
	batteryLevel,
	isCharging,
	onPrev,
	onNext
}: SectionHeaderProps) {
	const [now, setNow] = useState(() => new Date());

	useEffect(() => {
		const timer = window.setInterval(() => setNow(new Date()), 1000);
		return () => window.clearInterval(timer);
	}, []);

	const NetworkIcon = networkKind === "wifi" ? Wifi : networkKind === "lan" ? EthernetPort : WifiOff;

	return (
		<header className="sp-header">
			<div className="sp-net">
				<NetworkIcon size={12} strokeWidth={2.2} />
				<span className="sp-net-name" title={networkName}>
					{networkName}
				</span>
			</div>

			<div className="sp-clock">
				<button type="button" className="sp-nav" onClick={onPrev} aria-label="Previous section">
					<ChevronLeft size={13} strokeWidth={2.4} />
				</button>
				<span className="sp-time">{clockLabel(now)}</span>
				<button type="button" className="sp-nav" onClick={onNext} aria-label="Next section">
					<ChevronRight size={13} strokeWidth={2.4} />
				</button>
			</div>

			<div className="sp-battery-wrap">
				<span className="sp-battery-level">{batteryLevel}%</span>
				<HeaderBattery level={batteryLevel} charging={isCharging} />
			</div>
		</header>
	);
}
