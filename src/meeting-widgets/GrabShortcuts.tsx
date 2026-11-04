import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands } from "@/bindings";
import { useOsType } from "@/hooks/useOsType";
import { keycapLabels } from "@/lib/utils/keyboard";

type Row = { keys: string[]; what: string };

const key =
  "font-[inherit] text-[11px] leading-4 px-[5px] rounded-[5px] border border-line-strong border-b-2 bg-background text-plum";

/** Every key the grab overlay knows, grouped as on the toolbar's `?`. */
export const GrabShortcuts: React.FC = () => {
  const { t } = useTranslation();
  const osType = useOsType();
  const [bindings, setBindings] = useState<Record<string, string>>({});

  useEffect(() => {
    commands.getAppSettings().then((result) => {
      if (result.status !== "ok") return;
      const all = result.data.bindings ?? {};
      setBindings({
        grab: all.ask_grab?.current_binding ?? "",
        snap: all.ask_snap?.current_binding ?? "",
        ask: all.ask?.current_binding ?? "",
      });
    });
  }, []);

  const bound = (id: string, what: string): Row[] =>
    bindings[id] ? [{ keys: keycapLabels(bindings[id], osType), what }] : [];
  const ctrl = t("ask.grab.keys.ctrl");
  const groups: { name: string; rows: Row[] }[] = [
    {
      name: t("ask.grab.keys.whileAsking"),
      rows: [
        ...bound("grab", t("ask.grab.keys.grab")),
        ...bound("snap", t("ask.grab.keys.snap")),
        ...bound("ask", t("ask.grab.keys.send")),
        {
          keys: [t("ask.grab.keyEsc"), t("ask.grab.keyEsc")],
          what: t("ask.grab.keys.cancel"),
        },
      ],
    },
    {
      name: t("ask.grab.keys.picking"),
      rows: [
        { keys: [t("ask.grab.keys.drag")], what: t("ask.grab.keys.region") },
        { keys: [t("ask.grab.keys.click")], what: t("ask.grab.keys.window") },
        { keys: ["F"], what: t("ask.grab.keys.screen") },
      ],
    },
    {
      name: t("ask.grab.keys.marking"),
      rows: [
        {
          keys: ["A", "R", "H", "P", "T", "N"],
          what: t("ask.grab.keys.tools"),
        },
        { keys: ["1", "2", "3"], what: t("ask.grab.keys.colors") },
        { keys: [ctrl, "Z"], what: t("ask.grab.keys.undo") },
        { keys: [t("ask.grab.keys.delete")], what: t("ask.grab.keys.remove") },
      ],
    },
    {
      name: t("ask.grab.keys.finishing"),
      rows: [
        { keys: [t("ask.grab.keyEnter")], what: t("ask.grab.keys.add") },
        {
          keys: [ctrl, t("ask.grab.keyEnter")],
          what: t("ask.grab.keys.addAndAsk"),
        },
        { keys: [t("ask.grab.keyEsc")], what: t("ask.grab.keys.drop") },
      ],
    },
  ];

  return (
    <section
      aria-label={t("ask.grab.keys.title")}
      className="w-[420px] max-w-[calc(100vw-32px)] px-[18px] py-4 flex flex-col gap-3.5 rounded-xl border border-line bg-background text-text shadow-lg"
    >
      <div className="flex items-center justify-between">
        <span className="text-[15px] font-semibold">
          {t("ask.grab.keys.title")}
        </span>
        <span className="text-xs text-ink-2 flex items-center gap-1">
          {t("ask.grab.keys.hideBefore")}
          <kbd className={key}>?</kbd>
          {t("ask.grab.keys.hideAfter")}
        </span>
      </div>
      {groups.map((group) => (
        <div key={group.name} className="flex flex-col gap-1.5">
          <span className="text-xs font-medium tracking-[0.04em] uppercase text-ink-3">
            {group.name}
          </span>
          {group.rows.map((row) => (
            <div
              key={row.what}
              className="flex items-center gap-2.5 text-[13px] leading-[18px]"
            >
              <span className="w-32 flex-none flex flex-wrap gap-[3px]">
                {row.keys.map((k, i) => (
                  <kbd key={i} className={key}>
                    {k}
                  </kbd>
                ))}
              </span>
              <span>{row.what}</span>
            </div>
          ))}
        </div>
      ))}
    </section>
  );
};
