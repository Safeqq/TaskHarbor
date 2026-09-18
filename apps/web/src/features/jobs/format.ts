import type { JobStatus } from "./api";

const dateTimeFormatter = new Intl.DateTimeFormat(undefined, {
  dateStyle: "medium",
  timeStyle: "medium",
});

const timeFormatter = new Intl.DateTimeFormat(undefined, {
  hour: "2-digit",
  minute: "2-digit",
  second: "2-digit",
});

export function formatDateTime(value: string | null): string {
  if (value === null) {
    return "Not yet";
  }

  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? "Unknown" : dateTimeFormatter.format(date);
}

export function formatRefreshTime(value: Date | null): string {
  return value === null ? "Waiting for first response" : `Updated ${timeFormatter.format(value)}`;
}

export function formatDuration(value: number | null): string {
  return value === null ? "Not recorded" : `${value.toLocaleString()} ms`;
}

export function statusLabel(status: JobStatus): string {
  return status.charAt(0).toUpperCase() + status.slice(1);
}
