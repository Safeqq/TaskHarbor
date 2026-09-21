export type JobStatus =
  | "queued"
  | "running"
  | "retry_waiting"
  | "cancel_requested"
  | "succeeded"
  | "failed"
  | "cancelled";
export type JobType = "demo_delay" | "image_resize";
export type AttemptStatus = "running" | "succeeded" | "failed" | "cancelled";
export type JobPriority = "high" | "normal" | "low";
export type DisplayJobStatus = JobStatus | "scheduled";

export interface JobProgress {
  completed: number;
  total: number;
}

export interface JobResult {
  message: string;
  duration_ms: number | null;
}

export interface ImageSettings {
  max_width: number;
  jpeg_quality: number;
  output_media_type: "image/jpeg";
  transparency_background: "white";
}

export interface Artifact {
  id: number;
  item_index: number;
  filename: string;
  media_type: "image/jpeg" | "image/png";
  byte_size: number;
  width: number;
  height: number;
  download_url: string | null;
}

export interface JobAttempt {
  id: number;
  number: number;
  status: AttemptStatus;
  progress: JobProgress;
  started_at: string;
  finished_at: string | null;
  duration_ms: number | null;
  error_kind: "transient" | "permanent" | "cancelled" | null;
  error_message: string | null;
  worker: { id: string; name: string } | null;
  lease_expires_at: string | null;
}

export interface Job {
  id: number;
  name: string;
  job_type: JobType;
  status: JobStatus;
  progress: JobProgress;
  delay_ms: number | null;
  image_settings: ImageSettings | null;
  inputs: Artifact[];
  outputs: Artifact[];
  attempts: JobAttempt[];
  available_at: string;
  priority: JobPriority;
  schedule_id: number | null;
  scheduled_for: string | null;
  max_attempts: number;
  retry_of_job_id: number | null;
  result: JobResult | null;
  failure_message: string | null;
  cancel_requested_at: string | null;
  created_at: string;
  started_at: string | null;
  finished_at: string | null;
}

interface ListJobsResponse {
  jobs: Job[];
}

interface ErrorEnvelope {
  error?: {
    code?: string;
    message?: string;
    field?: string;
  };
}

const API_BASE_URL = (import.meta.env.VITE_API_BASE_URL ?? "").replace(/\/$/, "");
let csrfToken: string | null = null;

export function setCsrfToken(value: string | null): void {
  csrfToken = value;
}

export class ApiError extends Error {
  constructor(
    message: string,
    public readonly status: number | null = null,
    public readonly code: string | null = null,
    public readonly field: string | null = null,
  ) {
    super(message);
    this.name = "ApiError";
  }
}

export function isAbortError(error: unknown): boolean {
  return error instanceof DOMException && error.name === "AbortError";
}

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : "An unexpected error occurred.";
}

export async function listJobs(signal?: AbortSignal): Promise<Job[]> {
  const response = await apiRequest<ListJobsResponse>("/api/v1/jobs", { signal });
  return response.jobs;
}

export async function createImageJob(
  name: string,
  images: File[],
  maxWidth: number,
  jpegQuality: number,
  options: {
    priority: JobPriority;
    availableAt: string | null;
  },
  signal?: AbortSignal,
): Promise<Job> {
  const body = new FormData();
  body.append("name", name);
  body.append("max_width", maxWidth.toString());
  body.append("jpeg_quality", jpegQuality.toString());
  body.append("priority", options.priority);
  if (options.availableAt !== null) {
    body.append("available_at", options.availableAt);
  }
  images.forEach((image) => body.append("images", image, image.name));

  return apiRequest<Job>("/api/v1/jobs", {
    method: "POST",
    body,
    signal,
  });
}

export function displayJobStatus(job: Job, now = Date.now()): DisplayJobStatus {
  const availableAt = Date.parse(job.available_at);
  return job.status === "queued" && Number.isFinite(availableAt) && availableAt > now
    ? "scheduled"
    : job.status;
}

export async function cancelJob(id: number, signal?: AbortSignal): Promise<Job> {
  return apiRequest<Job>(`/api/v1/jobs/${id}/cancel`, {
    method: "POST",
    signal,
  });
}

export async function retryJob(id: number, signal?: AbortSignal): Promise<Job> {
  return apiRequest<Job>(`/api/v1/jobs/${id}/retry`, {
    method: "POST",
    signal,
  });
}

export function artifactDownloadUrl(path: string): string {
  return `${API_BASE_URL}${path}`;
}

export async function apiRequest<T>(path: string, init: RequestInit): Promise<T> {
  let response: Response;
  const headers = new Headers(init.headers);
  const method = (init.method ?? "GET").toUpperCase();
  if (!["GET", "HEAD", "OPTIONS"].includes(method) && csrfToken !== null) {
    headers.set("x-csrf-token", csrfToken);
  }

  try {
    response = await fetch(`${API_BASE_URL}${path}`, {
      ...init,
      credentials: "same-origin",
      headers,
    });
  } catch (error) {
    if (isAbortError(error)) {
      throw error;
    }

    throw new ApiError(
      "Could not reach the TaskHarbor API. Make sure the API is running.",
    );
  }

  if (!response.ok) {
    const payload = await readErrorEnvelope(response);
    const fallbackMessage =
      response.status >= 500
        ? "The TaskHarbor API is unavailable. Make sure the API and database are running."
        : `The API returned HTTP ${response.status}.`;
    throw new ApiError(
      payload.error?.message ?? fallbackMessage,
      response.status,
      payload.error?.code ?? null,
      payload.error?.field ?? null,
    );
  }

  if (response.status === 204) {
    return undefined as T;
  }

  try {
    return (await response.json()) as T;
  } catch {
    throw new ApiError(
      "The TaskHarbor API returned an unreadable response.",
      response.status,
    );
  }
}

async function readErrorEnvelope(response: Response): Promise<ErrorEnvelope> {
  try {
    return (await response.json()) as ErrorEnvelope;
  } catch {
    return {};
  }
}
