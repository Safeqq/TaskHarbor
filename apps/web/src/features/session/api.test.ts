import { afterEach, describe, expect, it, vi } from "vitest";

import { setCsrfToken } from "../jobs/api";
import { getSession, login, logout, type Session } from "./api";

const session: Session = {
  user: { id: 1, username: "owner" },
  expires_at: "2026-09-21T00:00:00Z",
  csrf_token: "csrf-from-server",
};

afterEach(() => {
  setCsrfToken(null);
  vi.unstubAllGlobals();
});

describe("session API client", () => {
  it("restores a session and sends its CSRF token on logout", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse(session))
      .mockResolvedValueOnce(new Response(null, { status: 204 }));
    vi.stubGlobal("fetch", fetchMock);

    await expect(getSession()).resolves.toEqual(session);
    await expect(logout()).resolves.toBeUndefined();

    expect(fetchMock.mock.calls[0]?.[0]).toBe("/api/v1/session");
    const logoutInit = fetchMock.mock.calls[1]?.[1] as RequestInit;
    expect(logoutInit).toMatchObject({ method: "DELETE", credentials: "same-origin" });
    expect((logoutInit.headers as Headers).get("x-csrf-token")).toBe(session.csrf_token);
  });

  it("submits credentials without exposing them in the URL", async () => {
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse(session, 201));
    vi.stubGlobal("fetch", fetchMock);

    await expect(login("owner", "private-password")).resolves.toEqual(session);

    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(url).toBe("/api/v1/session/login");
    expect(init.credentials).toBe("same-origin");
    expect(JSON.parse(init.body as string)).toEqual({
      username: "owner",
      password: "private-password",
    });
  });
});

function jsonResponse(value: unknown, status = 200): Response {
  return new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": "application/json" },
  });
}
