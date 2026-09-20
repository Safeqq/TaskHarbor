import { formatDateTime, formatRefreshTime } from "../jobs/format";
import { type Worker, type WorkerStatus } from "./api";
import { useWorkers } from "./useWorkers";

interface WorkersPageProps {
  pollIntervalMs: number;
}

export function WorkersPage({ pollIntervalMs }: WorkersPageProps) {
  const { workers, hasLoaded, error, isRefreshing, lastUpdated, refresh } =
    useWorkers(pollIntervalMs);
  const online = workers.filter((worker) => worker.status === "online").length;
  const activeAttempts = workers.reduce((total, worker) => total + worker.active_attempts, 0);
  const totalCapacity = workers
    .filter((worker) => worker.status === "online")
    .reduce((total, worker) => total + worker.concurrency_limit, 0);

  return (
    <main id="main-content">
      <section className="hero workers-hero" aria-labelledby="workers-title">
        <div>
          <div className="eyebrow">
            <span>Operations</span>
            <span aria-hidden="true">/</span>
            <strong>Workers</strong>
          </div>
          <h1 id="workers-title">Workers, visible.</h1>
          <p>
            Heartbeats show recent contact. Active attempts remain protected by renewable leases
            and per-attempt claim tokens.
          </p>
        </div>
        <div className="sync-card">
          <span>
            <small>Automatic refresh</small>
            <strong>{formatRefreshTime(lastUpdated)}</strong>
          </span>
        </div>
      </section>

      <section className="worker-overview" aria-label="Worker summary">
        <WorkerMetric label="Online workers" value={`${online}/${workers.length}`} />
        <WorkerMetric label="Active attempts" value={activeAttempts.toString()} />
        <WorkerMetric label="Online capacity" value={totalCapacity.toString()} />
      </section>

      <section className="workers-card" aria-labelledby="worker-list-title">
        <div className="jobs-card__header">
          <div>
            <div className="section-kicker">Process registry</div>
            <h2 id="worker-list-title">Worker fleet</h2>
          </div>
          <div className="jobs-card__actions">
            <span>{workers.length} registered</span>
            <button type="button" className="icon-button" onClick={refresh}>
              {isRefreshing ? "…" : "↻"}
              <span className="sr-only">Refresh workers</span>
            </button>
          </div>
        </div>

        {error && (
          <div className="inline-alert" role="alert">
            <span>{error}</span>
            <button type="button" onClick={refresh}>Retry</button>
          </div>
        )}
        {!hasLoaded && !error && <p className="worker-state" role="status">Loading workers…</p>}
        {hasLoaded && workers.length === 0 && !error && (
          <p className="worker-state">No worker has registered yet.</p>
        )}
        {workers.length > 0 && (
          <div className="worker-grid">
            {workers.map((worker) => <WorkerCard key={worker.id} worker={worker} />)}
          </div>
        )}
      </section>

      <aside className="lease-note">
        <strong>How offline is decided</strong>
        <p>
          Offline means the stored heartbeat deadline has passed. Recovery still waits for each
          active attempt lease to expire before another worker can claim a retry.
        </p>
      </aside>
    </main>
  );
}

function WorkerMetric({ label, value }: { label: string; value: string }) {
  return (
    <article>
      <span>{label}</span>
      <strong>{value}</strong>
    </article>
  );
}

function WorkerCard({ worker }: { worker: Worker }) {
  return (
    <article className="worker-card">
      <div className="worker-card__header">
        <span className="worker-card__icon" aria-hidden="true">W</span>
        <div>
          <strong>{worker.name}</strong>
          <span title={worker.id}>{worker.id.slice(0, 8)}</span>
        </div>
        <WorkerStatusPill status={worker.status} />
      </div>
      <div className="worker-capacity">
        <span>Active slots</span>
        <strong>{worker.active_attempts}/{worker.concurrency_limit}</strong>
        <div aria-hidden="true">
          <span
            style={{
              width: `${Math.min(100, (worker.active_attempts / worker.concurrency_limit) * 100)}%`,
            }}
          />
        </div>
      </div>
      <dl>
        <div><dt>Last heartbeat</dt><dd>{formatDateTime(worker.last_heartbeat_at)}</dd></div>
        <div><dt>Lease duration</dt><dd>{worker.lease_duration_seconds}s</dd></div>
        <div><dt>Started</dt><dd>{formatDateTime(worker.started_at)}</dd></div>
        <div>
          <dt>{worker.status === "stopped" ? "Stopped" : "Heartbeat deadline"}</dt>
          <dd>{formatDateTime(worker.stopped_at ?? worker.heartbeat_expires_at)}</dd>
        </div>
      </dl>
    </article>
  );
}

function WorkerStatusPill({ status }: { status: WorkerStatus }) {
  return <span className={`worker-status worker-status--${status}`}>{status}</span>;
}
