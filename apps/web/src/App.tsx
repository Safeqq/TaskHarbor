import { useEffect, useMemo, useState } from "react";

import { CreateJobForm } from "./features/jobs/CreateJobForm";
import { JobDetail } from "./features/jobs/JobDetail";
import { JobList } from "./features/jobs/JobList";
import { formatRefreshTime, statusLabel } from "./features/jobs/format";
import {
  displayJobStatus,
  type DisplayJobStatus,
  type Job,
} from "./features/jobs/api";
import {
  DEFAULT_POLL_INTERVAL_MS,
  useJobs,
} from "./features/jobs/useJobs";
import { SchedulesPage } from "./features/schedules/SchedulesPage";
import { WorkersPage } from "./features/workers/WorkersPage";
import { LoginPage } from "./features/session/LoginPage";
import { useSession } from "./features/session/useSession";

interface AppProps {
  pollIntervalMs?: number;
}

const statuses: DisplayJobStatus[] = [
  "queued",
  "scheduled",
  "running",
  "retry_waiting",
  "cancel_requested",
  "succeeded",
  "failed",
  "cancelled",
];

export default function App({ pollIntervalMs = DEFAULT_POLL_INTERVAL_MS }: AppProps) {
  const { session, isLoading, error, signIn, signOut } = useSession();

  if (isLoading) {
    return <main className="login-page" role="status">Loading TaskHarbor…</main>;
  }
  if (session === null) {
    return <LoginPage initialError={error} onLogin={signIn} />;
  }
  return (
    <Dashboard
      pollIntervalMs={pollIntervalMs}
      username={session.user.username}
      onLogout={signOut}
    />
  );
}

interface DashboardProps extends AppProps {
  username: string;
  onLogout: () => Promise<void>;
}

function Dashboard({
  pollIntervalMs = DEFAULT_POLL_INTERVAL_MS,
  username,
  onLogout,
}: DashboardProps) {
  const {
    jobs,
    hasLoaded,
    error,
    isRefreshing,
    lastUpdated,
    refresh,
    upsertJob,
  } = useJobs(pollIntervalMs);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [activeView, setActiveView] = useState<"jobs" | "schedules" | "workers">("jobs");

  useEffect(() => {
    if (selectedId === null && jobs.length > 0) {
      setSelectedId(Math.max(...jobs.map((job) => job.id)));
      return;
    }

    if (selectedId !== null && hasLoaded && !jobs.some((job) => job.id === selectedId)) {
      setSelectedId(jobs.length > 0 ? Math.max(...jobs.map((job) => job.id)) : null);
    }
  }, [hasLoaded, jobs, selectedId]);

  const selectedJob = jobs.find((job) => job.id === selectedId) ?? null;
  const statusCounts = useMemo(
    () =>
      statuses.reduce<Record<DisplayJobStatus, number>>(
        (counts, status) => ({
          ...counts,
          [status]: jobs.filter((job) => displayJobStatus(job) === status).length,
        }),
        {
          queued: 0,
          scheduled: 0,
          running: 0,
          retry_waiting: 0,
          cancel_requested: 0,
          succeeded: 0,
          failed: 0,
          cancelled: 0,
        },
      ),
    [jobs],
  );

  const handleCreated = (job: Job) => {
    upsertJob(job);
    setSelectedId(job.id);
    refresh();
  };

  const handleUpdated = (job: Job) => {
    upsertJob(job);
    refresh();
  };

  const connectionLabel = error
    ? "Updates paused"
    : isRefreshing
      ? "Syncing"
      : hasLoaded
        ? "Live"
        : "Connecting";

  return (
    <div className="app-shell">
      <a className="skip-link" href="#main-content">
        Skip to content
      </a>

      <header className="topbar">
        <a className="brand" href="/" aria-label="TaskHarbor home">
          <span className="brand__mark" aria-hidden="true">
            <LogoIcon />
          </span>
          <span>
            Task<span>Harbor</span>
          </span>
        </a>
        <nav className="topbar__nav" aria-label="Primary navigation">
          <button
            type="button"
            className={activeView === "jobs" ? "topbar__nav-button topbar__nav-button--active" : "topbar__nav-button"}
            onClick={() => setActiveView("jobs")}
          >
            Jobs
          </button>
          <button
            type="button"
            className={activeView === "schedules" ? "topbar__nav-button topbar__nav-button--active" : "topbar__nav-button"}
            onClick={() => setActiveView("schedules")}
          >
            Schedules
          </button>
          <button
            type="button"
            className={activeView === "workers" ? "topbar__nav-button topbar__nav-button--active" : "topbar__nav-button"}
            onClick={() => setActiveView("workers")}
          >
            Workers
          </button>
        </nav>
        <div className="topbar__context">
          <span className="environment-label">{username}</span>
          <button className="topbar__logout" type="button" onClick={() => void onLogout()}>
            Sign out
          </button>
          <span className="environment-label">Local workspace</span>
          <span className={`connection-pill${error ? " connection-pill--error" : ""}`}>
            <span aria-hidden="true" />
            <span aria-live="polite">{connectionLabel}</span>
          </span>
        </div>
      </header>

      {activeView === "jobs" ? (
      <main id="main-content">
        <section className="hero" aria-labelledby="page-title">
          <div>
            <div className="eyebrow">
              <span>Operations</span>
              <span aria-hidden="true">/</span>
              <strong>Jobs</strong>
            </div>
            <h1 id="page-title">Image work, in view.</h1>
            <p>
              Upload source images, follow each item through the worker, and download published
              JPEG results from one quiet control surface.
            </p>
          </div>
          <div className="sync-card">
            <span className="sync-card__icon" aria-hidden="true">
              <PulseIcon />
            </span>
            <span>
              <small>Automatic refresh</small>
              <strong>{formatRefreshTime(lastUpdated)}</strong>
            </span>
          </div>
        </section>

        <section className="status-overview" aria-label="Job status summary">
          {statuses.map((status) => (
            <article className={`status-stat status-stat--${status}`} key={status}>
              <span className="status-stat__label">
                <span aria-hidden="true" />
                {statusLabel(status)}
              </span>
              <strong>{statusCounts[status]}</strong>
            </article>
          ))}
        </section>

        <div className="workspace-grid">
          <CreateJobForm onCreated={handleCreated} />

          <section className="jobs-card" aria-labelledby="jobs-list-title">
            <div className="jobs-card__header">
              <div>
                <div className="section-kicker">Queue activity</div>
                <h2 id="jobs-list-title">Recent jobs</h2>
              </div>
              <div className="jobs-card__actions">
                <span>{jobs.length} total</span>
                <button
                  className="icon-button"
                  type="button"
                  onClick={refresh}
                  aria-label="Refresh jobs now"
                  title="Refresh jobs now"
                >
                  <RefreshIcon active={isRefreshing} />
                </button>
              </div>
            </div>

            {hasLoaded && error && (
              <div className="inline-alert" role="alert">
                <AlertIcon />
                <span>
                  <strong>Live updates paused.</strong> {error}
                </span>
                <button type="button" onClick={refresh}>
                  Retry
                </button>
              </div>
            )}

            {!hasLoaded && !error && <LoadingJobs />}
            {!hasLoaded && error && <JobsError message={error} onRetry={refresh} />}
            {hasLoaded && jobs.length === 0 && !error && <EmptyJobs />}
            {hasLoaded && jobs.length > 0 && (
              <JobList jobs={jobs} selectedId={selectedId} onSelect={setSelectedId} />
            )}
          </section>
        </div>

        <JobDetail job={selectedJob} onUpdated={handleUpdated} onRetried={handleCreated} />
      </main>
      ) : activeView === "schedules" ? (
        <SchedulesPage
          pollIntervalMs={pollIntervalMs}
          onOpenJob={(id) => {
            setSelectedId(id);
            setActiveView("jobs");
            refresh();
          }}
        />
      ) : (
        <WorkersPage pollIntervalMs={pollIntervalMs} />
      )}

      <footer>
        <span>TaskHarbor</span>
        <span>Polling every {Math.round(pollIntervalMs / 100) / 10}s</span>
      </footer>
    </div>
  );
}

