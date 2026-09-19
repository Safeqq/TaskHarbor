import { artifactDownloadUrl, type Artifact, type Job } from "./api";
import { formatBytes, formatDateTime, formatDuration } from "./format";
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
        {job.image_settings ? (
          <>
            <DetailItem
              label="Maximum width"
              value={`${job.image_settings.max_width.toLocaleString()} px`}
            />
            <DetailItem label="JPEG quality" value={`${job.image_settings.jpeg_quality}/100`} />
          </>
        ) : (
          <DetailItem
            label="Configured delay"
            value={job.delay_ms === null ? "Not configured" : `${job.delay_ms.toLocaleString()} ms`}
          />
        )}
        <DetailItem label="Created" value={formatDateTime(job.created_at)} />
        <DetailItem label="Started" value={formatDateTime(job.started_at)} />
        <DetailItem label="Finished" value={formatDateTime(job.finished_at)} />
        <DetailItem
          label="Actual duration"
          value={formatDuration(job.result?.duration_ms ?? null)}
        />
      </dl>

      {job.job_type === "image_resize" && <ImageArtifacts job={job} />}

      {job.failure_message && (
        <div className="failure-note" role="alert">
          <FailureIcon />
          <div>
            <span>Job failed</span>
            <p>{job.failure_message}</p>
          </div>
        </div>
      )}

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

function ImageArtifacts({ job }: { job: Job }) {
  const outputsByIndex = new Map(job.outputs.map((output) => [output.item_index, output]));
  const inputs = [...job.inputs].sort((left, right) => left.item_index - right.item_index);

  return (
    <section className="artifact-section" aria-labelledby={`job-${job.id}-artifacts`}>
      <div className="artifact-section__heading">
        <div>
          <span className="section-kicker">Files</span>
          <h3 id={`job-${job.id}-artifacts`}>Input and output artifacts</h3>
        </div>
        <span>
          {job.outputs.length}/{job.inputs.length} published
        </span>
      </div>

      {inputs.length === 0 ? (
        <p className="artifact-empty">No image metadata is available for this job.</p>
      ) : (
        <div className="artifact-list">
          {inputs.map((input) => (
            <ArtifactPair
              key={input.id}
              input={input}
              output={outputsByIndex.get(input.item_index) ?? null}
              jobStatus={job.status}
            />
          ))}
        </div>
      )}
    </section>
  );
}

interface ArtifactPairProps {
  input: Artifact;
  output: Artifact | null;
  jobStatus: Job["status"];
}

function ArtifactPair({ input, output, jobStatus }: ArtifactPairProps) {
  return (
    <article className="artifact-pair">
      <div className="artifact-pair__title">
        <span>Item {(input.item_index + 1).toString().padStart(2, "0")}</span>
        <strong title={input.filename}>{input.filename}</strong>
      </div>
      <ArtifactMeta label="Input" artifact={input} />
      {output === null ? (
        <div className="artifact-meta artifact-meta--pending">
          <span>Output</span>
          <p>{jobStatus === "failed" ? "Not published" : "Waiting for worker"}</p>
        </div>
      ) : (
        <ArtifactMeta label="Output" artifact={output} downloadable />
      )}
    </article>
  );
}

interface ArtifactMetaProps {
  label: string;
  artifact: Artifact;
  downloadable?: boolean;
}

function ArtifactMeta({ label, artifact, downloadable = false }: ArtifactMetaProps) {
  return (
    <div className="artifact-meta">
      <span>{label}</span>
      <p>
        {artifact.width.toLocaleString()} × {artifact.height.toLocaleString()} px
      </p>
      <small>
        {formatBytes(artifact.byte_size)} · {artifact.media_type}
      </small>
      {downloadable && artifact.download_url !== null && (
        <a className="download-link" href={artifactDownloadUrl(artifact.download_url)} download>
          Download JPEG
          <DownloadIcon />
        </a>
      )}
    </div>
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

function FailureIcon() {
  return (
    <svg viewBox="0 0 20 20" aria-hidden="true">
      <path d="M10 3 2.5 17h15zM10 8v4M10 14.5v.2" />
    </svg>
  );
}

function DownloadIcon() {
  return (
    <svg viewBox="0 0 20 20" aria-hidden="true">
      <path d="M10 3v10M6 9l4 4 4-4M4 17h12" />
    </svg>
  );
}
