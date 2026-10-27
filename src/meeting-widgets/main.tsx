import React from "react";
import ReactDOM from "react-dom/client";
import { listen } from "@tauri-apps/api/event";
import {
  applyTheme,
  getStoredTheme,
  syncThemeFromSettings,
} from "@/lib/utils/theme";
import type { Theme } from "@/bindings";
import "@/i18n";
import "@/App.css";
import { MeetingPill } from "./MeetingPill";
import { MeetingPrompt } from "./MeetingPrompt";
import { SpeechPill } from "./SpeechPill";

// The meeting pill, the call prompt and the speaking pill: small windows
// from one page, told apart by `?view=`. Like the overlay, each sets its own theme.
applyTheme(getStoredTheme());
syncThemeFromSettings();
listen<Theme>("theme-changed", (event) => applyTheme(event.payload));

const view = new URLSearchParams(window.location.search).get("view");

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    {view === "prompt" ? (
      <MeetingPrompt />
    ) : view === "speech" ? (
      <SpeechPill />
    ) : (
      <MeetingPill />
    )}
  </React.StrictMode>,
);
