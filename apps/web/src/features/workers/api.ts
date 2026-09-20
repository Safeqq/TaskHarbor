import { apiRequest } from "../jobs/api";

export type WorkerStatus = "online" | "offline" | "stopped";

export interface Worker {
  id: string;
  name: string;
  status: WorkerStatus;
  concurrency_limit: number;
  lease_duration_seconds: number;
  active_attempts: number;
  started_at: string;
  last_heartbeat_at: string;
  heartbeat_expires_at: string;
  stopped_at: string | null;
}

interface ListWorkersResponse {
  workers: Worker[];
}

export async function listWorkers(signal?: AbortSignal): Promise<Worker[]> {
  const response = await apiRequest<ListWorkersResponse>("/api/v1/workers", { signal });
  return response.workers;
}
