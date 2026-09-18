import { useEffect, useRef, useState, type FormEvent } from "react";

import { createJob, errorMessage, isAbortError, type Job } from "./api";

const MAX_JOB_NAME_LENGTH = 100;

interface CreateJobFormProps {
  onCreated: (job: Job) => void;
}

export function CreateJobForm({ onCreated }: CreateJobFormProps) {
  const [name, setName] = useState("");
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const requestRef = useRef<AbortController | null>(null);

  useEffect(
    () => () => {
      requestRef.current?.abort();
    },
    [],
  );

  const handleSubmit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setError(null);
    setNotice(null);

    if (name.trim().length === 0) {
      setError("Enter a name that contains at least one visible character.");
      return;
    }

    if (Array.from(name).length > MAX_JOB_NAME_LENGTH) {
      setError(`Keep the job name within ${MAX_JOB_NAME_LENGTH} characters.`);
      return;
    }

    const controller = new AbortController();
    requestRef.current = controller;
    setIsSubmitting(true);

    try {
      const job = await createJob(name, controller.signal);
      if (controller.signal.aborted) {
        return;
      }

      onCreated(job);
      setName("");
      setNotice(`Job #${job.id} entered the queue.`);
    } catch (requestError) {
      if (!isAbortError(requestError)) {
        setError(errorMessage(requestError));
      }
    } finally {
      if (!controller.signal.aborted) {
        setIsSubmitting(false);
      }
      requestRef.current = null;
    }
  };

  return (
    <section className="create-card" aria-labelledby="create-job-title">
      <div className="section-kicker">New work</div>
      <h2 id="create-job-title">Create a demo job</h2>
      <p className="create-card__intro">
        Send a short background task to the worker and watch it move through the queue.
      </p>

      <form onSubmit={(event) => void handleSubmit(event)} noValidate>
        <label htmlFor="job-name">Job name</label>
        <p className="field-hint" id="job-name-hint">
          Use a clear label so the job is easy to recognize later.
        </p>
        <input
          id="job-name"
          name="name"
          type="text"
          value={name}
          onChange={(event) => setName(event.target.value)}
          maxLength={MAX_JOB_NAME_LENGTH}
          placeholder="Generate product thumbnails"
          aria-describedby={`job-name-hint${error ? " job-name-error" : ""}`}
          aria-invalid={error !== null}
          disabled={isSubmitting}
          autoComplete="off"
          required
        />

        {error && (
          <p className="form-message form-message--error" id="job-name-error" role="alert">
            {error}
          </p>
        )}
        {notice && (
          <p className="form-message form-message--success" role="status">
            {notice}
          </p>
        )}

        <button className="button button--primary" type="submit" disabled={isSubmitting}>
          {isSubmitting ? "Creating…" : "Create job"}
          {!isSubmitting && <ArrowIcon />}
        </button>
      </form>

      <div className="create-card__meta" aria-label="Demo job settings">
        <span>demo_delay</span>
        <span aria-hidden="true">·</span>
        <span>500 ms</span>
      </div>
    </section>
  );
}

function ArrowIcon() {
  return (
    <svg viewBox="0 0 20 20" aria-hidden="true">
      <path d="M4 10h11M11 5l5 5-5 5" />
    </svg>
  );
}
