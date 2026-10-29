import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { watchAppearance } from "./lib/appearance";
import { defaultBridge } from "./lib/bridge";
import { watchTheme } from "./lib/theme";
import "./styles.css";

const bridge = defaultBridge();
watchTheme();
watchAppearance(bridge);

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App bridge={bridge} />
  </StrictMode>,
);
