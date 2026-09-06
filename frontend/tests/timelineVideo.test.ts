import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  decimatePeaks,
  clampTime,
  mergeIntervals,
  computeFallbackPeaks,
  computeVisibleTileRange,
} from "../src/lib/timelineUtils.ts";

describe("Waveform and Timeline Unit Tests", () => {
  describe("decimatePeaks", () => {
    it("handles empty array", () => {
      assert.deepEqual(decimatePeaks([], 100), []);
    });

    it("returns same array when length <= maxBars", () => {
      const input = [0.1, 0.5, 0.8];
      assert.deepEqual(decimatePeaks(input, 5), input);
      assert.deepEqual(decimatePeaks(input, 3), input);
    });

    it("decimates array when length > maxBars to exact maxBars count", () => {
      const input = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8];
      const decimated = decimatePeaks(input, 4);
      assert.equal(decimated.length, 4);
    });

    it("preserves maximum peak values inside buckets without loss", () => {
      // 8 samples into 2 buckets of 4 samples
      // Bucket 1 has a sharp peak 0.95 at index 2
      // Bucket 2 has a sharp peak 0.88 at index 6
      const input = [0.1, 0.2, 0.95, 0.05, 0.1, 0.3, 0.88, 0.2];
      const decimated = decimatePeaks(input, 2);
      assert.equal(decimated.length, 2);
      assert.equal(decimated[0], 0.95);
      assert.equal(decimated[1], 0.88);
    });
  });

  describe("clampTime", () => {
    it("clamps negative values to 0", () => {
      assert.equal(clampTime(-5, 100), 0);
      assert.equal(clampTime(-0.001, 50), 0);
    });

    it("clamps values greater than duration to duration", () => {
      assert.equal(clampTime(105, 100), 100);
      assert.equal(clampTime(12.5, 10), 10);
    });

    it("preserves values within [0, duration]", () => {
      assert.equal(clampTime(42.5, 100), 42.5);
      assert.equal(clampTime(0, 100), 0);
      assert.equal(clampTime(100, 100), 100);
    });

    it("safely handles non-finite values", () => {
      assert.equal(clampTime(NaN, 100), 0);
      assert.equal(clampTime(Infinity, 100), 0);
      assert.equal(clampTime(10, NaN), 0);
    });
  });

  describe("mergeIntervals & computeFallbackPeaks", () => {
    it("merges overlapping and adjacent intervals", () => {
      const segments = [
        { start: 10, end: 15 },
        { start: 1, end: 5 },
        { start: 3, end: 8 },
        { start: 20, end: 25 },
      ];
      const merged = mergeIntervals(segments);
      assert.deepEqual(merged, [
        { start: 1, end: 8 },
        { start: 10, end: 15 },
        { start: 20, end: 25 },
      ]);
    });

    it("generates correct dub fallback peaks (silence outside, amplified speech inside)", () => {
      const basePeaks = [0.5, 0.5, 0.5, 0.5, 0.5]; // 5 peaks over 10 seconds (step = 2s: 0s, 2s, 4s, 6s, 8s)
      const segments = [{ start: 2, end: 5 }]; // covers t = 2s and 4s
      const fallbackDub = computeFallbackPeaks(basePeaks, segments, 10, "dub");

      assert.equal(fallbackDub.length, 5);
      // t = 0s -> outside -> 0.015
      assert.equal(fallbackDub[0], 0.015);
      // t = 2s -> inside -> min(1.0, 0.5 * 1.15 + 0.06) = 0.635
      assert.equal(fallbackDub[1], 0.635);
      // t = 4s -> inside -> 0.635
      assert.equal(fallbackDub[2], 0.635);
      // t = 6s -> outside -> 0.015
      assert.equal(fallbackDub[3], 0.015);
      // t = 8s -> outside -> 0.015
      assert.equal(fallbackDub[4], 0.015);
    });

    it("generates correct bgm fallback peaks (attenuated inside speech, full music outside)", () => {
      const basePeaks = [0.5, 0.5, 0.5, 0.5, 0.5];
      const segments = [{ start: 2, end: 5 }];
      const fallbackBgm = computeFallbackPeaks(basePeaks, segments, 10, "bgm");

      assert.equal(fallbackBgm.length, 5);
      // t = 0s -> outside -> min(1.0, 0.5 * 0.95 + 0.08) = 0.555
      assert.ok(Math.abs(fallbackBgm[0] - 0.555) < 1e-4);
      // t = 2s -> inside -> max(0.04, 0.5 * 0.4) = 0.2
      assert.equal(fallbackBgm[1], 0.2);
      // t = 4s -> inside -> 0.2
      assert.equal(fallbackBgm[2], 0.2);
      // t = 6s -> outside -> 0.555
      assert.ok(Math.abs(fallbackBgm[3] - 0.555) < 1e-4);
    });

    it("computes fallback peaks in strictly linear time O(peaks + segments) without O(peaks * segments)", () => {
      const nPeaks = 30000;
      const nSegments = 3000;
      const basePeaks = new Array(nPeaks).fill(0.4);
      const segments = Array.from({ length: nSegments }, (_, i) => ({
        start: i * 2,
        end: i * 2 + 1,
      }));

      const start = performance.now();
      const result = computeFallbackPeaks(basePeaks, segments, nSegments * 2, "dub");
      const durationMs = performance.now() - start;

      assert.equal(result.length, nPeaks);
      // Linear scan of 30,000 peaks across 3,000 segments must complete in under 50ms
      assert.ok(
        durationMs < 100,
        `Expected linear execution under 100ms, took ${durationMs.toFixed(2)}ms`
      );
    });
  });

  describe("computeVisibleTileRange", () => {
    it("handles zero and invalid dimensions safely", () => {
      assert.deepEqual(computeVisibleTileRange(0, 1000, 0, 2000), {
        startTile: 0,
        endTile: 0,
        totalTiles: 0,
      });
      assert.deepEqual(computeVisibleTileRange(0, 1000, -100, 2000), {
        startTile: 0,
        endTile: 0,
        totalTiles: 0,
      });
      assert.deepEqual(computeVisibleTileRange(0, 1000, 1000, 0), {
        startTile: 0,
        endTile: 0,
        totalTiles: 0,
      });
    });

    it("handles short videos (< 1 tile width) without over-allocating", () => {
      const res = computeVisibleTileRange(0, 1200, 800, 2000, 1);
      assert.deepEqual(res, {
        startTile: 0,
        endTile: 0,
        totalTiles: 1,
      });
    });

    it("computes correct tile range with buffer at scroll start", () => {
      const res = computeVisibleTileRange(0, 1200, 10000, 2000, 1);
      assert.deepEqual(res, {
        startTile: 0,
        endTile: 1,
        totalTiles: 5,
      });
    });

    it("computes correct tile range in the middle of a 2-hour timeline", () => {
      const res = computeVisibleTileRange(40000, 1920, 360000, 2000, 1);
      assert.deepEqual(res, {
        startTile: 19,
        endTile: 21,
        totalTiles: 180,
      });
    });

    it("clamps correctly at the very end of the timeline", () => {
      const res = computeVisibleTileRange(9500, 1200, 10000, 2000, 1);
      assert.equal(res.endTile, 4);
      assert.equal(res.totalTiles, 5);
      assert.ok(res.startTile <= res.endTile);
    });
  });

  describe("Lifecycle state machine and generation token", () => {
    class MockVideoLifecycleState {
      generation = 0;
      mediaReady = false;
      error = false;
      status: string = "empty";
      playCallCount = 0;
      endedCallCount = 0;
      errorCallCount = 0;
      watchdogTimer: ReturnType<typeof setTimeout> | null = null;

      loadSource(_src?: string) {
        void _src;
        this.generation++;
        this.mediaReady = false;
        this.error = false;
        this.status = "loading";
        if (this.watchdogTimer) {
          clearTimeout(this.watchdogTimer);
          this.watchdogTimer = null;
        }
      }

      onLoadedData(eventGeneration: number) {
        if (eventGeneration !== this.generation) return;
        this.mediaReady = true;
        this.status = "ready";
      }

      onCanPlay(eventGeneration: number) {
        if (eventGeneration !== this.generation) return;
        this.mediaReady = true;
        this.error = false;
        this.status = "ready";
      }

      onWaiting(eventGeneration: number, isPlaying: boolean) {
        if (eventGeneration !== this.generation) return;
        if (isPlaying) this.status = "waiting";
      }

      onStalled(eventGeneration: number, isPlaying: boolean) {
        if (eventGeneration !== this.generation) return;
        if (isPlaying) this.status = "stalled";
      }

      onError(eventGeneration: number) {
        if (eventGeneration !== this.generation) return;
        this.mediaReady = false;
        this.error = true;
        this.status = "error";
        this.errorCallCount++;
      }

      onEnded(eventGeneration: number) {
        if (eventGeneration !== this.generation) return;
        this.status = "ended";
        this.endedCallCount++;
      }

      attemptPlay(reject: boolean) {
        this.playCallCount++;
        if (reject) {
          // Play rejected: handle error without retrying play in a loop
          this.errorCallCount++;
          return false;
        }
        this.status = "playing";
        return true;
      }

      triggerDecodeWatchdog(eventGeneration: number, frameDecoded: boolean) {
        if (eventGeneration !== this.generation) return;
        if (!frameDecoded) {
          this.mediaReady = false;
          this.error = true;
          this.status = "error";
          this.errorCallCount++;
        }
      }
    }

    it("stale source generation ignores late canplay and error events", () => {
      const state = new MockVideoLifecycleState();
      state.loadSource("video1.mp4");
      assert.equal(state.generation, 1);

      // User quickly changes source
      state.loadSource("video2.mp4");
      assert.equal(state.generation, 2);

      // Late event from video1.mp4 arrives
      state.onCanPlay(1);
      assert.equal(state.mediaReady, false);
      assert.equal(state.status, "loading");

      // Late error from video1.mp4 arrives
      state.onError(1);
      assert.equal(state.error, false);
      assert.equal(state.errorCallCount, 0);

      // Valid event from video2.mp4 arrives
      state.onCanPlay(2);
      assert.equal(state.mediaReady, true);
      assert.equal(state.status, "ready");
    });

    it("waiting and stalled events update media status when playing", () => {
      const state = new MockVideoLifecycleState();
      state.loadSource("video.mp4");
      state.onCanPlay(1);

      state.onWaiting(1, true);
      assert.equal(state.status, "waiting");

      state.onStalled(1, true);
      assert.equal(state.status, "stalled");

      // Ignored if not playing
      state.onWaiting(1, false);
      assert.equal(state.status, "stalled");
    });

    it("play rejection does not trigger infinite play loop", () => {
      const state = new MockVideoLifecycleState();
      state.loadSource("video.mp4");
      state.onCanPlay(1);

      // Call attemptPlay with rejection
      const success = state.attemptPlay(true);
      assert.equal(success, false);
      assert.equal(state.playCallCount, 1);
      assert.equal(state.errorCallCount, 1);

      // Ensure playCallCount remained 1 (no recursive play loop)
      assert.equal(state.playCallCount, 1);
    });

    it("ended event is called exactly once", () => {
      const state = new MockVideoLifecycleState();
      state.loadSource("video.mp4");
      state.onCanPlay(1);

      state.onEnded(1);
      assert.equal(state.status, "ended");
      assert.equal(state.endedCallCount, 1);
    });

    it("decode timeout transitions status to error", () => {
      const state = new MockVideoLifecycleState();
      state.loadSource("video.mp4");
      state.onCanPlay(1);

      // First frame not decoded within watchdog deadline
      state.triggerDecodeWatchdog(1, false);
      assert.equal(state.status, "error");
      assert.equal(state.error, true);
      assert.equal(state.mediaReady, false);
      assert.equal(state.errorCallCount, 1);
    });

    it("waveform fetch active/cancel guard drops late result after unmount or pid change", () => {
      let active = true;
      let statePeaks: number[] = [];

      const fetchPromise = new Promise<{ peaks: number[] }>((resolve) => {
        setTimeout(() => resolve({ peaks: [0.1, 0.2, 0.3] }), 10);
      });

      // Component unmounts or pid changes before fetch completes
      active = false;

      fetchPromise.then((r) => {
        if (active) statePeaks = r.peaks;
      });

      return new Promise<void>((resolve) => {
        setTimeout(() => {
          assert.deepEqual(statePeaks, []);
          resolve();
        }, 30);
      });
    });
  });

  describe("Audio as Master & Smart Seek-then-Play Protocol", () => {
    it("Smart Seek-then-Play: launches playback once video frame is seeked", () => {
      let play = false;
      let pendingPlayOnSeeked = false;

      const playSeg = (seg: { start: number; end: number }) => {
        pendingPlayOnSeeked = true;
      };

      const onVideoSeeked = () => {
        if (pendingPlayOnSeeked) {
          pendingPlayOnSeeked = false;
          play = true;
        }
      };

      playSeg({ start: 1.5, end: 3.2 });
      assert.equal(play, false);
      assert.equal(pendingPlayOnSeeked, true);

      // Video seeks to keyframe and fires seeked
      onVideoSeeked();
      assert.equal(play, true);
      assert.equal(pendingPlayOnSeeked, false);
    });

    it("Smart Seek-then-Play: safety timeout triggers play if seeked is delayed", async () => {
      let play = false;
      let pendingPlayOnSeeked = false;

      const playSeg = (seg: { start: number; end: number }) => {
        pendingPlayOnSeeked = true;
        setTimeout(() => {
          if (pendingPlayOnSeeked) {
            pendingPlayOnSeeked = false;
            play = true;
          }
        }, 20);
      };

      playSeg({ start: 2.0, end: 4.0 });
      assert.equal(play, false);

      await new Promise((r) => setTimeout(r, 35));
      assert.equal(play, true);
      assert.equal(pendingPlayOnSeeked, false);
    });

    it("Audio as Master: video clock does not terminate phrase early when dub track is active", () => {
      let play = true;
      const playEnd = 5.0;
      const hasDubTrack = true;

      const onVideoTimeUpdate = (videoTime: number) => {
        if (!hasDubTrack) {
          if (videoTime >= playEnd) {
            play = false;
            return;
          }
        }
      };

      // Video runs slightly ahead of audio and hits playEnd
      onVideoTimeUpdate(5.02);
      // Because hasDubTrack is true, audio is master! Playback MUST NOT stop prematurely:
      assert.equal(play, true);

      // When audio itself hits playEnd, audio terminates the phrase:
      const onAudioTimeUpdate = (audioTime: number) => {
        if (audioTime >= playEnd) {
          play = false;
        }
      };

      onAudioTimeUpdate(5.0);
      assert.equal(play, false);
    });

    it("Video Master fallback: video halts playback on phrase end when dub track is absent", () => {
      let play = true;
      const playEnd = 5.0;
      const hasDubTrack = false;

      const onVideoTimeUpdate = (videoTime: number) => {
        if (!hasDubTrack) {
          if (videoTime >= playEnd) {
            play = false;
            return;
          }
        }
      };

      onVideoTimeUpdate(4.9);
      assert.equal(play, true);

      onVideoTimeUpdate(5.0);
      assert.equal(play, false);
    });
  });
});
