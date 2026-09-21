import { apiRequest, setCsrfToken } from "../jobs/api";

export interface SessionUser {
  id: number;
  username: string;
}

export interface Session {
  user: SessionUser;
  expires_at: string;
  csrf_token: string;
}

export async function getSession(signal?: AbortSignal): Promise<Session> {
  const session = await apiRequest<Session>("/api/v1/session", { signal });
  setCsrfToken(session.csrf_token);
  return session;
}

export async function login(username: string, password: string): Promise<Session> {
  const session = await apiRequest<Session>("/api/v1/session/login", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ username, password }),
  });
  setCsrfToken(session.csrf_token);
  return session;
}

export async function logout(): Promise<void> {
  await apiRequest<void>("/api/v1/session", { method: "DELETE" });
  setCsrfToken(null);
}
