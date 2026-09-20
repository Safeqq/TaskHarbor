import { useEffect, useRef, useState, type FormEvent } from "react";

import {
  errorMessage,
  isAbortError,
  type JobPriority,
} from "../jobs/api";
import { formatBytes, formatDateTime, formatRefreshTime } from "../jobs/format";
import { StatusBadge } from "../jobs/StatusBadge";
import {
  createSchedule,
  updateSchedule,
  type Schedule,
  type ScheduleUpdate,
} from "./api";
import { useSchedules } from "./useSchedules";

const DEFAULT_WIDTH = 1600;
const DEFAULT_QUALITY = 85;

interface SchedulesPageProps {
  pollIntervalMs: number;
  onOpenJob: (id: number) => void;
}

export function SchedulesPage({ pollIntervalMs, onOpenJob }: SchedulesPageProps) {
  const {
    schedules,
    hasLoaded,
    error,
    isRefreshing,
    lastUpdated,
    refresh,
    upsertSchedule,
  } = useSchedules(pollIntervalMs);
  const [selectedId, setSelectedId] = useState<number | null>(null);

  useEffect(() => {
    if (selectedId === null && schedules.length > 0) {
      setSelectedId(Math.max(...schedules.map((schedule) => schedule.id)));
    } else if (
      selectedId !== null &&
      hasLoaded &&
      !schedules.some((schedule) => schedule.id === selectedId)
    ) {
      setSelectedId(schedules.length > 0 ? Math.max(...schedules.map((item) => item.id)) : null);
    }
  }, [hasLoaded, schedules, selectedId]);

  const selected = schedules.find((schedule) => schedule.id === selectedId) ?? null;

  return (
    <main id="main-content">
      <section className="hero schedule-hero" aria-labelledby="schedules-title">
        <div>
          <div className="eyebrow">
            <span>Operations</span>
            <span aria-hidden="true">/</span>
            <strong>Schedules</strong>
          </div>
          <h1 id="schedules-title">Recurring work, anchored.</h1>
          <p>
            Keep image settings and source files in one template, then inspect every created or
            skipped UTC slot.
          </p>
        </div>
        <div className="sync-card">
          <span>
            <small>Automatic refresh</small>
            <strong>{formatRefreshTime(lastUpdated)}</strong>
          </span>
        </div>
      </section>

      <div className="schedule-workspace">
        <CreateScheduleForm
          onCreated={(schedule) => {
            upsertSchedule(schedule);
            setSelectedId(schedule.id);
            refresh();
          }}
        />

        <section className="schedules-card" aria-labelledby="schedule-list-title">
          <div className="jobs-card__header">
            <div>
              <div className="section-kicker">Recurring queue</div>
              <h2 id="schedule-list-title">Schedules</h2>
            </div>
            <div className="jobs-card__actions">
              <span>{schedules.length} total</span>
              <button type="button" className="icon-button" onClick={refresh}>
                {isRefreshing ? "…" : "↻"}
                <span className="sr-only">Refresh schedules</span>
              </button>
            </div>
          </div>

          {error && (
            <div className="inline-alert" role="alert">
              <span>{error}</span>
              <button type="button" onClick={refresh}>Retry</button>
            </div>
          )}
          {!hasLoaded && !error && <p className="schedule-state" role="status">Loading schedules…</p>}
          {hasLoaded && schedules.length === 0 && !error && (
            <p className="schedule-state">No recurring schedules yet.</p>
          )}
          {schedules.length > 0 && (
            <ul className="schedule-list" aria-label="Schedules">
              {[...schedules]
                .sort((left, right) => right.id - left.id)
                .map((schedule) => (
                  <li key={schedule.id}>
                    <button
                      type="button"
                      className={selectedId === schedule.id ? "schedule-row schedule-row--selected" : "schedule-row"}
                      onClick={() => setSelectedId(schedule.id)}
                      aria-pressed={selectedId === schedule.id}
                    >
                      <span>
                        <small>#{schedule.id.toString().padStart(3, "0")}</small>
                        <strong>{schedule.name}</strong>
                        <span>Next {formatDateTime(schedule.next_run_at)}</span>
                      </span>
                      <span className={schedule.enabled ? "schedule-state-pill" : "schedule-state-pill schedule-state-pill--paused"}>
                        {schedule.enabled ? "Enabled" : "Paused"}
                      </span>
                    </button>
                  </li>
                ))}
            </ul>
          )}
        </section>
      </div>

      <ScheduleDetail
        schedule={selected}
        onUpdated={(schedule) => {
          upsertSchedule(schedule);
          refresh();
        }}
        onOpenJob={onOpenJob}
      />
    </main>
  );
}

