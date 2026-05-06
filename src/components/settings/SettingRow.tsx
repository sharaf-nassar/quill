import type { ReactNode } from "react";

interface SettingRowProps {
  label: ReactNode;
  description?: ReactNode;
  control: ReactNode;
  badge?: ReactNode;
}

export function SettingRow({ label, description, control, badge }: SettingRowProps) {
  return (
    <div className="settings-row">
      <div className="settings-row-meta">
        <div className="settings-row-label">
          {label}
          {badge != null && <span className="settings-row-badge">{badge}</span>}
        </div>
        {description != null && (
          <div className="settings-row-description">{description}</div>
        )}
      </div>
      <div className="settings-row-control">{control}</div>
    </div>
  );
}

export default SettingRow;
