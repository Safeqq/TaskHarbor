import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import App from "./App";
import { ApiError, type Job } from "./features/jobs/api";

const apiMocks = vi.hoisted(() => ({
  listJobs: vi.fn(),
  createImageJob: vi.fn(),
}));

vi.mock("./features/jobs/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./features/jobs/api")>();
  return {
    ...actual,
    listJobs: apiMocks.listJobs,
    createImageJob: apiMocks.createImageJob,
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
  result: null,
  failure_message: null,
  created_at: "2026-09-18T09:00:00Z",
  started_at: null,
  finished_at: null,
};

beforeEach(() => {
  apiMocks.listJobs.mockReset();
  apiMocks.createImageJob.mockReset();
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
      expect.any(AbortSignal),
    );
    expect(
      await screen.findByRole("heading", { name: queuedJob.name }),
    ).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("Job #41 entered the queue");
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
