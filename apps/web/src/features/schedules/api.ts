import {
  apiRequest,
  type JobPriority,
  type JobStatus,
  type ImageSettings,
} from "../jobs/api";

export interface ScheduleInput {
  id: number;
  item_index: number;
  filename: string;
  media_type: "image/jpeg" | "image/png";
  byte_size: number;
  width: number;
  height: number;
}

export interface ScheduleOccurrence {
  id: number;
  scheduled_for: string;
  outcome: "created" | "skipped_overlap";
  job_id: number | null;
  job_status: JobStatus | null;
  reason: string | null;
  coalesced_slots: number;
  created_at: string;
}

export interface Schedule {
  id: number;
  name: string;
  enabled: boolean;
  interval_seconds: number;
  anchor_at: string;
  next_run_at: string;
  priority: JobPriority;
  image_settings: ImageSettings;
  inputs: ScheduleInput[];
  occurrences: ScheduleOccurrence[];
  created_at: string;
  updated_at: string;
}

interface ListSchedulesResponse {
  schedules: Schedule[];
}

export interface ScheduleUpdate {
  name: string;
  enabled: boolean;
  interval_seconds: number;
  anchor_at: string;
  priority: JobPriority;
  max_width: number;
  jpeg_quality: number;
}

export async function listSchedules(signal?: AbortSignal): Promise<Schedule[]> {
  const response = await apiRequest<ListSchedulesResponse>("/api/v1/schedules", { signal });
  return response.schedules;
}

export async function createSchedule(
  name: string,
  images: File[],
  update: Omit<ScheduleUpdate, "name" | "enabled">,
  signal?: AbortSignal,
): Promise<Schedule> {
  const body = new FormData();
  body.append("name", name);
  body.append("interval_seconds", update.interval_seconds.toString());
  body.append("anchor_at", update.anchor_at);
  body.append("priority", update.priority);
  body.append("max_width", update.max_width.toString());
  body.append("jpeg_quality", update.jpeg_quality.toString());
  images.forEach((image) => body.append("images", image, image.name));

  return apiRequest<Schedule>("/api/v1/schedules", {
    method: "POST",
    body,
    signal,
  });
}

export async function updateSchedule(
  id: number,
  update: ScheduleUpdate,
  signal?: AbortSignal,
): Promise<Schedule> {
  return apiRequest<Schedule>(`/api/v1/schedules/${id}`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(update),
    signal,
  });
}
