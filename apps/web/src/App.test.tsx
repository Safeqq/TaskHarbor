import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import App from "./App";
import { ApiError, type Job } from "./features/jobs/api";
import type { Schedule } from "./features/schedules/api";

const apiMocks = vi.hoisted(() => ({
  listJobs: vi.fn(),
  createImageJob: vi.fn(),
  cancelJob: vi.fn(),
  retryJob: vi.fn(),
}));

const scheduleMocks = vi.hoisted(() => ({
  listSchedules: vi.fn(),
  createSchedule: vi.fn(),
  updateSchedule: vi.fn(),
}));

vi.mock("./features/jobs/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./features/jobs/api")>();
  return {
    ...actual,
    listJobs: apiMocks.listJobs,
    createImageJob: apiMocks.createImageJob,
    cancelJob: apiMocks.cancelJob,
    retryJob: apiMocks.retryJob,
  };
});

vi.mock("./features/schedules/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./features/schedules/api")>();
  return {
    ...actual,
    listSchedules: scheduleMocks.listSchedules,
    createSchedule: scheduleMocks.createSchedule,
    updateSchedule: scheduleMocks.updateSchedule,
  };
});

const queuedJob: Job = {
  id: 41,
  name: "Design thumbnails",
  job_type: "image_resize",
  status: "queued",
  progress: { completed: 0, total: 1 },
  delay_ms: null,
  image_settings: {
    max_width: 1600,
    jpeg_quality: 85,
    output_media_type: "image/jpeg",
    transparency_background: "white",
  },
  inputs: [
    {
      id: 91,
      item_index: 0,
      filename: "product.png",
      media_type: "image/png",
      byte_size: 4,
      width: 800,
      height: 600,
      download_url: null,
    },
  ],
  outputs: [],
  attempts: [],
  available_at: "2026-09-18T09:00:00Z",
  priority: "normal",
  schedule_id: null,
  scheduled_for: null,
  max_attempts: 3,
  retry_of_job_id: null,
  result: null,
  failure_message: null,
  cancel_requested_at: null,
  created_at: "2026-09-18T09:00:00Z",
  started_at: null,
  finished_at: null,
};

const recurringSchedule: Schedule = {
  id: 7,
  name: "Nightly catalog",
  enabled: true,
  interval_seconds: 3600,
  anchor_at: "2026-09-20T01:00:00Z",
  next_run_at: "2026-09-20T02:00:00Z",
  priority: "normal",
  image_settings: {
    max_width: 1600,
    jpeg_quality: 85,
    output_media_type: "image/jpeg",
    transparency_background: "white",
  },
  inputs: [],
  occurrences: [
    {
      id: 3,
      scheduled_for: "2026-09-20T01:00:00Z",
      outcome: "created",
      job_id: 41,
      job_status: "queued",
      reason: null,
      coalesced_slots: 0,
      created_at: "2026-09-20T01:00:00Z",
    },
  ],
  created_at: "2026-09-19T08:00:00Z",
  updated_at: "2026-09-19T08:00:00Z",
};

