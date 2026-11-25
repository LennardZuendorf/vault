import { expect, test } from "bun:test";
import { optionTypedAt } from "../src/lib/optionAt";

const key = (init: Partial<KeyboardEvent>) =>
  ({
    type: "keydown",
    key: "",
    altKey: false,
    ctrlKey: false,
    metaKey: false,
    isComposing: false,
    ...init,
  }) as KeyboardEvent;

test("an Option-typed @ goes to the agent as @ (PANE-8)", () => {
  expect(optionTypedAt(key({ key: "@", altKey: true }))).toBe(true);
});

test("other keys keep xterm's Option handling", () => {
  expect(optionTypedAt(key({ key: "@" }))).toBe(false); // Shift+2
  expect(optionTypedAt(key({ key: "∫", altKey: true }))).toBe(false); // Option+B
  expect(optionTypedAt(key({ key: "@", altKey: true, metaKey: true }))).toBe(false);
  expect(optionTypedAt(key({ key: "@", altKey: true, ctrlKey: true }))).toBe(false);
  expect(optionTypedAt(key({ key: "@", altKey: true, type: "keyup" }))).toBe(false);
  expect(optionTypedAt(key({ key: "@", altKey: true, isComposing: true }))).toBe(false);
});
