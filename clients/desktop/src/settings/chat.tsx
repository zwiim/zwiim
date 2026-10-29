// Settings for talking to agents: which key sends a message, what one sent
// while an agent works does, whether a long paste becomes a file, how much of
// a reply and its context shows, and how the window tells you an agent
// finished while you were elsewhere.

import { playChime } from "@/lib/chime";
import {
  alwaysShowContext,
  autoConvertLongText,
  expandToolCalls,
  followUpMode,
  rememberAlwaysShowContext,
  rememberAutoConvertLongText,
  rememberExpandToolCalls,
  rememberFollowUpMode,
  rememberSendKey,
  rememberTurnAlerts,
  sendKey,
  turnAlerts,
  type FollowUpMode,
  type SendKey,
} from "@/lib/prefs";
import { sendShortcut } from "@/lib/send-key";
import { usePref } from "@/lib/use-pref";
import { cn } from "@/lib/utils";
import type { SettingMeta } from "@/settings/registry";
import { SettingRow, ToggleRow } from "@/settings/rows";

const SEND_KEYS: SendKey[] = ["enter", "mod_enter"];

export function SendKeySetting({ setting }: { setting: SettingMeta }) {
  const current = usePref(sendKey);
  return (
    <SettingRow setting={setting}>
      <div className="bg-muted flex gap-0.5 rounded-lg p-0.5" role="radiogroup" aria-label={setting.title}>
        {SEND_KEYS.map((key) => (
          <button
            key={key}
            className={cn(
              "text-ui rounded-md px-2.5 py-1 transition-colors",
              current === key ? "bg-background text-foreground shadow-sm" : "text-muted-foreground hover:text-foreground",
            )}
            role="radio"
            aria-checked={current === key}
            onClick={() => rememberSendKey(key)}
          >
            {sendShortcut(key)}
          </button>
        ))}
      </div>
    </SettingRow>
  );
}

const FOLLOW_UP_MODES: { mode: FollowUpMode; label: string }[] = [
  { mode: "queue", label: "Queues" },
  { mode: "steer", label: "Steers" },
];

export function FollowUpModeSetting({ setting }: { setting: SettingMeta }) {
  const current = usePref(followUpMode);
  return (
    <SettingRow setting={setting}>
      <div className="bg-muted flex gap-0.5 rounded-lg p-0.5" role="radiogroup" aria-label={setting.title}>
        {FOLLOW_UP_MODES.map(({ mode, label }) => (
          <button
            key={mode}
            className={cn(
              "text-ui rounded-md px-2.5 py-1 transition-colors",
              current === mode ? "bg-background text-foreground shadow-sm" : "text-muted-foreground hover:text-foreground",
            )}
            role="radio"
            aria-checked={current === mode}
            onClick={() => rememberFollowUpMode(mode)}
          >
            {label}
          </button>
        ))}
      </div>
    </SettingRow>
  );
}

export function LongPasteToggle({ setting }: { setting: SettingMeta }) {
  const on = usePref(autoConvertLongText);
  return <ToggleRow setting={setting} testId="long-paste" checked={on} onChange={rememberAutoConvertLongText} />;
}

export function ExpandToolCallsToggle({ setting }: { setting: SettingMeta }) {
  const on = usePref(expandToolCalls);
  return <ToggleRow setting={setting} testId="expand-tool-calls" checked={on} onChange={rememberExpandToolCalls} />;
}

export function AlwaysShowContextToggle({ setting }: { setting: SettingMeta }) {
  const on = usePref(alwaysShowContext);
  return <ToggleRow setting={setting} testId="always-show-context" checked={on} onChange={rememberAlwaysShowContext} />;
}

export function TurnNotifyToggle({ setting }: { setting: SettingMeta }) {
  const notify = usePref(() => turnAlerts().notify);
  return (
    <ToggleRow
      setting={setting}
      testId="turn-notify"
      checked={notify}
      onChange={(on) => rememberTurnAlerts({ notify: on })}
    />
  );
}

export function TurnSoundToggle({ setting }: { setting: SettingMeta }) {
  const sound = usePref(() => turnAlerts().sound);
  return (
    <ToggleRow
      setting={setting}
      testId="turn-sound"
      checked={sound}
      onChange={(on) => {
        rememberTurnAlerts({ sound: on });
        // Let the user hear what they turned on.
        if (on) playChime();
      }}
    />
  );
}
