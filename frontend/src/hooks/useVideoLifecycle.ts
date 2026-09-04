import { useEffect, useRef, useState, useCallback } from "react";
import { clampTime } from "../lib/timelineUtils";

export interface SeekRequest {
  id: number;
  time: number;
}

export type MediaStatus =
  | "empty"
  | "loading"
  | "ready"
  | "playing"
  | "waiting"
  | "stalled"
  | "error"
  | "ended";

export interface UseVideoLifecycleOptions {
  src: string;
  playing: boolean;
  requestedTime?: number;
  seekRequest?: SeekRequest | null;
  muted?: boolean;
  volume?: number;
  onClock?: (currentTime: number) => void;
  onSeeked?: (currentTime: number) => void;
  onEnded?: () => void;
  onError?: () => void;
}

export interface UseVideoLifecycleResult {
  videoRef: React.RefObject<HTMLVideoElement | null>;
  status: MediaStatus;
  error: boolean;
  mediaReady: boolean;
  seek: (time: number) => void;
  retry: () => void;
}

export function useVideoLifecycle({
  src,
  playing,
  requestedTime = 0,
  seekRequest = null,
  muted = false,
  volume = 1,
  onClock,
  onSeeked,
  onEnded,
  onError,
}: UseVideoLifecycleOptions): UseVideoLifecycleResult {
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const [status, setStatus] = useState<MediaStatus>("empty");
  const [mediaReady, setMediaReady] = useState(false);
  const [error, setError] = useState(false);

  const generationRef = useRef(0);
  const onClockRef = useRef(onClock);
  const onSeekedRef = useRef(onSeeked);
  const onEndedRef = useRef(onEnded);
  const onErrorRef = useRef(onError);
  const desiredTimeRef = useRef(requestedTime);
  const lastHandledSeekId = useRef<number>(-1);
  const watchdogTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const frameCallbackIdRef = useRef<number | null>(null);
  const isPlayingIntentRef = useRef(playing);
  const isMountedRef = useRef(true);

  useEffect(() => {
    onClockRef.current = onClock;
    onSeekedRef.current = onSeeked;
    onEndedRef.current = onEnded;
    onErrorRef.current = onError;
    desiredTimeRef.current = requestedTime;
    isPlayingIntentRef.current = playing;
  });

  const clearWatchdog = useCallback(() => {
    if (watchdogTimerRef.current !== null) {
      clearTimeout(watchdogTimerRef.current);
      watchdogTimerRef.current = null;
    }
    const v = videoRef.current;
    if (
      frameCallbackIdRef.current !== null &&
      v &&
      typeof v.cancelVideoFrameCallback === "function"
    ) {
      try {
        v.cancelVideoFrameCallback(frameCallbackIdRef.current);
      } catch {
        /* ignore */
      }
      frameCallbackIdRef.current = null;
    }
  }, []);

  const triggerError = useCallback(
    (generation: number) => {
      if (!isMountedRef.current || generation !== generationRef.current) return;
      clearWatchdog();
      setMediaReady(false);
      setError(true);
      setStatus("error");
      const v = videoRef.current;
      if (v) {
        try {
          v.pause();
        } catch {
          /* ignore */
        }
      }
      onErrorRef.current?.();
    },
    [clearWatchdog]
  );

  const startWatchdog = useCallback(
    (generation: number) => {
      clearWatchdog();
      const v = videoRef.current;
      if (!v) return;

      let frameDecoded = false;
      if (typeof v.requestVideoFrameCallback === "function") {
        try {
          frameCallbackIdRef.current = v.requestVideoFrameCallback(() => {
            frameDecoded = true;
            clearWatchdog();
          });
        } catch {
          /* ignore */
        }
      }

      watchdogTimerRef.current = setTimeout(() => {
        if (generation !== generationRef.current) return;
        if (!frameDecoded) {
          // If currentTime hasn't moved and readyState < 2 after 8s of playing, signal error
          if (v.readyState < 2 || (v.paused && isPlayingIntentRef.current)) {
            triggerError(generation);
          }
        }
      }, 8000);
    },
    [clearWatchdog, triggerError]
  );

  const loadMedia = useCallback(
    (generation: number) => {
      const v = videoRef.current;
      if (!v || !src || generation !== generationRef.current) {
        setStatus("empty");
        setMediaReady(false);
        setError(false);
        return;
      }

      clearWatchdog();
      setStatus("loading");
      setMediaReady(false);
      setError(false);

      try {
        v.pause();
      } catch {
        /* ignore */
      }

      v.src = src;
      v.load();
    },
    [src, clearWatchdog]
  );

  // Setup media source and event listeners
  useEffect(() => {
    isMountedRef.current = true;
    const v = videoRef.current;
    if (!v) return;

    const generation = ++generationRef.current;
    loadMedia(generation);

    const onLoadedMetadata = () => {
      if (generation !== generationRef.current) return;
      setStatus("loading");
      const target = desiredTimeRef.current;
      if (Number.isFinite(target) && target > 0) {
        const clamped = clampTime(target, v.duration || target);
        if (Math.abs(v.currentTime - clamped) > 0.03) {
          try {
            v.currentTime = clamped;
          } catch {
            /* ignore */
          }
        }
      }
    };

    const onLoadedData = () => {
      if (generation !== generationRef.current) return;
      setMediaReady(true);
      setStatus((prev) => (prev !== "playing" ? "ready" : prev));
    };

    const onCanPlay = () => {
      if (generation !== generationRef.current) return;
      setMediaReady(true);
      setError(false);
      setStatus((prev) => (prev !== "playing" ? "ready" : prev));
    };

    const onPlaying = () => {
      if (generation !== generationRef.current) return;
      setMediaReady(true);
      setError(false);
      setStatus("playing");
      startWatchdog(generation);
    };

    const onTimeUpdate = () => {
      if (generation !== generationRef.current) return;
      onClockRef.current?.(v.currentTime);
    };

    const onSeeking = () => {
      if (generation !== generationRef.current) return;
    };

    const onSeeked = () => {
      if (generation !== generationRef.current) return;
      onSeekedRef.current?.(v.currentTime);
    };

    const onWaiting = () => {
      if (generation !== generationRef.current) return;
      if (isPlayingIntentRef.current) setStatus("waiting");
    };

    const onStalled = () => {
      if (generation !== generationRef.current) return;
      if (isPlayingIntentRef.current) setStatus("stalled");
    };

    const onAbort = () => {
      if (generation !== generationRef.current) return;
    };

    const onEmptied = () => {
      if (generation !== generationRef.current) return;
    };

    const onErrorEvent = () => {
      if (generation !== generationRef.current) return;
      triggerError(generation);
    };

    const onEndedEvent = () => {
      if (generation !== generationRef.current) return;
      clearWatchdog();
      setStatus("ended");
      onEndedRef.current?.();
    };

    v.addEventListener("loadedmetadata", onLoadedMetadata);
    v.addEventListener("loadeddata", onLoadedData);
    v.addEventListener("canplay", onCanPlay);
    v.addEventListener("playing", onPlaying);
    v.addEventListener("timeupdate", onTimeUpdate);
    v.addEventListener("seeking", onSeeking);
    v.addEventListener("seeked", onSeeked);
    v.addEventListener("waiting", onWaiting);
    v.addEventListener("stalled", onStalled);
    v.addEventListener("abort", onAbort);
    v.addEventListener("emptied", onEmptied);
    v.addEventListener("error", onErrorEvent);
    v.addEventListener("ended", onEndedEvent);

    return () => {
      isMountedRef.current = false;
      clearWatchdog();
      v.removeEventListener("loadedmetadata", onLoadedMetadata);
      v.removeEventListener("loadeddata", onLoadedData);
      v.removeEventListener("canplay", onCanPlay);
      v.removeEventListener("playing", onPlaying);
      v.removeEventListener("timeupdate", onTimeUpdate);
      v.removeEventListener("seeking", onSeeking);
      v.removeEventListener("seeked", onSeeked);
      v.removeEventListener("waiting", onWaiting);
      v.removeEventListener("stalled", onStalled);
      v.removeEventListener("abort", onAbort);
      v.removeEventListener("emptied", onEmptied);
      v.removeEventListener("error", onErrorEvent);
      v.removeEventListener("ended", onEndedEvent);
    };
  }, [src, loadMedia, startWatchdog, triggerError, clearWatchdog]);

  // Volume and mute synchronization
  useEffect(() => {
    const v = videoRef.current;
    if (!v) return;
    v.muted = muted;
    v.volume = volume;
  }, [muted, volume]);

  // Play / Pause synchronization
  useEffect(() => {
    const v = videoRef.current;
    if (!v || error) return;

    if (!playing) {
      clearWatchdog();
      if (!v.paused) {
        try {
          v.pause();
        } catch {
          /* ignore */
        }
      }
      return;
    }

    // Attempt playback once; handle rejection safely without infinite loop
    const playPromise = v.play();
    if (playPromise !== undefined) {
      playPromise.catch((err: unknown) => {
        const e = err as { name?: string } | null;
        if (e?.name !== "AbortError") {
          console.warn("Video playback rejected:", err);
          // Do not loop play calls. Notify onEnded/onError to halt playback intent if unrecoverable
          onErrorRef.current?.();
        }
      });
    }
  }, [playing, error, clearWatchdog]);

  // High-precision smooth clock loop during playback (60fps / 120fps instead of 4Hz onTimeUpdate)
  useEffect(() => {
    const v = videoRef.current;
    if (!v || !playing || error) return;

    let animId: number | null = null;
    let rvfcId: number | null = null;
    let running = true;

    const tick = () => {
      if (!running) return;
      if (v && !v.paused) {
        onClockRef.current?.(v.currentTime);
      }
      if (typeof v.requestVideoFrameCallback === "function") {
        rvfcId = v.requestVideoFrameCallback(() => {
          tick();
        });
      } else {
        animId = requestAnimationFrame(tick);
      }
    };

    tick();

    return () => {
      running = false;
      if (rvfcId !== null && typeof v.cancelVideoFrameCallback === "function") {
        try {
          v.cancelVideoFrameCallback(rvfcId);
        } catch {
          /* ignore */
        }
      }
      if (animId !== null) {
        cancelAnimationFrame(animId);
      }
    };
  }, [playing, error]);

  // Explicit seek request handling
  useEffect(() => {
    if (!seekRequest || seekRequest.id === lastHandledSeekId.current) return;
    lastHandledSeekId.current = seekRequest.id;

    const v = videoRef.current;
    if (!v || v.readyState < 1) return;

    const target = clampTime(seekRequest.time, v.duration || seekRequest.time);
    try {
      v.currentTime = target;
    } catch {
      /* ignore */
    }
  }, [seekRequest]);

  // Requested time (scrubbing while paused)
  useEffect(() => {
    const v = videoRef.current;
    if (!v || v.readyState < 1 || playing) return;

    const target = clampTime(requestedTime, v.duration || requestedTime);
    if (Math.abs(v.currentTime - target) > 0.03 && !v.seeking) {
      try {
        v.currentTime = target;
      } catch {
        /* ignore */
      }
    }
  }, [requestedTime, playing]);

  const seek = useCallback((time: number) => {
    const v = videoRef.current;
    if (!v) return;
    const target = clampTime(time, v.duration || time);
    try {
      v.currentTime = target;
    } catch {
      /* ignore */
    }
  }, []);

  const retry = useCallback(() => {
    const generation = ++generationRef.current;
    loadMedia(generation);
  }, [loadMedia]);

  return {
    videoRef,
    status,
    error,
    mediaReady,
    seek,
    retry,
  };
}
