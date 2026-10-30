import React, { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, Copy, Download, Play } from "lucide-react";
import {
  commands,
  events,
  type SpeechModelStatus,
  type SpeechSettings,
} from "@/bindings";
import { useSettings } from "@/hooks/useSettings";
import { Button } from "../ui/Button";
import { Dropdown } from "../ui/Dropdown";
import { SettingContainer } from "../ui/SettingContainer";
import { SettingsGroup } from "../ui/SettingsGroup";
import { Slider } from "../ui/Slider";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { ShortcutInput } from "../settings/ShortcutInput";
import { useReadAloud } from "./useReadAloud";

const WORD_LIMITS = [100, 300, 600, 1000];
const MB = 1_000_000;

/** Claude Code's settings for reading its replies, and its notices, aloud. */
const claudeHooks = (exe: string): string => {
  const hook = [
    {
      hooks: [{ type: "command", command: exe, args: ["--speak-claude-hook"] }],
    },
  ];
  return JSON.stringify({ hooks: { Stop: hook, Notification: hook } }, null, 2);
};

/** Copies `text`, saying so for a moment. */
const CopyButton: React.FC<{
  text: string;
  idle: string;
  label?: string;
}> = ({ text, idle, label }) => {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout>>();
  useEffect(() => () => clearTimeout(timer.current), []);
  const copy = async () => {
    await navigator.clipboard.writeText(text);
    setCopied(true);
    clearTimeout(timer.current);
    timer.current = setTimeout(() => setCopied(false), 1500);
  };
  return (
    <Button variant="secondary" size="sm" onClick={copy} aria-label={label}>
      <span className="inline-flex items-center gap-1">
        {copied ? (
          <Check className="w-3 h-3" aria-hidden="true" />
        ) : (
          <Copy className="w-3 h-3" aria-hidden="true" />
        )}
        {copied ? t("speech.settings.copied") : idle}
      </span>
    </Button>
  );
};

/** A command line to copy. */
export const CopyRow: React.FC<{ text: string; label: string }> = ({
  text,
  label,
}) => {
  const { t } = useTranslation();
  return (
    <div className="flex items-center gap-2">
      <code className="flex-1 min-w-0 truncate px-2 py-1 rounded-md bg-wash text-xs text-ink-2">
        {text}
      </code>
      <CopyButton text={text} idle={t("speech.settings.copy")} label={label} />
    </div>
  );
};

