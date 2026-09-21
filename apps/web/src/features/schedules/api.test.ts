import { afterEach, describe, expect, it, vi } from "vitest";

import {
  createSchedule,
  listSchedules,
  updateSchedule,
  type Schedule,
} from "./api";
import { setCsrfToken } from "../jobs/api";

const schedule: Schedule = {
  id: 8,
  name: "Catalog refresh",
  enabled: true,
  interval_seconds: 3600,
  anchor_at: "2026-09-20T08:00:00Z",
  next_run_at: "2026-09-20T08:00:00Z",
  priority: "high",
  image_settings: {
    max_width: 1200,
    jpeg_quality: 90,
    output_media_type: "image/jpeg",
    transparency_background: "white",
  },
  inputs: [
    {
      id: 4,
      item_index: 0,
      filename: "catalog.png",
      media_type: "image/png",
      byte_size: 4,
      width: 8,
      height: 4,
    },
  ],
  occurrences: [],
  created_at: "2026-09-19T08:00:00Z",
  updated_at: "2026-09-19T08:00:00Z",
};

afterEach(() => {
  setCsrfToken(null);
  vi.unstubAllGlobals();
});

describe("schedules API client", () => {
  it("lists, creates, and updates schedules with the documented shapes", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse({ schedules: [schedule] }))
      .mockResolvedValueOnce(jsonResponse(schedule, 201))
      .mockResolvedValueOnce(jsonResponse({ ...schedule, enabled: false }));
    vi.stubGlobal("fetch", fetchMock);
    setCsrfToken("schedule-csrf");

    await expect(listSchedules()).resolves.toEqual([schedule]);
    const image = new File(["png"], "catalog.png", { type: "image/png" });
    await expect(
      createSchedule(
        schedule.name,
        [image],
        {
          interval_seconds: schedule.interval_seconds,
          anchor_at: schedule.anchor_at,
          priority: schedule.priority,
          max_width: schedule.image_settings.max_width,
          jpeg_quality: schedule.image_settings.jpeg_quality,
        },
      ),
    ).resolves.toEqual(schedule);

    const createInit = fetchMock.mock.calls[1]?.[1] as RequestInit;
    const form = createInit.body as FormData;
    expect(createInit.credentials).toBe("same-origin");
    expect((createInit.headers as Headers).has("content-type")).toBe(false);
    expect(form.get("anchor_at")).toBe(schedule.anchor_at);
    expect(form.get("interval_seconds")).toBe("3600");
    expect(form.get("priority")).toBe("high");
    expect(form.getAll("images")).toEqual([image]);

    await expect(
      updateSchedule(schedule.id, {
        name: schedule.name,
        enabled: false,
        interval_seconds: schedule.interval_seconds,
        anchor_at: schedule.anchor_at,
        priority: schedule.priority,
        max_width: schedule.image_settings.max_width,
        jpeg_quality: schedule.image_settings.jpeg_quality,
      }),
    ).resolves.toMatchObject({ enabled: false });
    expect(fetchMock.mock.calls[2]?.[0]).toBe(`/api/v1/schedules/${schedule.id}`);
    expect(fetchMock.mock.calls[2]?.[1]).toMatchObject({
      method: "PUT",
      credentials: "same-origin",
    });
    expect(
      ((fetchMock.mock.calls[2]?.[1] as RequestInit).headers as Headers).get("content-type"),
    ).toBe("application/json");
  });
});

function jsonResponse(value: unknown, status = 200): Response {
  return new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": "application/json" },
  });
}
