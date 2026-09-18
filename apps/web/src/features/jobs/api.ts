export type JobStatus = "queued" | "running" | "succeeded" | "failed";

export interface JobProgress {
  completed: number;
  total: number;
}

export interface JobResult {
  message: string;
  duration_ms: number | null;
}

export interface Job {
  id: number;
  name: string;
  job_type: "demo_delay";
  status: JobStatus;
  progress: JobProgress;
  delay_ms: number;
  result: JobResult | null;
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
  const response = await request<ListJobsResponse>("/api/v1/jobs", { signal });
  return response.jobs;
}

export async function createJob(name: string, signal?: AbortSignal): Promise<Job> {
  return request<Job>("/api/v1/jobs", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ name }),
    signal,
  });
}

async function request<T>(path: string, init: RequestInit): Promise<T> {
  let response: Response;

  try {
    response = await fetch(`${API_BASE_URL}${path}`, init);
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
