import type { DisplayJobStatus } from "./api";
import { statusLabel } from "./format";

interface StatusBadgeProps {
  status: DisplayJobStatus;
}

export function StatusBadge({ status }: StatusBadgeProps) {
  return (
    <span className={`status-badge status-badge--${status}`}>
      <span className="status-badge__dot" aria-hidden="true" />
      {statusLabel(status)}
    </span>
  );
}
