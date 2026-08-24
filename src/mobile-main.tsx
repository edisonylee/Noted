import React from "react";
import ReactDOM from "react-dom/client";
import "@fontsource-variable/geist";
import { MobileShell } from "./MobileShell";
import { reportMobileDeepLinkStartupError, startMobileDeepLinks } from "./mobileDeepLinks";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <MobileShell />
  </React.StrictMode>,
);

void startMobileDeepLinks().catch((reason: unknown) => {
  reportMobileDeepLinkStartupError(reason);
});
