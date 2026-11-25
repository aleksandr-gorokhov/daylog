import { getCurrentWindow } from "@tauri-apps/api/window";
import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { PausePrompt } from "./components/PausePrompt";
import "./styles.css";

// Интерфейс общий для двух окон (tauri.conf.json): главное показывает дни,
// `pause-prompt` — вопрос о причине паузы.
const Root = getCurrentWindow().label === "pause-prompt" ? PausePrompt : App;

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <Root />
  </React.StrictMode>,
);
