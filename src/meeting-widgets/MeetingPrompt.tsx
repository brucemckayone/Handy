import React, { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, CircleDot, Copy, Hand, Square, X } from "lucide-react";
import {
  commands,
  events,
  type CallApp,
  type PromptChoice,
  type PromptState,
  type RecordingInfo,
} from "@/bindings";
import { formatElapsed } from "@/components/meetings/format";
import { useElapsed } from "@/components/meetings/useElapsed";
import { formatKeyCombination } from "@/lib/utils/keyboard";
import { useOsType } from "@/hooks/useOsType";

/** Cards that only inform go away by themselves after this long. */
const INFO_CARD_MS = 8000;

const primary =
  "inline-flex items-center gap-1.5 min-h-8 px-3 py-1 rounded-[7px] text-[13px] font-semibold text-white bg-background-ui border border-background-ui hover:opacity-90 cursor-pointer";
const secondary =
  "inline-flex items-center gap-1.5 min-h-8 px-3 py-1 rounded-[7px] text-[13px] font-medium text-text bg-wash border border-line hover:border-logo-primary cursor-pointer";
const ghost =
  "px-1.5 py-1 rounded-md text-xs text-ink-2 hover:bg-wash cursor-pointer";
const link =
  "self-start text-[13px] font-semibold text-logo-primary hover:underline cursor-pointer";

