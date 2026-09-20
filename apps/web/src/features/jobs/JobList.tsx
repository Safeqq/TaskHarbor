import { displayJobStatus, type Job } from "./api";
import { formatDateTime } from "./format";
import { JobProgress } from "./JobProgress";
import { StatusBadge } from "./StatusBadge";

interface JobListProps {
  jobs: Job[];
  selectedId: number | null;
  onSelect: (id: number) => void;
}

export function JobList({ jobs, selectedId, onSelect }: JobListProps) {
  const newestFirst = [...jobs].sort((left, right) => right.id - left.id);

  return (
    <ul className="job-list" aria-label="Jobs">
      {newestFirst.map((job) => (
        <li key={job.id}>
          <button
            className={`job-row${selectedId === job.id ? " job-row--selected" : ""}`}
            type="button"
            onClick={() => onSelect(job.id)}
            aria-pressed={selectedId === job.id}
          >
            <span className="job-row__identity">
              <span className="job-row__id">#{job.id.toString().padStart(3, "0")}</span>
              <span className="job-row__name">{job.name}</span>
              <span className="job-row__time">Created {formatDateTime(job.created_at)}</span>
            </span>
            <span className="job-row__state">
              <StatusBadge status={displayJobStatus(job)} />
              <JobProgress progress={job.progress} compact />
            </span>
            <ChevronIcon />
          </button>
        </li>
      ))}
    </ul>
  );
}

function ChevronIcon() {
  return (
    <svg className="job-row__chevron" viewBox="0 0 20 20" aria-hidden="true">
      <path d="m7 4 6 6-6 6" />
    </svg>
  );
}
