import test from "node:test";
import assert from "node:assert/strict";
import { newOwnedRequests } from "../src/alerts/request-ownership.ts";

test("a request seen on a hidden display remains new when that display becomes the HUD owner", () => {
  const seen = new Set<string>();
  const requests = [{ id: "question" }];
  assert.deepEqual(newOwnedRequests(requests, seen, false), []);
  assert.deepEqual(newOwnedRequests(requests, seen, true), requests);
  assert.deepEqual(newOwnedRequests(requests, seen, true), []);
});
test("a withdrawn request clears its identity, while a dismissed live question stays dismissed", () => {
  const seen = new Set(["question"]);
  assert.deepEqual(newOwnedRequests([{ id: "question" }], seen, true), []);
  newOwnedRequests([], seen, false);
  assert.deepEqual(newOwnedRequests([{ id: "question" }], seen, true), [
    { id: "question" },
  ]);
});
