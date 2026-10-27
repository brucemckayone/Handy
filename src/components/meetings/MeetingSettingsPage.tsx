import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  ChevronLeft,
  Cloud,
  Download,
  Laptop,
  ShieldCheck,
} from "lucide-react";
import {
  commands,
  events,
  type MeetingSettings,
  type SpeakerModelStatus,
  type SummaryEngine,
} from "@/bindings";
import { useSettings } from "@/hooks/useSettings";
import { useSettingsStore } from "@/stores/settingsStore";
import { Button } from "../ui/Button";
import { Dropdown } from "../ui/Dropdown";
import { SettingContainer } from "../ui/SettingContainer";
import { SettingsGroup } from "../ui/SettingsGroup";
import { Textarea } from "../ui/Textarea";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { ShortcutInput } from "../settings/ShortcutInput";

const SAME_AS_DICTATION = "__same__";
const DEFAULT_DEVICE = "default";

interface MeetingSettingsPageProps {
  onBack: () => void;
}

export const MeetingSettingsPage: React.FC<MeetingSettingsPageProps> = ({
  onBack,
}) => {
  const { t } = useTranslation();
  const {
    settings,
    updateSetting,
    audioDevices,
    outputDevices,
    refreshAudioDevices,
    refreshOutputDevices,
  } = useSettings();
  const defaultSettings = useSettingsStore((state) => state.defaultSettings);
  const [claudePath, setClaudePath] = useState<string | null | undefined>();
  const [model, setModel] = useState<SpeakerModelStatus | null>(null);
  const [download, setDownload] = useState<{ percent: number } | null>(null);
  const [downloadError, setDownloadError] = useState<string | null>(null);
  const [prompt, setPrompt] = useState("");

  const meeting = settings?.meeting;

  useEffect(() => {
    commands.findClaudeCode().then(setClaudePath);
    commands.getSpeakerModelStatus().then(setModel);
    const unlisten = events.speakerModelEvent.listen(({ payload }) => {
      if (payload.done) {
        setDownload(null);
        setDownloadError(payload.error);
        commands.getSpeakerModelStatus().then(setModel);
      } else {
        setDownload({
          percent: Math.round(
            (payload.downloaded / Math.max(1, payload.total)) * 100,
          ),
        });
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  useEffect(() => {
    if (meeting) setPrompt(meeting.summary_prompt);
  }, [meeting?.summary_prompt]);

  if (!meeting) return null;

  const change = (patch: Partial<MeetingSettings>) =>
    updateSetting("meeting", { ...meeting, ...patch });

  const micOptions = [
    {
      value: SAME_AS_DICTATION,
      label: t("meetings.settingsPage.sameAsDictation"),
    },
    ...audioDevices.map((d) => ({ value: d.name, label: d.name })),
  ];
  const outputOptions = outputDevices.map((d) => ({
    value: d.name.toLowerCase() === DEFAULT_DEVICE ? DEFAULT_DEVICE : d.name,
    label:
      d.name.toLowerCase() === DEFAULT_DEVICE
        ? t("meetings.settingsPage.defaultOutput")
        : d.name,
  }));
  if (!outputOptions.some((o) => o.value === DEFAULT_DEVICE)) {
    outputOptions.unshift({
      value: DEFAULT_DEVICE,
      label: t("meetings.settingsPage.defaultOutput"),
    });
  }
  const bookmarkOptions = [15, 30, 60].map((seconds) => ({
    value: String(seconds),
    label:
      seconds === 60
        ? t("meetings.settingsPage.bookmarkMinute")
        : t("meetings.settingsPage.bookmarkSeconds", { seconds }),
  }));

  const startDownload = async () => {
    setDownloadError(null);
    setDownload({ percent: 0 });
    const result = await commands.downloadSpeakerModel();
    if (result.status === "error") {
      setDownload(null);
      setDownloadError(result.error);
    }
  };

  const engineButton = (
    engine: SummaryEngine,
    icon: React.ReactNode,
    title: string,
    detail: string,
  ) => {
    const selected = meeting.summary_engine === engine;
    return (
      <button
        type="button"
        role="radio"
        aria-checked={selected}
        onClick={() => change({ summary_engine: engine })}
        className={`flex-1 flex items-start gap-2 p-2.5 rounded-lg border text-start cursor-pointer ${selected ? "border-background-ui bg-logo-primary/15" : "border-line hover:border-logo-primary"}`}
      >
        {icon}
        <span className="flex flex-col">
          <span className="text-sm font-medium">{title}</span>
          <span className="text-xs text-ink-3">{detail}</span>
        </span>
      </button>
    );
  };

  const customUrl =
    settings?.post_process_providers?.find((p) => p.id === "custom")
      ?.base_url ?? "";
  const localModel =
    settings?.post_process_models?.custom?.trim() || "llama3.2:3b";

  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      <button
        type="button"
        onClick={onBack}
        className="inline-flex items-center gap-0.5 px-1 text-xs font-medium text-ink-3 hover:text-logo-primary cursor-pointer"
      >
        <ChevronLeft className="w-3.5 h-3.5 rtl:rotate-180" />
        {t("meetings.back")}
      </button>

      <SettingsGroup title={t("meetings.settingsPage.recording")}>
        <SettingContainer
          title={t("meetings.settingsPage.microphone")}
          description={t("meetings.settingsPage.microphoneDescription")}
          grouped
        >
          <Dropdown
            options={micOptions}
            selectedValue={meeting.microphone ?? SAME_AS_DICTATION}
            onSelect={(value) =>
              change({ microphone: value === SAME_AS_DICTATION ? null : value })
            }
            onOpen={refreshAudioDevices}
          />
        </SettingContainer>
        <SettingContainer
          title={t("meetings.settingsPage.output")}
          description={t("meetings.settingsPage.outputDescription")}
          grouped
        >
          <Dropdown
            options={outputOptions}
            selectedValue={meeting.output_device ?? DEFAULT_DEVICE}
            onSelect={(value) =>
              change({ output_device: value === DEFAULT_DEVICE ? null : value })
            }
            onOpen={refreshOutputDevices}
          />
        </SettingContainer>
        <SettingContainer
          title={t("meetings.settingsPage.bookmark")}
          description={t("meetings.settingsPage.bookmarkDescription")}
          grouped
        >
          <Dropdown
            options={bookmarkOptions}
            selectedValue={String(meeting.bookmark_seconds)}
            onSelect={(value) => change({ bookmark_seconds: Number(value) })}
          />
        </SettingContainer>
      </SettingsGroup>

      <SettingsGroup title={t("meetings.settingsPage.startStop")}>
        <ToggleSwitch
          label={t("meetings.settingsPage.ask")}
          description={t("meetings.settingsPage.askDescription")}
          checked={meeting.ask_when_call_starts}
          onChange={(on) => change({ ask_when_call_starts: on })}
          grouped
        />
        {meeting.ask_when_call_starts && (
          <SettingContainer
            title={t("meetings.settingsPage.apps")}
            description={t("meetings.settingsPage.askDescription")}
            grouped
          >
            <div className="flex flex-col gap-1 text-sm">
              {(
                [
                  ["detect_teams", "teams"],
                  ["detect_zoom", "zoom"],
                  ["detect_browser", "browser"],
                ] as const
              ).map(([key, label]) => (
                <label
                  key={key}
                  className="inline-flex items-center gap-2 cursor-pointer"
                >
                  <input
                    type="checkbox"
                    checked={meeting[key]}
                    onChange={() => change({ [key]: !meeting[key] })}
                    className="accent-background-ui"
                  />
                  {t(`meetings.settingsPage.${label}`)}
                </label>
              ))}
            </div>
          </SettingContainer>
        )}
        <ToggleSwitch
          label={t("meetings.settingsPage.autoStop")}
          description={t("meetings.settingsPage.autoStopDescription")}
          checked={meeting.stop_when_call_ends}
          onChange={(on) => change({ stop_when_call_ends: on })}
          grouped
        />
        <ShortcutInput shortcutId="meeting_toggle" grouped />
        <ShortcutInput shortcutId="meeting_bookmark" grouped />
      </SettingsGroup>

      <SettingsGroup title={t("meetings.settingsPage.transcript")}>
        <ToggleSwitch
          label={t("meetings.settingsPage.labels")}
          description={t("meetings.settingsPage.labelsDescription")}
          checked={meeting.label_speakers}
          onChange={(on) => change({ label_speakers: on })}
          grouped
        />
        {meeting.label_speakers && model && (
          <SettingContainer
            title={t("meetings.settingsPage.speakerModel")}
            description={t("meetings.settingsPage.speakerModelDescription", {
              size: Math.round(model.size_bytes / 1_000_000),
            })}
            grouped
          >
            {model.installed ? (
              <span className="text-sm text-moss">
                {t("meetings.settingsPage.installed")}
              </span>
            ) : download ? (
              <span className="text-sm text-ink-2 tabular-nums">
                {t("meetings.settingsPage.downloading", {
                  percent: download.percent,
                })}
              </span>
            ) : (
              <div className="flex flex-col items-end gap-1">
                <Button variant="secondary" size="sm" onClick={startDownload}>
                  <span className="inline-flex items-center gap-1">
                    <Download className="w-3 h-3" />
                    {t("meetings.settingsPage.download")}
                  </span>
                </Button>
                {downloadError && (
                  <span className="text-xs text-error">
                    {t("meetings.settingsPage.downloadFailed", {
                      error: downloadError,
                    })}
                  </span>
                )}
              </div>
            )}
          </SettingContainer>
        )}
        <ToggleSwitch
          label={t("meetings.settingsPage.dictation")}
          description={t("meetings.settingsPage.dictationDescription")}
          checked={meeting.leave_out_dictation}
          onChange={(on) => change({ leave_out_dictation: on })}
          grouped
        />
      </SettingsGroup>

      <SettingsGroup title={t("meetings.settingsPage.summary")}>
        <ToggleSwitch
          label={t("meetings.settingsPage.autoSummary")}
          description={t("meetings.settingsPage.autoSummaryDescription")}
          checked={meeting.summarise_automatically}
          onChange={(on) => change({ summarise_automatically: on })}
          grouped
        />
        <SettingContainer
          title={t("meetings.settingsPage.engine")}
          description={t("meetings.settingsPage.engineDescription")}
          layout="stacked"
          grouped
        >
          <div className="space-y-2">
            <div
              role="radiogroup"
              aria-label={t("meetings.settingsPage.engine")}
              className="flex gap-2 flex-wrap"
            >
              {engineButton(
                "claude_code",
                <Cloud className="w-4 h-4 mt-0.5 shrink-0 text-ink-3" />,
                t("meetings.engines.claude_code"),
                t("meetings.engines.claude_codeDetail"),
              )}
              {engineButton(
                "local",
                <Laptop className="w-4 h-4 mt-0.5 shrink-0 text-ink-3" />,
                t("meetings.engines.local"),
                t("meetings.engines.localDetail", { model: localModel }),
              )}
            </div>
            {meeting.summary_engine === "claude_code" ? (
              <div
                role="note"
                className="flex items-start gap-2 p-2 rounded-md bg-wash text-xs text-ink-2"
              >
                <ShieldCheck className="w-4 h-4 shrink-0" />
                <span className="space-y-0.5">
                  <span className="block">
                    {t("meetings.settingsPage.cloudNote")}
                  </span>
                  {claudePath === null && (
                    <span className="block text-warning">
                      {t("meetings.settingsPage.claudeMissing")}
                    </span>
                  )}
                  {claudePath && (
                    <span className="block text-ink-3 break-all">
                      {t("meetings.settingsPage.claudeFound", {
                        path: claudePath,
                      })}
                    </span>
                  )}
                </span>
              </div>
            ) : (
              <div
                role="note"
                className="p-2 rounded-md bg-wash text-xs text-ink-2"
              >
                {t("meetings.settingsPage.localNote", { url: customUrl })}
              </div>
            )}
          </div>
        </SettingContainer>
        <SettingContainer
          title={t("meetings.settingsPage.prompt")}
          description={t("meetings.settingsPage.promptDescription")}
          layout="stacked"
          grouped
        >
          <div className="space-y-1">
            <Textarea
              rows={4}
              value={prompt}
              onChange={(e) => setPrompt(e.target.value)}
              onBlur={() => {
                if (prompt !== meeting.summary_prompt)
                  change({ summary_prompt: prompt });
              }}
              className="w-full font-normal"
            />
            {defaultSettings?.meeting &&
              meeting.summary_prompt !==
                defaultSettings.meeting.summary_prompt && (
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() =>
                    change({
                      summary_prompt: defaultSettings.meeting!.summary_prompt,
                    })
                  }
                >
                  {t("meetings.settingsPage.resetPrompt")}
                </Button>
              )}
          </div>
        </SettingContainer>
      </SettingsGroup>

      <SettingsGroup title={t("meetings.settingsPage.storage")}>
        <SettingContainer
          title={t("meetings.settingsPage.storageTitle")}
          description={t("meetings.settingsPage.storageDescription")}
          descriptionMode="inline"
          grouped
        >
          <span className="text-sm text-ink-2 whitespace-nowrap">
            {t("meetings.settingsPage.storageValue")}
          </span>
        </SettingContainer>
      </SettingsGroup>
    </div>
  );
};
