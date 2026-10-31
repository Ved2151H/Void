import React from "react";
import ReactDOM from "react-dom/client";
import BottomCorners from "./BottomCorners";

document.addEventListener("contextmenu", (e) => e.preventDefault());

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
	<React.StrictMode>
		<BottomCorners />
	</React.StrictMode>
);
