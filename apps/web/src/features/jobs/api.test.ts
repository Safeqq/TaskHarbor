import { afterEach, describe, expect, it, vi } from "vitest";

import { ApiError, createJob, listJobs, type Job } from "./api";

const job: Job = {
  id: 12,
  name: "Generate catalog previews",
  job_type: "demo_delay",
  status: "queued",
  progress: { completed: 0, total: 1 },
  delay_ms: 500,
  result: null,
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

    const request = createJob("   ");

    await expect(request).rejects.toMatchObject<Partial<ApiError>>({
      status: 422,
      code: "validation_error",
      field: "name",
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
