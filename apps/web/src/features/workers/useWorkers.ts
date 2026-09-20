import { useCallback, useEffect, useState } from "react";

import { errorMessage, isAbortError } from "../jobs/api";
import { listWorkers, type Worker } from "./api";

interface WorkersState {
  workers: Worker[];
  hasLoaded: boolean;
  error: string | null;
  isRefreshing: boolean;
  lastUpdated: Date | null;
}

const initialState: WorkersState = {
  workers: [],
  hasLoaded: false,
  error: null,
  isRefreshing: false,
  lastUpdated: null,
};

export function useWorkers(pollIntervalMs: number) {
  const [state, setState] = useState(initialState);
  const [refreshGeneration, setRefreshGeneration] = useState(0);

  useEffect(() => {
    let active = true;
    let timer: number | undefined;
    let controller: AbortController | undefined;

    const poll = async () => {
      controller = new AbortController();
      setState((current) => ({ ...current, isRefreshing: current.hasLoaded }));
      try {
        const workers = await listWorkers(controller.signal);
        if (!active) return;
        setState({
          workers,
          hasLoaded: true,
          error: null,
          isRefreshing: false,
          lastUpdated: new Date(),
        });
      } catch (error) {
        if (!active || isAbortError(error)) return;
        setState((current) => ({
          ...current,
          error: errorMessage(error),
          isRefreshing: false,
        }));
      } finally {
        controller = undefined;
        if (active) timer = window.setTimeout(() => void poll(), pollIntervalMs);
      }
    };

    void poll();
    return () => {
      active = false;
      if (timer !== undefined) window.clearTimeout(timer);
      controller?.abort();
    };
  }, [pollIntervalMs, refreshGeneration]);

  const refresh = useCallback(() => {
    setState((current) => ({ ...current, error: null }));
    setRefreshGeneration((generation) => generation + 1);
  }, []);

  return { ...state, refresh };
}
