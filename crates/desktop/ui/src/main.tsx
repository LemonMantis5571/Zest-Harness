import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { ErrorBoundary } from "./components/ErrorBoundary.tsx";
import "./index.css";
import App from "./App.tsx";
import { shouldPreserveNativeContextMenu } from "./lib/contextMenu.ts";
import { markStartup, measureStartup } from "./lib/startupPerf.ts";

markStartup("ui-module");

// A desktop webview should not expose Chromium's page menu over the app. Keep
// the native menu in editable controls so users can still paste, copy, and
// inspect text while composing a message or editing settings.
document.addEventListener(
  "contextmenu",
  (event) => {
    if (shouldPreserveNativeContextMenu(event.target)) return;
    event.preventDefault();
  },
  true
);

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </StrictMode>
);

markStartup("root-rendered");
measureStartup("root-rendered", "ui-module");
