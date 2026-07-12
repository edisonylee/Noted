import React from "react";
import ReactDOM from "react-dom/client";
import "@fontsource-variable/geist";
import App from "./App";
import { RecordPrompt } from "./RecordPrompt";
import { isDesktop } from "./api";
import "./App.css";

// The record-prompt popup is a second webview onto the same bundle — it renders
// just the prompt card, not the app. Label is read straight off the internals
// so the phone browser (no Tauri) never touches the window API.
const isPromptWindow = (() => {
  if (!isDesktop) return false;
  try {
    type TauriInternals = { metadata?: { currentWebviewWindow?: { label?: string } } };
    const internals = (window as unknown as { __TAURI_INTERNALS__?: TauriInternals })
      .__TAURI_INTERNALS__;
    return internals?.metadata?.currentWebviewWindow?.label === "record-prompt";
  } catch {
    return false;
  }
})();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>{isPromptWindow ? <RecordPrompt /> : <App />}</React.StrictMode>,
);
