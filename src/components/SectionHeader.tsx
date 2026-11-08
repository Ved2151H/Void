import { useEffect, useState } from "react";
import { ChevronLeft, ChevronRight, EthernetPort, Wifi, WifiOff } from "lucide-react";
import { BatteryIcon } from "../icons";
import { batteryColor } from "../batteryTooltip";
import "./SectionPanels.css";

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
				<span className="sp-battery-level" style={{ color: batteryColor(batteryLevel, isCharging) }}>
					{batteryLevel}%
				</span>
				<span className="sp-battery">
					<BatteryIcon level={batteryLevel} charging={isCharging} />
				</span>
			</div>
		</header>
	);
}
