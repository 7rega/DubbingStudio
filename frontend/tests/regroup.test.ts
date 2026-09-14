import { test } from "node:test";
import assert from "node:assert/strict";
import { longestMergeChain, mergeKey, releaseBoundaryAudio } from "../src/lib/regroup.ts";

test("whole-chain limit includes overlapping selected pairs, not just pair durations", () => {
  const ab = { a: "a", b: "b", start: 0, end: 12 };
  const bc = { a: "b", b: "c", start: 6, end: 18 };
  assert.equal(longestMergeChain([ab]), 12);
  assert.equal(longestMergeChain([bc, ab]), 18);
  assert.ok(longestMergeChain([bc, ab]) > 15);
  assert.equal(longestMergeChain([{ ...ab, end: 10 }, { ...bc, start: 5, end: 15 }]), 15);
});

test("disconnected selections do not count the silence between chains", () => {
  assert.equal(longestMergeChain([
    { a: "a", b: "b", start: 0, end: 8 },
    { a: "c", b: "d", start: 50, end: 63 },
  ]), 13);
  assert.equal(longestMergeChain([]), 0);
});

test("malformed cycles are rejected instead of enabling apply", () => {
  assert.equal(longestMergeChain([
    { a: "a", b: "b", start: 0, end: 1 },
    { a: "b", b: "a", start: 1, end: 2 },
  ]), Infinity);
});

test("pair keys preserve arbitrary segment IDs", () => {
  assert.notEqual(mergeKey("a|b", "c"), mergeKey("a", "b|c"));
});

test("retired boundary playback detaches late events and releases the audio source", () => {
  const calls: string[] = [];
  const audio = {
    onloadedmetadata: () => calls.push("late play"),
    ontimeupdate: () => {}, onended: () => {}, onerror: () => {},
    pause: () => calls.push("pause"),
    removeAttribute: (name: string) => calls.push(`remove ${name}`),
    load: () => calls.push("load"),
  };
  releaseBoundaryAudio(audio as unknown as HTMLAudioElement);
  assert.equal(audio.onloadedmetadata, null);
  assert.equal(audio.ontimeupdate, null);
  assert.equal(audio.onended, null);
  assert.equal(audio.onerror, null);
  assert.deepEqual(calls, ["pause", "remove src", "load"]);
  releaseBoundaryAudio(null);
});