function CreateScheduleForm({ onCreated }: { onCreated: (schedule: Schedule) => void }) {
  const [name, setName] = useState("");
  const [files, setFiles] = useState<File[]>([]);
  const [anchor, setAnchor] = useState(() => toLocalDateTime(new Date(Date.now() + 60_000)));
  const [intervalMinutes, setIntervalMinutes] = useState(60);
  const [priority, setPriority] = useState<JobPriority>("normal");
  const [maxWidth, setMaxWidth] = useState(DEFAULT_WIDTH);
  const [quality, setQuality] = useState(DEFAULT_QUALITY);
  const [submitting, setSubmitting] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const requestRef = useRef<AbortController | null>(null);
  const fileInputRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => () => requestRef.current?.abort(), []);

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setMessage(null);
    if (name.trim() === "" || files.length === 0) {
      setMessage("Enter a name and select at least one image.");
      return;
    }
    const anchorDate = new Date(anchor);
    if (
      Number.isNaN(anchorDate.getTime()) ||
      !Number.isInteger(intervalMinutes) ||
      intervalMinutes < 1 ||
      !Number.isInteger(maxWidth) ||
      maxWidth < 1 ||
      maxWidth > 8192 ||
      !Number.isInteger(quality) ||
      quality < 1 ||
      quality > 100
    ) {
      setMessage("Check the anchor, interval, width, and JPEG quality values.");
      return;
    }

    const controller = new AbortController();
    requestRef.current = controller;
    setSubmitting(true);
    try {
      const schedule = await createSchedule(
        name,
        files,
        {
          interval_seconds: intervalMinutes * 60,
          anchor_at: anchorDate.toISOString(),
          priority,
          max_width: maxWidth,
          jpeg_quality: quality,
        },
        controller.signal,
      );
      onCreated(schedule);
      setName("");
      setFiles([]);
      if (fileInputRef.current !== null) fileInputRef.current.value = "";
      setMessage(`Schedule #${schedule.id} created.`);
    } catch (error) {
      if (!isAbortError(error)) setMessage(errorMessage(error));
    } finally {
      if (!controller.signal.aborted) setSubmitting(false);
      requestRef.current = null;
    }
  };

  return (
    <section className="create-card schedule-create" aria-labelledby="create-schedule-title">
      <div className="section-kicker">New recurrence</div>
      <h2 id="create-schedule-title">Create schedule</h2>
      <p className="create-card__intro">Intervals stay anchored in UTC and start at one minute.</p>
      <form onSubmit={(event) => void submit(event)}>
        <label htmlFor="schedule-name">Schedule name</label>
        <input id="schedule-name" value={name} onChange={(event) => setName(event.currentTarget.value)} maxLength={100} disabled={submitting} />

        <label className="field-label-spaced" htmlFor="schedule-images">Template images</label>
        <input ref={fileInputRef} id="schedule-images" className="file-input" type="file" accept="image/jpeg,image/png" multiple onChange={(event) => setFiles(Array.from(event.currentTarget.files ?? []))} disabled={submitting} />
        {files.length > 0 && <p className="field-hint">{files.length} files · {formatBytes(files.reduce((sum, file) => sum + file.size, 0))}</p>}

        <div className="scheduling-fields">
          <div>
            <label htmlFor="schedule-anchor">First UTC-anchored slot</label>
            <input id="schedule-anchor" type="datetime-local" step="1" value={anchor} onChange={(event) => setAnchor(event.currentTarget.value)} disabled={submitting} />
          </div>
          <div>
            <label htmlFor="schedule-interval">Interval (minutes)</label>
            <input id="schedule-interval" type="number" min="1" max="525600" value={intervalMinutes} onChange={(event) => setIntervalMinutes(event.currentTarget.valueAsNumber)} disabled={submitting} />
          </div>
          <div>
            <label htmlFor="schedule-priority">Priority</label>
            <select id="schedule-priority" value={priority} onChange={(event) => setPriority(event.currentTarget.value as JobPriority)} disabled={submitting}>
              <option value="high">High</option>
              <option value="normal">Normal</option>
              <option value="low">Low</option>
            </select>
          </div>
        </div>

        <div className="settings-grid">
          <div>
            <label htmlFor="schedule-width">Maximum width</label>
            <input id="schedule-width" type="number" min="1" max="8192" value={maxWidth} onChange={(event) => setMaxWidth(event.currentTarget.valueAsNumber)} disabled={submitting} />
          </div>
          <div>
            <label htmlFor="schedule-quality">JPEG quality</label>
            <input id="schedule-quality" type="number" min="1" max="100" value={quality} onChange={(event) => setQuality(event.currentTarget.valueAsNumber)} disabled={submitting} />
          </div>
        </div>

        {message && <p className="form-message" role="status">{message}</p>}
        <button className="button button--primary" type="submit" disabled={submitting}>
          {submitting ? "Creating…" : "Create recurring schedule"}
        </button>
      </form>
    </section>
  );
}

