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

export function formatBytes(value: number): string {
  if (value < 1024) {
    return `${value.toLocaleString()} B`;
  }

  const units = ["KiB", "MiB", "GiB"];
  let size = value / 1024;
  let unitIndex = 0;
  while (size >= 1024 && unitIndex < units.length - 1) {
    size /= 1024;
    unitIndex += 1;
  }

  const digits = size >= 10 ? 1 : 2;
  return `${size.toLocaleString(undefined, { maximumFractionDigits: digits })} ${units[unitIndex]}`;
}

export function statusLabel(status: JobStatus): string {
  const words = status.split("_").join(" ");
  return words.charAt(0).toUpperCase() + words.slice(1);
}
