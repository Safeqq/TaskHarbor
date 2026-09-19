import { useEffect, useRef, useState, type ChangeEvent, type FormEvent } from "react";

import { createImageJob, errorMessage, isAbortError, type Job } from "./api";
import { formatBytes } from "./format";

const MAX_JOB_NAME_LENGTH = 100;
const MAX_FILES = 10;
const MAX_FILE_BYTES = 5 * 1024 * 1024;
const MAX_TOTAL_BYTES = 25 * 1024 * 1024;
const MAX_OUTPUT_WIDTH = 8192;
const DEFAULT_OUTPUT_WIDTH = 1600;
const DEFAULT_JPEG_QUALITY = 85;

interface CreateJobFormProps {
  onCreated: (job: Job) => void;
}

export function CreateJobForm({ onCreated }: CreateJobFormProps) {
  const [name, setName] = useState("");
  const [files, setFiles] = useState<File[]>([]);
  const [maxWidth, setMaxWidth] = useState(DEFAULT_OUTPUT_WIDTH);
  const [jpegQuality, setJpegQuality] = useState(DEFAULT_JPEG_QUALITY);
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const requestRef = useRef<AbortController | null>(null);
  const fileInputRef = useRef<HTMLInputElement | null>(null);

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

    const fileError = validateFiles(files);
    if (fileError !== null) {
      setError(fileError);
      return;
    }

    if (!Number.isInteger(maxWidth) || maxWidth < 1 || maxWidth > MAX_OUTPUT_WIDTH) {
      setError(`Set the maximum width between 1 and ${MAX_OUTPUT_WIDTH.toLocaleString()} pixels.`);
      return;
    }

    if (!Number.isInteger(jpegQuality) || jpegQuality < 1 || jpegQuality > 100) {
      setError("Set JPEG quality between 1 and 100.");
      return;
    }

    const controller = new AbortController();
    requestRef.current = controller;
    setIsSubmitting(true);

    try {
      const job = await createImageJob(
        name,
        files,
        maxWidth,
        jpegQuality,
        controller.signal,
      );
      if (controller.signal.aborted) {
        return;
      }

      onCreated(job);
      setName("");
      setFiles([]);
      if (fileInputRef.current !== null) {
        fileInputRef.current.value = "";
      }
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

  const handleFiles = (event: ChangeEvent<HTMLInputElement>) => {
    const selected = Array.from(event.currentTarget.files ?? []);
    setFiles(selected);
    setError(validateFiles(selected));
    setNotice(null);
  };

  const totalBytes = files.reduce((total, file) => total + file.size, 0);

  return (
    <section className="create-card" aria-labelledby="create-job-title">
      <div className="section-kicker">New work</div>
      <h2 id="create-job-title">Resize images</h2>
      <p className="create-card__intro">
        Upload JPEG or PNG images. The worker keeps their aspect ratio and publishes JPEG outputs.
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
          aria-describedby={`job-name-hint${error ? " create-job-error" : ""}`}
          aria-invalid={error !== null}
          disabled={isSubmitting}
          autoComplete="off"
          required
        />

        <label className="field-label-spaced" htmlFor="job-images">
          Source images
        </label>
        <p className="field-hint" id="job-images-hint">
          Up to 10 files, 5 MiB each and 25 MiB combined. File contents are verified by the API.
        </p>
        <input
          ref={fileInputRef}
          id="job-images"
          className="file-input"
          name="images"
          type="file"
          accept="image/jpeg,image/png"
          multiple
          onChange={handleFiles}
          aria-describedby={`job-images-hint${error ? " create-job-error" : ""}`}
          aria-invalid={error !== null}
          disabled={isSubmitting}
          required
        />

        {files.length > 0 && (
          <div className="selected-files" aria-live="polite">
            <div className="selected-files__summary">
              <strong>
                {files.length} {files.length === 1 ? "file" : "files"}
              </strong>
              <span>{formatBytes(totalBytes)} total</span>
            </div>
            <ul>
              {files.map((file, index) => (
                <li key={`${file.name}-${file.size}-${index}`}>
                  <span title={file.name}>{file.name}</span>
                  <span>{formatBytes(file.size)}</span>
                </li>
              ))}
            </ul>
          </div>
        )}

        <div className="settings-grid">
          <div>
            <label htmlFor="max-width">Maximum width</label>
            <input
              id="max-width"
              name="max_width"
              type="number"
              min="1"
              max={MAX_OUTPUT_WIDTH}
              value={maxWidth}
              onChange={(event) => setMaxWidth(event.currentTarget.valueAsNumber)}
              disabled={isSubmitting}
              required
            />
            <span>pixels</span>
          </div>
          <div>
            <label htmlFor="jpeg-quality">JPEG quality</label>
            <input
              id="jpeg-quality"
              name="jpeg_quality"
              type="number"
              min="1"
              max="100"
              value={jpegQuality}
              onChange={(event) => setJpegQuality(event.currentTarget.valueAsNumber)}
              disabled={isSubmitting}
              required
            />
            <span>1–100</span>
          </div>
        </div>

        {error && (
          <p className="form-message form-message--error" id="create-job-error" role="alert">
            {error}
          </p>
        )}
        {notice && (
          <p className="form-message form-message--success" role="status">
            {notice}
          </p>
        )}

        <button className="button button--primary" type="submit" disabled={isSubmitting}>
          {isSubmitting ? "Uploading…" : "Upload and create"}
          {!isSubmitting && <ArrowIcon />}
        </button>
      </form>

      <div className="create-card__meta" aria-label="Image job behavior">
        <span>image_resize</span>
        <span aria-hidden="true">·</span>
        <span>no upscaling</span>
        <span aria-hidden="true">·</span>
        <span>white transparency</span>
      </div>
    </section>
  );
}

function validateFiles(files: File[]): string | null {
  if (files.length === 0) {
    return "Select at least one JPEG or PNG image.";
  }

  if (files.length > MAX_FILES) {
    return `Select no more than ${MAX_FILES} images for one job.`;
  }

  const oversized = files.find((file) => file.size > MAX_FILE_BYTES);
  if (oversized !== undefined) {
    return `${oversized.name} is larger than 5 MiB.`;
  }

  const totalBytes = files.reduce((total, file) => total + file.size, 0);
  if (totalBytes > MAX_TOTAL_BYTES) {
    return "Keep the combined image size at or below 25 MiB.";
  }

  return null;
}

function ArrowIcon() {
  return (
    <svg viewBox="0 0 20 20" aria-hidden="true">
      <path d="M4 10h11M11 5l5 5-5 5" />
    </svg>
  );
}