function LoadingJobs() {
  return (
    <div className="loading-state" role="status">
      <span className="sr-only">Loading jobs…</span>
      {[0, 1, 2].map((item) => (
        <div className="loading-row" key={item} aria-hidden="true">
          <span />
          <span />
          <span />
        </div>
      ))}
    </div>
  );
}

interface JobsErrorProps {
  message: string;
  onRetry: () => void;
}

function JobsError({ message, onRetry }: JobsErrorProps) {
  return (
    <div className="state-panel state-panel--error" role="alert">
      <span className="state-panel__icon" aria-hidden="true">
        <AlertIcon />
      </span>
      <h3>Jobs could not be loaded</h3>
      <p>{message}</p>
      <button className="button button--secondary" type="button" onClick={onRetry}>
        Try again
      </button>
    </div>
  );
}

function EmptyJobs() {
  return (
    <div className="state-panel">
      <span className="state-panel__icon" aria-hidden="true">
        <HarborIcon />
      </span>
      <h3>No jobs in the harbor yet</h3>
      <p>Upload your first image job with the form. It will appear here immediately.</p>
    </div>
  );
}

function LogoIcon() {
  return (
    <svg viewBox="0 0 32 32">
      <path d="M8 7h16M16 7v16" />
      <path d="M7 17c1.7 4.8 4.7 7 9 7s7.3-2.2 9-7M5 17h6M21 17h6" />
    </svg>
  );
}

function PulseIcon() {
  return (
    <svg viewBox="0 0 24 24">
      <path d="M3 12h4l2-6 4 12 2-6h6" />
    </svg>
  );
}

function RefreshIcon({ active }: { active: boolean }) {
  return (
    <svg className={active ? "spin" : undefined} viewBox="0 0 20 20" aria-hidden="true">
      <path d="M16 7a6.5 6.5 0 1 0 .1 5.7M16 3v4h-4" />
    </svg>
  );
}

function AlertIcon() {
  return (
    <svg viewBox="0 0 20 20" aria-hidden="true">
      <path d="M10 3 2.5 17h15zM10 8v4M10 14.5v.2" />
    </svg>
  );
}

function HarborIcon() {
  return (
    <svg viewBox="0 0 24 24">
      <path d="M4 19h16M6 16h12M8 13h8M10 10h4M12 5v5" />
    </svg>
  );
}
