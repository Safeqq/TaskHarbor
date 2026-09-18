import type { Job } from "./api";
import { formatDateTime, formatDuration } from "./format";
import { JobProgress } from "./JobProgress";
import { StatusBadge } from "./StatusBadge";

interface JobDetailProps {
  job: Job | null;
}

export function JobDetail({ job }: JobDetailProps) {
  if (job === null) {
    return (
      <section className="detail-card detail-card--empty" aria-labelledby="job-detail-title">
        <div className="empty-detail-icon" aria-hidden="true">
          <DetailIcon />
        </div>
        <div>
          <div className="section-kicker">Job detail</div>
          <h2 id="job-detail-title">Select a job to inspect it</h2>
          <p>Status, progress, timing, and worker output will appear here.</p>
        </div>
      </section>
    );
  }

  return (
    <section className="detail-card" aria-labelledby="job-detail-title">
      <div className="detail-card__header">
        <div>
          <div className="section-kicker">Job #{job.id.toString().padStart(3, "0")}</div>
          <h2 id="job-detail-title">{job.name}</h2>
        </div>
        <StatusBadge status={job.status} />
      </div>

      <JobProgress progress={job.progress} />

      <dl className="detail-grid">
        <DetailItem label="Job type" value={job.job_type} mono />
        <DetailItem label="Configured delay" value={`${job.delay_ms.toLocaleString()} ms`} />
        <DetailItem label="Created" value={formatDateTime(job.created_at)} />
        <DetailItem label="Started" value={formatDateTime(job.started_at)} />
        <DetailItem label="Finished" value={formatDateTime(job.finished_at)} />
        <DetailItem
          label="Actual duration"
          value={formatDuration(job.result?.duration_ms ?? null)}
        />
      </dl>

      {job.result && (
        <div className="result-note">
          <ResultIcon />
          <div>
            <span>Worker result</span>
            <p>{job.result.message}</p>
          </div>
        </div>
      )}
    </section>
  );
}

interface DetailItemProps {
  label: string;
  value: string;
  mono?: boolean;
}

function DetailItem({ label, value, mono = false }: DetailItemProps) {
  return (
    <div>
      <dt>{label}</dt>
      <dd className={mono ? "mono" : undefined}>{value}</dd>
    </div>
  );
}

function DetailIcon() {
  return (
    <svg viewBox="0 0 24 24">
      <path d="M7 3h7l4 4v14H7z" />
      <path d="M14 3v5h5M10 12h5M10 16h5" />
    </svg>
  );
}

function ResultIcon() {
  return (
    <svg viewBox="0 0 20 20" aria-hidden="true">
      <path d="m4 10 4 4 8-8" />
    </svg>
  );
}
