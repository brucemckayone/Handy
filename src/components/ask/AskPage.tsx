import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { ask as confirm } from "@tauri-apps/plugin-dialog";
import { commands, events, type AskSettings } from "@/bindings";
import { useSettings } from "@/hooks/useSettings";
import { Button } from "../ui/Button";
import { Dropdown } from "../ui/Dropdown";
import { SettingContainer } from "../ui/SettingContainer";
import { SettingsGroup } from "../ui/SettingsGroup";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { ShortcutInput } from "../settings/ShortcutInput";
import { CopyRow } from "../read-aloud/ReadAloudPage";

const MODELS = ["sonnet", "haiku", "opus"];
const SAME_WINDOW_MINUTES = [15, 60, 240, 0];
const KEEP_DAYS = [1, 7, 30, 0];

/** Ask: spoken questions answered by Claude Code. */
export const AskPage: React.FC = () => {
  const { t } = useTranslation();
  const { settings, updateSetting } = useSettings();
  const [claude, setClaude] = useState<string | null | undefined>(undefined);
  const [threadCount, setThreadCount] = useState(0);
  const [exe, setExe] = useState<string | null>(null);

  const countThreads = () =>
    commands.listAskThreads().then((list) => {
      if (list.status === "ok") setThreadCount(list.data.length);
    });

  useEffect(() => {
    commands.findClaudeCode().then(setClaude);
    commands.getHandyExecutable().then(setExe);
    countThreads();
    const unlisten = events.askDeletedEvent.listen(() => countThreads());
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const options = settings?.ask;
  if (!options) return null;

  const change = (patch: Partial<AskSettings>) =>
    updateSetting("ask", { ...options, ...patch });

  const minutesLabel = (minutes: number) =>
    minutes === 0
      ? t("ask.settings.whileOpen")
      : minutes % 60 === 0
        ? t("ask.settings.hours", { count: minutes / 60 })
        : t("ask.settings.minutes", { count: minutes });
  const daysLabel = (days: number) =>
    days === 0
      ? t("ask.settings.never")
      : t("ask.settings.days", { count: days });
  const handy = exe ? `& "${exe}"` : "handy";

  const deleteAll = async () => {
    const sure = await confirm(t("ask.settings.deleteAllConfirm"), {
      kind: "warning",
    });
    if (!sure) return;
    await commands.deleteAllAskThreads();
    countThreads();
  };

  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      <SettingsGroup title={t("ask.settings.askGroup")}>
        <ShortcutInput shortcutId="ask" grouped />
        <SettingContainer
          title={t("ask.settings.claude")}
          description={t("ask.settings.claudeDescription")}
          grouped
        >
          {claude === undefined ? null : claude ? (
            <div className="flex flex-col items-end">
              <span className="text-sm text-moss">
                {t("ask.settings.claudeFound")}
              </span>
              <span className="text-xs text-ink-2 font-mono break-all text-end">
                {claude}
              </span>
            </div>
          ) : (
            <span className="text-sm text-error">
              {t("ask.settings.claudeMissing")}
            </span>
          )}
        </SettingContainer>
        <SettingContainer
          title={t("ask.settings.model")}
          description={t("ask.settings.modelDescription")}
          grouped
        >
          <Dropdown
            options={MODELS.map((model) => ({
              value: model,
              label: t(`ask.settings.models.${model}`),
            }))}
            selectedValue={options.model}
            onSelect={(model) => change({ model })}
          />
        </SettingContainer>
        <ToggleSwitch
          label={t("ask.settings.readAloud")}
          description={t("ask.settings.readAloudDescription")}
          checked={options.read_aloud}
          onChange={(on) => change({ read_aloud: on })}
          grouped
        />
        <ToggleSwitch
          label={t("ask.settings.openPanel")}
          description={t("ask.settings.openPanelDescription")}
          checked={options.open_panel}
          onChange={(on) => change({ open_panel: on })}
          grouped
        />
      </SettingsGroup>

      <SettingsGroup title={t("ask.settings.contextGroup")}>
        <SettingContainer
          title={t("ask.settings.selection")}
          description={t("ask.settings.selectionDescription")}
          grouped
        >
          <span className="text-sm text-ink-2">{t("ask.settings.always")}</span>
        </SettingContainer>
        <ToggleSwitch
          label={t("ask.settings.screenshot")}
          description={t("ask.settings.screenshotDescription")}
          checked={options.screenshot}
          onChange={(on) => change({ screenshot: on })}
          grouped
        />
        <ToggleSwitch
          label={t("ask.settings.agents")}
          description={t("ask.settings.agentsDescription")}
          checked={options.use_agents}
          onChange={(on) => change({ use_agents: on })}
          grouped
        />
        <SettingContainer
          title={t("ask.settings.tools")}
          description={t("ask.settings.toolsDescription")}
          grouped
        >
          <span className="h-6 inline-flex items-center px-2 rounded-md bg-moss-wash text-xs text-moss">
            {t("ask.settings.readOnly")}
          </span>
        </SettingContainer>
      </SettingsGroup>

      <SettingsGroup title={t("ask.settings.threadsGroup")}>
        <SettingContainer
          title={t("ask.settings.sameWindow")}
          description={t("ask.settings.sameWindowDescription")}
          grouped
        >
          <Dropdown
            options={SAME_WINDOW_MINUTES.map((minutes) => ({
              value: String(minutes),
              label: minutesLabel(minutes),
            }))}
            selectedValue={String(options.same_window_minutes)}
            onSelect={(value) => change({ same_window_minutes: Number(value) })}
          />
        </SettingContainer>
        <SettingContainer
          title={t("ask.settings.keep")}
          description={t("ask.settings.keepDescription")}
          grouped
        >
          <Dropdown
            options={KEEP_DAYS.map((days) => ({
              value: String(days),
              label: daysLabel(days),
            }))}
            selectedValue={String(options.keep_days)}
            onSelect={(value) => change({ keep_days: Number(value) })}
          />
        </SettingContainer>
        <SettingContainer
          title={t("ask.settings.deleteAll")}
          description={t("ask.settings.deleteAllDescription", {
            count: threadCount,
          })}
          grouped
        >
          <Button
            variant="secondary"
            size="sm"
            onClick={deleteAll}
            disabled={threadCount === 0}
          >
            {t("ask.settings.deleteAllButton")}
          </Button>
        </SettingContainer>
      </SettingsGroup>

      <SettingsGroup title={t("ask.settings.appsGroup")}>
        <SettingContainer
          title={t("ask.settings.commands")}
          description={t("ask.settings.commandsDescription")}
          layout="stacked"
          grouped
        >
          <div className="space-y-1.5">
            <CopyRow
              text={`${handy} --ask "What changed on this branch?"`}
              label={t("ask.settings.copyAsk")}
            />
            <CopyRow
              text={`git diff | ${handy} --ask "Review this change" --ask-stdin`}
              label={t("ask.settings.copyStdin")}
            />
          </div>
        </SettingContainer>
      </SettingsGroup>
    </div>
  );
};
