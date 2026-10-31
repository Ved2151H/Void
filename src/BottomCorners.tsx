import { motion } from "framer-motion";
import "./App.css";

/**
 * The bottom half of the "Screen Corners" effect. The window is only shown
 * while the setting is on and the notch is on screen, and it is pinned to the
 * physical bottom edge by the backend, so these masks mirror the top corners.
 */
export default function BottomCorners() {
	return (
		<div className="screen">
			<motion.div
				className="screen-corner bottom-left"
				initial={{ opacity: 0 }}
				animate={{ opacity: 1, filter: "blur(0px)" }}
			/>
			<motion.div
				className="screen-corner bottom-right"
				initial={{ opacity: 0 }}
				animate={{ opacity: 1, filter: "blur(0px)" }}
			/>
		</div>
	);
}
