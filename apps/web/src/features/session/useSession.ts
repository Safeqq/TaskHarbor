import { useCallback, useEffect, useState } from "react";

import { ApiError, errorMessage, setCsrfToken } from "../jobs/api";
import { getSession, login, logout, type Session } from "./api";

export function useSession() {
  const [session, setSession] = useState<Session | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const controller = new AbortController();
    getSession(controller.signal)
      .then(setSession)
      .catch((requestError: unknown) => {
        if (!(requestError instanceof DOMException && requestError.name === "AbortError")) {
          if (!(requestError instanceof ApiError && requestError.status === 401)) {
            setError(errorMessage(requestError));
          }
          setCsrfToken(null);
        }
      })
      .finally(() => setIsLoading(false));
    return () => controller.abort();
  }, []);

  const signIn = useCallback(async (username: string, password: string) => {
    setError(null);
    const next = await login(username, password);
    setSession(next);
  }, []);

  const signOut = useCallback(async () => {
    try {
      await logout();
    } finally {
      setSession(null);
      setCsrfToken(null);
    }
  }, []);

  return { session, isLoading, error, signIn, signOut };
}
