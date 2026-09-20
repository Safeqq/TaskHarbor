import { afterEach, describe, expect, it, vi } from "vitest";

import { ApiError, cancelJob, createImageJob, listJobs, retryJob, type Job } from "./api";

const job: Job = {
  id: 12,
  name: "Generate catalog previews",
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
      id: 31,
      item_index: 0,
      filename: "source.png",
      media_type: "image/png",
      byte_size: 4,
      width: 8,
      height: 4,
      download_url: null,
    },
  ],
  outputs: [],
  attempts: [],
  available_at: "2026-09-18T08:00:00Z",
  priority: "normal",
  schedule_id: null,
  scheduled_for: null,
  max_attempts: 3,
  retry_of_job_id: null,
  result: null,
  failure_message: null,
  cancel_requested_at: null,
  created_at: "2026-09-18T08:00:00Z",
  started_at: null,
  finished_at: null,
};

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("jobs API client", () => {
  it("reads jobs from the real API response shape", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify({ jobs: [job] }), {
        status: 200,
        headers: { "content-type": "application/json" },
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await expect(listJobs()).resolves.toEqual([job]);
    expect(fetchMock).toHaveBeenCalledWith("/api/v1/jobs", { signal: undefined });
  });

  it("keeps the backend validation message for the form", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(
        new Response(
          JSON.stringify({
            error: {
              code: "validation_error",
              message: "job name must contain at least one non-whitespace character",
              field: "name",
            },
          }),
          { status: 422, headers: { "content-type": "application/json" } },
        ),
      ),
    );

    const request = createImageJob(
      "   ",
      [new File(["png"], "source.png", { type: "image/png" })],
      1600,
      85,
      { priority: "normal", availableAt: null },
    );

    await expect(request).rejects.toMatchObject<Partial<ApiError>>({
      status: 422,
      code: "validation_error",
      field: "name",
    });
  });

  it("creates a multipart request and leaves the boundary to the browser", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify(job), {
        status: 201,
        headers: { "content-type": "application/json" },
      }),
    );
    vi.stubGlobal("fetch", fetchMock);
    const image = new File(["png"], "source.png", { type: "image/png" });

    await expect(
      createImageJob(job.name, [image], 1200, 90, {
        priority: "high",
        availableAt: "2026-09-20T08:00:00.000Z",
      }),
    ).resolves.toEqual(job);

    const [, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(init.headers).toBeUndefined();
    expect(init.body).toBeInstanceOf(FormData);
    const form = init.body as FormData;
    expect(form.get("name")).toBe(job.name);
    expect(form.get("max_width")).toBe("1200");
    expect(form.get("jpeg_quality")).toBe("90");
    expect(form.get("priority")).toBe("high");
    expect(form.get("available_at")).toBe("2026-09-20T08:00:00.000Z");
    expect(form.getAll("images")).toEqual([image]);
  });

  it("sends lifecycle actions to their dedicated endpoints", async () => {
    const fetchMock = vi.fn().mockImplementation(() =>
      Promise.resolve(
        new Response(JSON.stringify(job), {
          status: 200,
          headers: { "content-type": "application/json" },
        }),
      ),
    );
    vi.stubGlobal("fetch", fetchMock);

    await cancelJob(job.id);
    await retryJob(job.id);

    expect(fetchMock).toHaveBeenNthCalledWith(1, `/api/v1/jobs/${job.id}/cancel`, {
      method: "POST",
      signal: undefined,
    });
    expect(fetchMock).toHaveBeenNthCalledWith(2, `/api/v1/jobs/${job.id}/retry`, {
      method: "POST",
      signal: undefined,
    });
  });

  it("turns a network failure into a clear availability error", async () => {
    vi.stubGlobal("fetch", vi.fn().mockRejectedValue(new TypeError("fetch failed")));

    await expect(listJobs()).rejects.toThrow(
      "Could not reach the TaskHarbor API. Make sure the API is running.",
    );
  });

  it("explains an unavailable API behind the development proxy", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response(null, { status: 502 })));

    await expect(listJobs()).rejects.toThrow(
      "The TaskHarbor API is unavailable. Make sure the API and database are running.",
    );
  });
});
