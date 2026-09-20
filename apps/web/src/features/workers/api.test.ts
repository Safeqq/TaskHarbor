import { afterEach, describe, expect, it, vi } from "vitest";

import { listWorkers, type Worker } from "./api";

const worker: Worker = {
  id: "11111111-1111-4111-8111-111111111111",
  name: "worker-a",
  status: "online",
  concurrency_limit: 2,
  lease_duration_seconds: 15,
  active_attempts: 1,
  started_at: "2026-09-20T01:00:00Z",
  last_heartbeat_at: "2026-09-20T01:01:00Z",
  heartbeat_expires_at: "2026-09-20T01:01:10Z",
  stopped_at: null,
};

afterEach(() => vi.unstubAllGlobals());

describe("workers API client", () => {
  it("reads worker liveness and capacity", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify({ workers: [worker] }), {
        status: 200,
        headers: { "content-type": "application/json" },
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await expect(listWorkers()).resolves.toEqual([worker]);
    expect(fetchMock).toHaveBeenCalledWith("/api/v1/workers", { signal: undefined });
  });
});