beforeEach(() => {
  apiMocks.listJobs.mockReset();
  apiMocks.createImageJob.mockReset();
  apiMocks.cancelJob.mockReset();
  apiMocks.retryJob.mockReset();
  scheduleMocks.listSchedules.mockReset();
  scheduleMocks.createSchedule.mockReset();
  scheduleMocks.updateSchedule.mockReset();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("Jobs dashboard", () => {
  it("distinguishes the initial loading state from an empty queue", async () => {
    const firstRequest = deferred<Job[]>();
    apiMocks.listJobs.mockReturnValue(firstRequest.promise);

    render(<App />);

    expect(screen.getByRole("status")).toHaveTextContent("Loading jobs");
    expect(screen.queryByText("No jobs in the harbor yet")).not.toBeInTheDocument();

    await act(async () => {
      firstRequest.resolve([]);
      await firstRequest.promise;
    });

    expect(await screen.findByText("No jobs in the harbor yet")).toBeInTheDocument();
  });

  it("shows API downtime as an error instead of an empty queue", async () => {
    apiMocks.listJobs.mockRejectedValue(
      new ApiError("Could not reach the TaskHarbor API. Make sure the API is running."),
    );

    render(<App />);

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Jobs could not be loaded");
    expect(alert).toHaveTextContent("Make sure the API is running");
    expect(screen.queryByText("No jobs in the harbor yet")).not.toBeInTheDocument();
  });

  it("creates a job through the labelled form and selects its detail", async () => {
    apiMocks.listJobs.mockResolvedValueOnce([]).mockResolvedValue([queuedJob]);
    apiMocks.createImageJob.mockResolvedValue(queuedJob);
    const image = new File(["png"], "product.png", { type: "image/png" });

    render(<App />);

    await screen.findByText("No jobs in the harbor yet");
    fireEvent.change(screen.getByLabelText("Job name"), {
      target: { value: queuedJob.name },
    });
    fireEvent.change(screen.getByLabelText("Source images"), {
      target: { files: [image] },
    });
    fireEvent.click(screen.getByRole("button", { name: "Upload and create" }));

    expect(apiMocks.createImageJob).toHaveBeenCalledWith(
      queuedJob.name,
      [image],
      1600,
      85,
      { priority: "normal", availableAt: null },
      expect.any(AbortSignal),
    );
    expect(
      await screen.findByRole("heading", { name: queuedJob.name }),
    ).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("Job #41 entered the queue");
  });

  it("requests cancellation and updates the selected job", async () => {
    const cancelledJob: Job = {
      ...queuedJob,
      status: "cancelled",
      cancel_requested_at: "2026-09-18T09:01:00Z",
      finished_at: "2026-09-18T09:01:00Z",
    };
    apiMocks.listJobs.mockResolvedValueOnce([queuedJob]).mockResolvedValue([cancelledJob]);
    apiMocks.cancelJob.mockResolvedValue(cancelledJob);

    render(<App />);

    await screen.findByRole("heading", { name: queuedJob.name });
    fireEvent.click(screen.getByRole("button", { name: "Cancel job" }));

    expect(apiMocks.cancelJob).toHaveBeenCalledWith(queuedJob.id, expect.any(AbortSignal));
    expect(await screen.findByRole("button", { name: "Retry as new job" })).toBeInTheDocument();
  });

  it("labels future queued work as scheduled", async () => {
    apiMocks.listJobs.mockResolvedValue([
      { ...queuedJob, available_at: "2099-01-01T00:00:00Z" },
    ]);

    render(<App />);

    expect(await screen.findByText("Scheduled")).toBeInTheDocument();
  });

  it("opens the schedules page and shows occurrence history", async () => {
    apiMocks.listJobs.mockResolvedValue([queuedJob]);
    scheduleMocks.listSchedules.mockResolvedValue([recurringSchedule]);

    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Schedules" }));

    expect(await screen.findByRole("heading", { name: "Recurring work, anchored." })).toBeInTheDocument();
    expect(await screen.findByRole("heading", { name: recurringSchedule.name })).toBeInTheDocument();
    expect(screen.getByText("Occurrence history")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Job #41/ })).toBeInTheDocument();
  });

  it("waits for each poll to finish and aborts the active request on unmount", async () => {
    vi.useFakeTimers();
    const firstRequest = deferred<Job[]>();
    const secondRequest = deferred<Job[]>();
    apiMocks.listJobs
      .mockReturnValueOnce(firstRequest.promise)
      .mockReturnValueOnce(secondRequest.promise);

    const { unmount } = render(<App pollIntervalMs={2_000} />);
    expect(apiMocks.listJobs).toHaveBeenCalledTimes(1);

    await act(async () => {
      vi.advanceTimersByTime(10_000);
      await Promise.resolve();
    });
    expect(apiMocks.listJobs).toHaveBeenCalledTimes(1);

    await act(async () => {
      firstRequest.resolve([]);
      await firstRequest.promise;
    });
    await act(async () => {
      vi.advanceTimersByTime(2_000);
      await Promise.resolve();
    });

    expect(apiMocks.listJobs).toHaveBeenCalledTimes(2);
    const activeSignal = apiMocks.listJobs.mock.calls[1]?.[0] as AbortSignal;
    expect(activeSignal.aborted).toBe(false);

    unmount();
    expect(activeSignal.aborted).toBe(true);

    await act(async () => {
      vi.advanceTimersByTime(10_000);
      await Promise.resolve();
    });
    expect(apiMocks.listJobs).toHaveBeenCalledTimes(2);
  });
});

interface Deferred<T> {
  promise: Promise<T>;
  resolve: (value: T) => void;
}

function deferred<T>(): Deferred<T> {
  let resolve: ((value: T) => void) | undefined;
  const promise = new Promise<T>((promiseResolve) => {
    resolve = promiseResolve;
  });

  if (resolve === undefined) {
    throw new Error("deferred promise was not initialized");
  }

  return { promise, resolve };
}
