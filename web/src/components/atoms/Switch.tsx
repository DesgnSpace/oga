import type { ChangeEventHandler } from "react";

interface SwitchProps {
  checked: boolean;
  label?: string;
  accessibleName?: string;
  disabled?: boolean;
  onChange: ChangeEventHandler<HTMLInputElement>;
  className?: string;
}

export function Switch({
  checked,
  label,
  accessibleName = label,
  disabled = false,
  onChange,
  className = "",
}: SwitchProps) {
  const classes = ["settings-switch", disabled ? "settings-switch-disabled" : "", className]
    .filter(Boolean)
    .join(" ");

  return (
    <label className={classes}>
      {label ? <span className="settings-switch-label">{label}</span> : null}
      <span className="settings-switch-control">
        <input
          type="checkbox"
          role="switch"
          checked={checked}
          disabled={disabled}
          aria-label={accessibleName !== label ? accessibleName : undefined}
          onChange={onChange}
        />
        <span className="settings-switch-track" aria-hidden="true">
          <span className="settings-switch-thumb" />
        </span>
      </span>
    </label>
  );
}
