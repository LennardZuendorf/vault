import { getVersion } from "@tauri-apps/api/app";
import { useEffect, useState } from "react";
import { versionLine } from "../lib/version";
import { useStore } from "../store";
import { AgentSettings } from "./AgentSettings";
import { AppearanceSettings } from "./AppearanceSettings";
import { BrowserSettings } from "./BrowserSettings";
import { CodeSettings } from "./CodeSettings";
import { NotificationSettings } from "./NotificationSettings";
import { ObservabilitySettings } from "./ObservabilitySettings";
import { PhoneSettings } from "./PhoneSettings";
import { SlackSettings } from "./SlackSettings";
import { TagsSettings } from "./TagsSettings";
import { TicketsSettings } from "./TicketsSettings";
import { Modal } from "./ui";

type Section =
  | "appearance"
  | "notifications"
  | "tags"
  | "agents"
  | "browser"
  | "phone"
  | "jira"
  | "github"
  | "slack"
  | "observability";

/**
 * The tabs (SET-6): this Mac's own first, then what it connects
 * to. Named for what each does, not the product behind it.
 */
const TABS: [Section, string][] = [
  ["appearance", "Appearance"],
  ["notifications", "Notifications"],
  ["tags", "Tags"],
  ["agents", "Agents"],
  ["browser", "Browser"],
  ["phone", "Remote access"],
  ["jira", "Tickets"],
  ["github", "Code"],
  ["slack", "Messaging"],
  ["observability", "Logs"],
];

export function Settings() {
  const [version, setVersion] = useState<string | null>(null);
  useEffect(() => {
    void getVersion()
      .then(setVersion)
      .catch(() => {});
  }, []);
  const line = version ? versionLine(version, import.meta.env.VITE_LAYER_BUILD) : null;
  const toggleSettings = useStore((s) => s.toggleSettings);
  const [section, setSection] = useState<Section>(
    () => (useStore.getState().settingsSection as Section | null) ?? "appearance",
  );

  return (
    <Modal
      title="Settings"
      className="settings"
      wide
      tall
      onClose={() => toggleSettings(false)}
      toolbar={
        <div className="ui-tabs section-tabs" role="tablist">
          {TABS.map(([id, label]) => (
            <button
              key={id}
              className="ui-tab"
              role="tab"
              aria-selected={section === id}
              onClick={() => setSection(id)}
            >
              {label}
            </button>
          ))}
        </div>
      }
    >
      {section === "appearance" && <AppearanceSettings />}
      {section === "tags" && <TagsSettings />}
      {section === "notifications" && (
        <NotificationSettings toMessaging={() => setSection("slack")} />
      )}
      {section === "agents" && <AgentSettings />}

      {section === "slack" && <SlackSettings />}

      {section === "jira" && <TicketsSettings toCode={() => setSection("github")} />}
      {section === "github" && <CodeSettings />}

      {section === "observability" && <ObservabilitySettings />}
      {section === "phone" && <PhoneSettings />}
      {section === "browser" && <BrowserSettings />}

      {/* Settings ends with the app's name and version, on every page
          (SET-11, studio/design/behaviour.md). */}
      {line && (
        <p className="settings-version" title={line.title}>
          {line.text}
        </p>
      )}
    </Modal>
  );
}