/** The card near the tray: a call started (record it?), or ended (stop?). */
export const MeetingPrompt: React.FC = () => {
  const { t } = useTranslation();
  const osType = useOsType();
  const [state, setState] = useState<PromptState | null>(null);
  const [recording, setRecording] = useState<RecordingInfo | null>(null);
  const [shortcut, setShortcut] = useState("Ctrl+Alt+M");
  const [copied, setCopied] = useState(false);
  const [countdown, setCountdown] = useState(0);
  const elapsed = useElapsed(recording?.started_at);
  const lastCall = useRef<CallApp | null>(null);

  useEffect(() => {
    commands.getMeetingPromptState().then(setState);
    commands.getMeetingRecording().then(setRecording);
    commands.getAppSettings().then((result) => {
      if (result.status !== "ok") return;
      const binding = result.data.bindings?.meeting_toggle?.current_binding;
      if (binding) setShortcut(formatKeyCombination(binding, osType));
    });
    const unlisteners = [
      events.meetingPromptEvent.listen(({ payload }) => {
        setCopied(false);
        setState(payload.state);
      }),
      events.meetingRecordingEvent.listen(({ payload }) =>
        setRecording(payload.recording),
      ),
    ];
    return () => unlisteners.forEach((u) => u.then((fn) => fn()));
  }, [osType]);

  useEffect(() => {
    if (state && "app" in state) lastCall.current = state.app;
    if (state?.state === "ask_end") setCountdown(state.seconds);
    const informational =
      state?.state === "recording" ||
      state?.state === "saved" ||
      state?.state === "kept" ||
      state?.state === "never";
    if (!informational) return;
    const id = setTimeout(() => respond("close"), INFO_CARD_MS);
    return () => clearTimeout(id);
  }, [state]);

  useEffect(() => {
    if (state?.state !== "ask_end") return;
    const id = setInterval(() => setCountdown((c) => Math.max(0, c - 1)), 1000);
    return () => clearInterval(id);
  }, [state]);

  const respond = (choice: PromptChoice) =>
    commands.respondToMeetingPrompt(choice, lastCall.current);

  if (!state) return null;

  const appName = (app: CallApp) => t(`meetings.prompt.apps.${app}`);

  const copyConsent = async () => {
    const result = await commands.copyMeetingText(
      t("meetings.startDialog.consentMessage"),
    );
    if (result.status === "ok") setCopied(true);
  };

  const openMeetings = () => {
    commands.openMeetings();
    respond("close");
  };

  let body: React.ReactNode;
  switch (state.state) {
    case "ask_start":
      body = (
        <div className="flex flex-col gap-2.5">
          <div>
            <h2 id="prompt-title" className="m-0 text-[15px] font-semibold">
              {t("meetings.prompt.askTitle", { app: appName(state.app) })}
            </h2>
            <p className="mt-0.5 text-[13px] text-ink-2">
              {t("meetings.prompt.askBody")}
            </p>
          </div>
          <div className="flex items-center gap-2 px-2.5 py-2 rounded-[7px] bg-wash text-[12.5px]">
            <span className="flex-1">{t("meetings.prompt.consent")}</span>
            <button
              type="button"
              onClick={copyConsent}
              className={`${secondary} min-h-6 px-2 py-0.5 text-[11.5px]`}
            >
              <Copy className="w-3 h-3" aria-hidden="true" />
              {copied
                ? t("meetings.startDialog.copied")
                : t("meetings.startDialog.copyMessage")}
            </button>
          </div>
          <div className="flex items-center justify-between gap-2 flex-wrap">
            <button
              type="button"
              onClick={() => respond("never")}
              className={ghost}
            >
              {t("meetings.prompt.never", { app: appName(state.app) })}
            </button>
            <div className="flex gap-1.5">
              <button
                type="button"
                onClick={() => respond("not_now")}
                className={secondary}
              >
                {t("meetings.prompt.notNow")}
              </button>
              <button
                type="button"
                onClick={() => respond("record")}
                className={primary}
              >
                <CircleDot className="w-3.5 h-3.5" aria-hidden="true" />
                {t("meetings.prompt.record")}
              </button>
            </div>
          </div>
        </div>
      );
      break;
    case "recording":
      body = (
        <div className="flex flex-col gap-2">
          <h2
            id="prompt-title"
            className="m-0 flex items-center gap-2 text-[15px] font-semibold tabular-nums"
          >
            <span
              className="w-2 h-2 rounded-full bg-background-ui animate-pulse"
              aria-hidden="true"
            />
            {t("meetings.prompt.recordingTitle", {
              elapsed: formatElapsed(elapsed),
            })}
          </h2>
          <p className="m-0 text-[13px] text-ink-2">
            {t("meetings.prompt.recordingBody", { shortcut })}
          </p>
          <button type="button" onClick={openMeetings} className={link}>
            {t("meetings.prompt.openMeetings")}
          </button>
        </div>
      );
      break;
    case "never":
      body = (
        <div className="flex flex-col gap-2">
          <h2 id="prompt-title" className="m-0 text-[15px] font-semibold">
            {t("meetings.prompt.neverTitle", { app: appName(state.app) })}
          </h2>
          <p className="m-0 text-[13px] text-ink-2">
            {t("meetings.prompt.neverBody", { shortcut })}
          </p>
          <div className="flex items-center gap-3">
            <button
              type="button"
              onClick={() => respond("undo")}
              className={secondary}
            >
              {t("meetings.prompt.undo")}
            </button>
            <button type="button" onClick={openMeetings} className={link}>
              {t("meetings.prompt.openMeetings")}
            </button>
          </div>
        </div>
      );
      break;
    case "ask_end":
      body = (
        <div className="flex flex-col gap-2.5">
          <div>
            <h2 id="prompt-title" className="m-0 text-[15px] font-semibold">
              {t("meetings.prompt.endTitle")}
            </h2>
            <p className="mt-0.5 text-[13px] text-ink-2 tabular-nums">
              {t("meetings.prompt.endBody", {
                app: appName(state.app),
                seconds: countdown,
              })}
            </p>
          </div>
          <div
            className="h-1 rounded bg-line overflow-hidden"
            aria-hidden="true"
          >
            <div
              className="h-full bg-background-ui transition-[width] duration-1000 ease-linear"
              style={{
                width: `${(countdown / Math.max(1, state.seconds)) * 100}%`,
              }}
            />
          </div>
          <div className="flex justify-end gap-1.5">
            <button
              type="button"
              onClick={() => respond("keep_recording")}
              className={secondary}
            >
              {t("meetings.prompt.keep")}
            </button>
            <button
              type="button"
              onClick={() => respond("stop_now")}
              className={primary}
            >
              <Square className="w-3 h-3 fill-current" aria-hidden="true" />
              {t("meetings.prompt.stopNow")}
            </button>
          </div>
        </div>
      );
      break;
    case "saved":
      body = (
        <div className="flex flex-col gap-2">
          <h2
            id="prompt-title"
            className="m-0 flex items-center gap-2 text-[15px] font-semibold"
          >
            <Check className="w-4 h-4 text-moss" aria-hidden="true" />
            {t("meetings.prompt.savedTitle")}
          </h2>
          <p className="m-0 text-[13px] text-ink-2">
            {t("meetings.prompt.savedBody", {
              count: Math.max(1, state.minutes),
            })}
          </p>
          <button type="button" onClick={openMeetings} className={link}>
            {t("meetings.prompt.openMeetings")}
          </button>
        </div>
      );
      break;
    case "kept":
      body = (
        <div className="flex flex-col gap-1.5">
          <h2 id="prompt-title" className="m-0 text-[15px] font-semibold">
            {t("meetings.prompt.keptTitle")}
          </h2>
          <p className="m-0 text-[13px] text-ink-2">
            {t("meetings.prompt.keptBody")}
          </p>
        </div>
      );
      break;
  }

  return (
    <div className="w-full h-full flex items-end justify-end p-1 font-ui text-text">
      <div
        role="dialog"
        aria-labelledby="prompt-title"
        className="w-full flex flex-col gap-2.5 px-4 pt-3.5 pb-4 rounded-xl border border-line bg-background shadow-xl"
      >
        <div className="flex items-center justify-between">
          <span className="inline-flex items-center gap-1.5 text-[11.5px] text-ink-3">
            <Hand
              className="w-3.5 h-3.5 text-logo-primary"
              aria-hidden="true"
            />
            {t("meetings.prompt.brand")}
          </span>
          <button
            type="button"
            onClick={() =>
              respond(state.state === "ask_start" ? "not_now" : "close")
            }
            aria-label={t("meetings.prompt.dismiss")}
            className="w-[26px] h-[26px] inline-flex items-center justify-center rounded-md text-ink-3 hover:bg-wash cursor-pointer"
          >
            <X className="w-3.5 h-3.5" aria-hidden="true" />
          </button>
        </div>
        {body}
      </div>
    </div>
  );
};
