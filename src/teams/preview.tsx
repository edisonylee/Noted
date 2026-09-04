import React from "react";
import ReactDOM from "react-dom/client";
import "@fontsource-variable/geist";
import "../App.css";
import { TeamWorkspace } from "./TeamWorkspace";

if (!import.meta.env.DEV)
  throw new Error("The team preview is available only during development");
ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <div
      style={{
        maxWidth: 1280,
        margin: "0 auto",
        minHeight: "100vh",
        padding: "16px 0",
      }}
    >
      <div style={{ padding: "8px 24px", color: "var(--muted)", fontSize: 12 }}>
        Noted team preview · Local test service
      </div>
      <TeamWorkspace />
    </div>
  </React.StrictMode>,
);
