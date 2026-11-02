import { useMarqueeOverflow } from "../hooks/useMarqueeOverflow";

/** A single-line title that scrolls only while it overflows its container. */
export function TitleMarquee({
	title,
	wrapClassName,
	textClassName,
	overflowClassName,
	wrapStyle
}: {
	title: string;
	wrapClassName: string;
	textClassName: string;
	overflowClassName: string;
	wrapStyle?: React.CSSProperties;
}) {
	const { textRef, containerRef, isOverflowing, scrollDistance, scrollDuration } =
		useMarqueeOverflow(title);

	return (
		<div ref={containerRef} className={wrapClassName} style={wrapStyle}>
			<span
				ref={textRef}
				className={`${textClassName} ${isOverflowing ? overflowClassName : ""}`}
				style={
					{
						"--scroll-distance": isOverflowing ? `-${scrollDistance}px` : undefined,
						"--scroll-duration": `${scrollDuration}s`
					} as React.CSSProperties
				}
			>
				{title}
			</span>
		</div>
	);
}
