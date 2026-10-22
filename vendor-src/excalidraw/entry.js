// The one module the studio's board loads (web/app/board.js): Excalidraw with the React it renders with.
import "@excalidraw/excalidraw/index.css";

export { default as React } from "react";
export { createRoot } from "react-dom/client";
export * from "@excalidraw/excalidraw";
