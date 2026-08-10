import React from "react";
import ReactDOM from "react-dom/client";
import "@fontsource-variable/geist";
import App from "./App";
import { RecordPrompt } from "./RecordPrompt";
import { api, isDesktop } from "./api";
import { configureAppTimeZone } from "./day";
import { ThemeProvider } from "./useTheme";
import "./App.css";

// The record-prompt popup is a second webview onto the same bundle — it must
// render just the prompt card, never the app (a 372px window otherwise falls
// into the phone layout). Detected by URL (the backend opens
// index.html?window=prompt) with the window label as belt-and-braces; the
// internals poke used before was unreliable and leaked the mini-app.
const isPromptWindow = (() => {
  if (new URLSearchParams(window.location.search).get("window") === "prompt") return true;
  if (!isDesktop) return false;
  try {
    type TauriInternals = {
      metadata?: {
        currentWebviewWindow?: { label?: string };
        currentWindow?: { label?: string };
      };
    };
    const meta = (window as unknown as { __TAURI_INTERNALS__?: TauriInternals })
      .__TAURI_INTERNALS__?.metadata;
    const label = meta?.currentWebviewWindow?.label ?? meta?.currentWindow?.label;
    return label === "record-prompt";
  } catch {
    return false;
  }
})();

// Desktop gets native window vibrancy behind a transparent webview; the
// "vibrant" class swaps solid backgrounds for glass where we want see-through.
// The phone browser keeps solid backgrounds.
if (isDesktop) document.body.classList.add("vibrant");

async function start() {
  if (!isPromptWindow) {
    try {
      const settings = await api.systemSettingsGet();
      configureAppTimeZone(settings.resolvedTimeZone);
    } catch {
      // Offline phone sessions use the last resolved zone cached by day.ts.
    }
  }

  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <ThemeProvider>{isPromptWindow ? <RecordPrompt /> : <App />}</ThemeProvider>
    </React.StrictMode>,
  );
}

void start();
