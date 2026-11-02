import { useEffect, useState } from "react";

/**
 * Active while interacting, then a grace period before the surface peeks away.
 * The effect intentionally only re-runs when interaction starts/stops.
 */
export function useInteractionState(isAnyInteraction: boolean) {
	const [state, setState] = useState<"active" | "grace" | "none">("none");

	useEffect(() => {
		if (isAnyInteraction) {
			setState("active");
		} else if (state !== "none") {
			setState("grace");
			const timer = setTimeout(() => setState("none"), 800);
			return () => clearTimeout(timer);
		}
		// eslint-disable-next-line react-hooks/exhaustive-deps
	}, [isAnyInteraction]);

	return state;
}
