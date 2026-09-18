import type { JobProgress as JobProgressValue } from "./api";

interface JobProgressProps {
  progress: JobProgressValue;
  compact?: boolean;
}

export function JobProgress({ progress, compact = false }: JobProgressProps) {
  const total = Math.max(progress.total, 1);
  const completed = Math.min(progress.completed, total);
  const percentage = Math.round((completed / total) * 100);

  return (
    <div className={compact ? "job-progress job-progress--compact" : "job-progress"}>
      <div className="job-progress__copy">
        <span>Progress</span>
        <strong>{percentage}%</strong>
      </div>
      <progress
        className="job-progress__bar"
        value={completed}
        max={total}
        aria-label={`${completed} of ${progress.total} items completed`}
      />
      {!compact && (
        <p>
          {progress.completed} of {progress.total} items complete
        </p>
      )}
    </div>
  );
}
