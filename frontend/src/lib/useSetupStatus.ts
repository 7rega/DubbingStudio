import { useCallback, useEffect, useRef, useSyncExternalStore } from "react";
import { api, type DownloadJob, type SetupStatus } from "./api";
import { useStore } from "../store";

let sharedStatus: SetupStatus | null = null;
let sharedError: string | null = null;
const listeners = new Set<() => void>();

function notify() {
  listeners.forEach((l) => l());
}

function updateStatus(s: SetupStatus) {
  sharedStatus = s;
  sharedError = null;
  notify();
}

function updateError(e: unknown) {
  sharedError = e instanceof Error ? e.message : String(e);
  notify();
}

let activeInterval: ReturnType<typeof setInterval> | null = null;

function checkPolling() {
  const isDownloading = sharedStatus?.active?.status === "downloading";
  if (isDownloading && !activeInterval) {
    activeInterval = setInterval(() => {
      api.setupStatus().then(updateStatus, updateError);
    }, 1000);
  } else if (!isDownloading && activeInterval) {
    clearInterval(activeInterval);
    activeInterval = null;
  }
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function useSetupStatus(onSettled?: (job: DownloadJob, status: SetupStatus) => void) {
  const status = useSyncExternalStore(subscribe, () => sharedStatus, () => null);
  const error = useSyncExternalStore(subscribe, () => sharedError, () => null);
  const wasRunning = useRef(false);
  const settled = useRef(onSettled);
  useEffect(() => { settled.current = onSettled; }, [onSettled]);

  const refresh = useCallback(async () => {
    try {
      const s = await api.setupStatus();
      updateStatus(s);
      checkPolling();
      return s;
    } catch (e) {
      updateError(e);
      throw e;
    }
  }, []);

  const adopt = useCallback((s: SetupStatus) => {
    updateStatus(s);
    checkPolling();
  }, []);

  useEffect(() => {
    const running = status?.active?.status === "downloading";
    if (running && status?.active) {
      const a = status.active;
      useStore.getState().setProgress("download", a.phase || "download", a.total > 0 ? Math.round((a.downloaded / a.total) * 100) : null);
    } else if (wasRunning.current) {
      useStore.getState().setProgress("", "", null);
    }
    if (wasRunning.current && !running && status?.active) {
      settled.current?.(status.active, status);
    }
    wasRunning.current = running;
    checkPolling();
  }, [status]);

  useEffect(() => {
    if (!sharedStatus) {
      refresh().catch(() => {});
    }
  }, [refresh]);

  const downloading = status?.active?.status === "downloading";

  return { status, error, refresh, adopt, downloading };
}
