import type { ReactNode } from "react";

export type ToggleTone = "on" | "off" | "na" | "setup" | "busy";

interface ToggleProps {
  tone: ToggleTone;
  label?: ReactNode;
  disabled?: boolean;
  pressed?: boolean;
  onClick?: () => void;
  ariaLabel?: string;
  title?: string;
}

const DEFAULT_LABELS: Record<ToggleTone, string> = {
  on: "ON",
  off: "OFF",
  na: "N/A",
  setup: "SETUP",
  busy: "...",
};

export function Toggle({
  tone,
  label,
  disabled,
  pressed,
  onClick,
  ariaLabel,
  title,
}: ToggleProps) {
  return (
    <button
      type="button"
      className={`settings-toggle settings-toggle--${tone}`}
      disabled={disabled}
      aria-pressed={pressed}
      aria-label={ariaLabel}
      title={title}
      onClick={onClick}
    >
      {label ?? DEFAULT_LABELS[tone]}
    </button>
  );
}

export default Toggle;
