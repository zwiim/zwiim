// The shapes a setting takes in the settings dialog. Each row reads its title
// and description from its registry entry, so what search matches is what the
// row shows, and each marks its root with the setting's id for deep links.

import type { ReactNode } from "react";
import type { SettingMeta } from "@/settings/registry";

/** A title (and description) on the left, the control on the right, and what it shows the width of the row below. */
export function SettingRow({
  setting,
  children,
  below,
}: {
  setting: SettingMeta;
  children: ReactNode;
  below?: ReactNode;
}) {
  return (
    <div className="flex flex-col gap-2 border-b pb-3" data-setting-id={setting.id}>
      <div className="flex items-center gap-3">
        <div className="flex min-w-0 flex-1 flex-col gap-0.5">
          <span className="text-ui">{setting.title}</span>
          {setting.description && <span className="text-muted-foreground">{setting.description}</span>}
        </div>
        {children}
      </div>
      {below}
    </div>
  );
}

/** One on/off choice, with the reason a change failed under it. */
export function ToggleRow({
  setting,
  checked,
  disabled,
  error,
  testId,
  onChange,
}: {
  setting: SettingMeta;
  checked: boolean;
  disabled?: boolean;
  error?: string | null;
  testId?: string;
  onChange(checked: boolean): void;
}) {
  return (
    <section className="flex flex-col gap-1 rounded-md border p-3" data-setting-id={setting.id}>
      <label className="flex items-center gap-2">
        <input
          type="checkbox"
          data-testid={testId}
          checked={checked}
          disabled={disabled}
          onChange={(e) => onChange(e.target.checked)}
        />
        <span className="font-medium">{setting.title}</span>
      </label>
      {setting.description && <p className="text-muted-foreground">{setting.description}</p>}
      {error && (
        <p className="text-destructive" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}