export const ReadAloudPage: React.FC = () => {
  const { t } = useTranslation();
  const { settings, updateSetting } = useSettings();
  const [model, setModel] = useState<SpeechModelStatus | null>(null);
  const [download, setDownload] = useState<{
    downloaded: number;
    total: number;
  } | null>(null);
  const [downloadError, setDownloadError] = useState<string | null>(null);
  const [exe, setExe] = useState<string | null>(null);
  const { read } = useReadAloud();

  useEffect(() => {
    commands.getSpeechModelStatus().then(setModel);
    commands.getHandyExecutable().then(setExe);
    const unlisten = events.speechModelEvent.listen(({ payload }) => {
      if (payload.done) {
        setDownload(null);
        setDownloadError(payload.error);
        commands.getSpeechModelStatus().then(setModel);
      } else {
        setDownload({ downloaded: payload.downloaded, total: payload.total });
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const speech = settings?.speech;
  if (!speech) return null;

  const change = (patch: Partial<SpeechSettings>) =>
    updateSetting("speech", { ...speech, ...patch });

  const startDownload = async () => {
    setDownloadError(null);
    setDownload({ downloaded: 0, total: model?.bytes ?? 0 });
    const result = await commands.downloadSpeechModel();
    if (result.status === "error") {
      setDownload(null);
      setDownloadError(result.error);
    }
  };

  const voiceOptions = (model?.voices ?? []).map((name, i) => ({
    value: String(i),
    label: name,
  }));
  const wordOptions = WORD_LIMITS.map((count) => ({
    value: String(count),
    label: t("speech.settings.words", { count }),
  }));
  const handy = exe ? `& "${exe}"` : "handy";
  const installed = model?.installed ?? false;

  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      <SettingsGroup title={t("speech.settings.modelGroup")}>
        <SettingContainer
          title={t("speech.settings.model")}
          description={t("speech.settings.modelDescription", {
            size: Math.round((model?.bytes ?? 0) / MB),
          })}
          grouped
        >
          {installed ? (
            <span className="text-sm text-moss">
              {t("speech.settings.installed")}
            </span>
          ) : download ? (
            <div className="flex flex-col items-end gap-1 w-40">
              <div
                className="w-full h-1.5 rounded-full bg-line overflow-hidden"
                role="progressbar"
                aria-valuenow={download.downloaded}
                aria-valuemin={0}
                aria-valuemax={download.total}
              >
                <div
                  className="h-full bg-background-ui transition-[width] duration-300"
                  style={{
                    width: `${(download.downloaded / Math.max(1, download.total)) * 100}%`,
                  }}
                />
              </div>
              <span className="text-xs text-ink-3 tabular-nums">
                {t("speech.settings.downloading", {
                  done: Math.round(download.downloaded / MB),
                  total: Math.round(download.total / MB),
                })}
              </span>
            </div>
          ) : (
            <div className="flex flex-col items-end gap-1">
              <Button variant="secondary" size="sm" onClick={startDownload}>
                <span className="inline-flex items-center gap-1">
                  <Download className="w-3 h-3" aria-hidden="true" />
                  {t("speech.settings.download")}
                </span>
              </Button>
              {downloadError && (
                <span className="text-xs text-error">
                  {t("speech.settings.downloadFailed", {
                    error: downloadError,
                  })}
                </span>
              )}
            </div>
          )}
        </SettingContainer>
      </SettingsGroup>

      <SettingsGroup title={t("speech.settings.voiceGroup")}>
        <SettingContainer
          title={t("speech.settings.voice")}
          description={t("speech.settings.voiceDescription")}
          grouped
        >
          <div className="flex items-center gap-2">
            <Dropdown
              options={voiceOptions}
              selectedValue={String(speech.voice)}
              onSelect={(value) => change({ voice: Number(value) })}
            />
            <Button
              variant="secondary"
              size="sm"
              onClick={() => read(t("speech.settings.sample"))}
              disabled={!installed}
            >
              <span className="inline-flex items-center gap-1">
                <Play className="w-3 h-3" aria-hidden="true" />
                {t("speech.settings.tryIt")}
              </span>
            </Button>
          </div>
        </SettingContainer>
        <Slider
          value={speech.speed}
          onChange={(value) => change({ speed: Math.round(value * 100) / 100 })}
          min={0.7}
          max={1.6}
          step={0.05}
          label={t("speech.settings.speed")}
          description={t("speech.settings.speedDescription")}
          descriptionMode="tooltip"
          grouped
          formatValue={(value) =>
            t("speech.settings.speedValue", { value: value.toFixed(2) })
          }
        />
        <SettingContainer
          title={t("speech.settings.limit")}
          description={t("speech.settings.limitDescription")}
          grouped
        >
          <Dropdown
            options={wordOptions}
            selectedValue={String(speech.max_words)}
            onSelect={(value) => change({ max_words: Number(value) })}
          />
        </SettingContainer>
        <ToggleSwitch
          label={t("speech.settings.quiet")}
          description={t("speech.settings.quietDescription")}
          checked={speech.quiet_during_meetings}
          onChange={(on) => change({ quiet_during_meetings: on })}
          grouped
        />
        <ShortcutInput shortcutId="read_aloud" grouped />
      </SettingsGroup>

      <SettingsGroup title={t("speech.settings.appsGroup")}>
        <SettingContainer
          title={t("speech.settings.commands")}
          description={t("speech.settings.commandsDescription")}
          layout="stacked"
          grouped
        >
          <div className="space-y-1.5">
            <CopyRow
              text={`${handy} --speak "Build finished"`}
              label={t("speech.settings.copySpeak")}
            />
            <CopyRow
              text={`some-tool | ${handy} --speak-stdin`}
              label={t("speech.settings.copyStdin")}
            />
            <CopyRow
              text={`${handy} --stop-speaking`}
              label={t("speech.settings.copyStop")}
            />
          </div>
        </SettingContainer>
        {exe && (
          <SettingContainer
            title={t("speech.settings.claude")}
            description={t("speech.settings.claudeDescription")}
            layout="stacked"
            grouped
          >
            <div className="space-y-1.5">
              <pre className="max-h-40 overflow-auto px-2 py-1.5 rounded-md bg-wash text-xs text-ink-2">
                {claudeHooks(exe)}
              </pre>
              <CopyButton
                text={claudeHooks(exe)}
                idle={t("speech.settings.copyHooks")}
              />
            </div>
          </SettingContainer>
        )}
      </SettingsGroup>
    </div>
  );
};
