import test from "node:test";
import assert from "node:assert/strict";
import { singleAlertDisplay } from "../src/lib/alert-display.ts";

const screens = [
  { screenId: "left", primary: false },
  { screenId: "main", primary: true },
];
test("a manually selected secondary HUD owns questions instead of the hidden primary HUD", () => {
  const settings = { hudScreenId: "left", hudPrimaryOnly: true };
  assert.equal(singleAlertDisplay("left", screens, settings), true);
  assert.equal(singleAlertDisplay("main", screens, settings), false);
});
test("unplugging a selected screen returns questions to the remaining primary HUD", () => {
  assert.equal(
    singleAlertDisplay("main", screens.slice(1), { hudScreenId: "left" }),
    true,
  );
  assert.equal(
    singleAlertDisplay("left", screens, { hudPrimaryOnly: true }),
    false,
  );
  assert.equal(
    singleAlertDisplay("main", screens, { hudPrimaryOnly: true }),
    true,
  );
  assert.equal(singleAlertDisplay("left", screens, {}), undefined);
});
