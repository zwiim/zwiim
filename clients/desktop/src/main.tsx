import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { defaultBridge } from "./lib/bridge";
import { watchTheme } from "./lib/theme";
import "./styles.css";

watchTheme();

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App bridge={defaultBridge()} />
  </StrictMode>,
);