function ScheduleDetail({
  schedule,
  onUpdated,
  onOpenJob,
}: {
  schedule: Schedule | null;
  onUpdated: (schedule: Schedule) => void;
  onOpenJob: (id: number) => void;
}) {
  const [draft, setDraft] = useState<ScheduleUpdate | null>(null);
  const [anchor, setAnchor] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (schedule === null) {
      setDraft(null);
      return;
    }
    setDraft({
      name: schedule.name,
      enabled: schedule.enabled,
      interval_seconds: schedule.interval_seconds,
      anchor_at: schedule.anchor_at,
      priority: schedule.priority,
      max_width: schedule.image_settings.max_width,
      jpeg_quality: schedule.image_settings.jpeg_quality,
    });
    setAnchor(toLocalDateTime(new Date(schedule.anchor_at)));
    setError(null);
  }, [schedule]);

  if (schedule === null || draft === null) {
    return <section className="detail-card detail-card--empty schedule-detail"><p>Select a schedule to inspect its future settings and occurrence history.</p></section>;
  }

  const save = async (enabled = draft.enabled) => {
    const anchorDate = new Date(anchor);
    if (Number.isNaN(anchorDate.getTime())) {
      setError("Choose a valid anchor time.");
      return;
    }
    setSaving(true);
    setError(null);
    try {
      const updated = await updateSchedule(schedule.id, {
        ...draft,
        enabled,
        anchor_at: anchorDate.toISOString(),
      });
      onUpdated(updated);
    } catch (requestError) {
      setError(errorMessage(requestError));
    } finally {
      setSaving(false);
    }
  };

  return (
    <section className="detail-card schedule-detail" aria-labelledby="schedule-detail-title">
      <div className="detail-card__header">
        <div>
          <div className="section-kicker">Schedule #{schedule.id.toString().padStart(3, "0")}</div>
          <h2 id="schedule-detail-title">{schedule.name}</h2>
        </div>
        <span className={schedule.enabled ? "schedule-state-pill" : "schedule-state-pill schedule-state-pill--paused"}>{schedule.enabled ? "Enabled" : "Paused"}</span>
      </div>

      <div className="schedule-edit-grid">
        <label>Name<input value={draft.name} onChange={(event) => setDraft({ ...draft, name: event.currentTarget.value })} /></label>
        <label>Anchor<input type="datetime-local" step="1" value={anchor} onChange={(event) => setAnchor(event.currentTarget.value)} /></label>
        <label>Interval (minutes)<input type="number" min="1" value={draft.interval_seconds / 60} onChange={(event) => setDraft({ ...draft, interval_seconds: event.currentTarget.valueAsNumber * 60 })} /></label>
        <label>Priority<select value={draft.priority} onChange={(event) => setDraft({ ...draft, priority: event.currentTarget.value as JobPriority })}><option value="high">High</option><option value="normal">Normal</option><option value="low">Low</option></select></label>
        <label>Maximum width<input type="number" min="1" max="8192" value={draft.max_width} onChange={(event) => setDraft({ ...draft, max_width: event.currentTarget.valueAsNumber })} /></label>
        <label>JPEG quality<input type="number" min="1" max="100" value={draft.jpeg_quality} onChange={(event) => setDraft({ ...draft, jpeg_quality: event.currentTarget.valueAsNumber })} /></label>
      </div>
      <div className="schedule-actions">
        <button className="button button--secondary" type="button" disabled={saving} onClick={() => void save()}>{saving ? "Saving…" : "Save future settings"}</button>
        <button className={schedule.enabled ? "button button--danger" : "button button--primary"} type="button" disabled={saving} onClick={() => void save(!schedule.enabled)}>{schedule.enabled ? "Pause schedule" : "Enable schedule"}</button>
      </div>
      {error && <p className="form-message form-message--error" role="alert">{error}</p>}

      <dl className="detail-grid schedule-summary">
        <div><dt>Next slot</dt><dd>{formatDateTime(schedule.next_run_at)}</dd></div>
        <div><dt>Anchor</dt><dd>{formatDateTime(schedule.anchor_at)}</dd></div>
        <div><dt>Template inputs</dt><dd>{schedule.inputs.length}</dd></div>
      </dl>

      <section className="attempt-section" aria-labelledby="occurrence-history-title">
        <div className="attempt-section__heading">
          <div><span className="section-kicker">Scheduler</span><h3 id="occurrence-history-title">Occurrence history</h3></div>
          <span>{schedule.occurrences.length} slots recorded</span>
        </div>
        {schedule.occurrences.length === 0 ? (
          <p className="artifact-empty">No due slot has been handled yet.</p>
        ) : (
          <ol className="occurrence-list">
            {schedule.occurrences.map((occurrence) => (
              <li key={occurrence.id}>
                <div>
                  <strong>{formatDateTime(occurrence.scheduled_for)}</strong>
                  <span>{occurrence.outcome === "created" ? "Job created" : "Skipped: overlap"}</span>
                  {occurrence.coalesced_slots > 0 && <small>{occurrence.coalesced_slots} older slots coalesced</small>}
                  {occurrence.reason && <small>{occurrence.reason}</small>}
                </div>
                {occurrence.job_id !== null && (
                  <button type="button" onClick={() => onOpenJob(occurrence.job_id!)}>
                    Job #{occurrence.job_id}
                    {occurrence.job_status && <StatusBadge status={occurrence.job_status} />}
                  </button>
                )}
              </li>
            ))}
          </ol>
        )}
      </section>
    </section>
  );
}

function toLocalDateTime(date: Date): string {
  const local = new Date(date.getTime() - date.getTimezoneOffset() * 60_000);
  return local.toISOString().slice(0, 19);
}
