import React from "react";
import ReactDOM from "react-dom/client";
import "@agent-hud/app/styles.css";
import App from "@agent-hud/app";
import { workbenchPage } from "./WorkbenchPage";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <React.Suspense fallback={null}>
      <App extensionPages={[workbenchPage]} />
    </React.Suspense>
  </React.StrictMode>,
);
