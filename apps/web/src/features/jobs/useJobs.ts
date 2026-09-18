import { useCallback, useEffect, useState } from "react";

import { errorMessage, isAbortError, listJobs, type Job } from "./api";

export const DEFAULT_POLL_INTERVAL_MS = 2_000;

interface JobsState {
  jobs: Job[];
  hasLoaded: boolean;
  error: string | null;
  isRefreshing: boolean;
  lastUpdated: Date | null;
}

const initialState: JobsState = {
  jobs: [],
  hasLoaded: false,
  error: null,
  isRefreshing: false,
  lastUpdated: null,
};

export function useJobs(pollIntervalMs = DEFAULT_POLL_INTERVAL_MS) {
  const [state, setState] = useState<JobsState>(initialState);
  const [refreshGeneration, setRefreshGeneration] = useState(0);

  useEffect(() => {
    let active = true;
    let timer: number | undefined;
    let controller: AbortController | undefined;

    const poll = async () => {
      controller = new AbortController();
      setState((current) => ({
        ...current,
        isRefreshing: current.hasLoaded,
      }));

      try {
        const jobs = await listJobs(controller.signal);
        if (!active) {
          return;
        }

        setState({
          jobs,
          hasLoaded: true,
          error: null,
          isRefreshing: false,
          lastUpdated: new Date(),
        });
      } catch (error) {
        if (!active || isAbortError(error)) {
          return;
        }

        setState((current) => ({
          ...current,
          error: errorMessage(error),
          isRefreshing: false,
        }));
      } finally {
        controller = undefined;
        if (active) {
          timer = window.setTimeout(() => void poll(), pollIntervalMs);
        }
      }
    };

    void poll();

    return () => {
      active = false;
      if (timer !== undefined) {
        window.clearTimeout(timer);
      }
      controller?.abort();
    };
  }, [pollIntervalMs, refreshGeneration]);

  const refresh = useCallback(() => {
    setState((current) => ({ ...current, error: null }));
    setRefreshGeneration((generation) => generation + 1);
  }, []);

  const upsertJob = useCallback((job: Job) => {
    setState((current) => {
      const existingIndex = current.jobs.findIndex((item) => item.id === job.id);
      const jobs = [...current.jobs];

      if (existingIndex === -1) {
        jobs.push(job);
      } else {
        jobs[existingIndex] = job;
      }

      return { ...current, jobs, hasLoaded: true, error: null };
    });
  }, []);

  return { ...state, refresh, upsertJob };
}
