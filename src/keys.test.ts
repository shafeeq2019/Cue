import { describe, expect, it } from "vitest";
import { displayAccelerator, eventToAccelerator } from "./keys";

const ev = (code: string, m: Partial<{ ctrlKey: boolean; altKey: boolean; shiftKey: boolean; metaKey: boolean }> = {}) => ({
  code,
  ctrlKey: false,
  altKey: false,
  shiftKey: false,
  metaKey: false,
  ...m,
});

describe("hotkeys", () => {
  it("records accelerators", () => {
    expect(eventToAccelerator(ev("Space", { altKey: true }))).toBe("Alt+Space");
    expect(eventToAccelerator(ev("KeyK", { ctrlKey: true, shiftKey: true }))).toBe("Control+Shift+K");
    expect(eventToAccelerator(ev("Digit1", { metaKey: true }))).toBe("Super+1");
    expect(eventToAccelerator(ev("ShiftLeft", { shiftKey: true }))).toBeNull();
  });

  it("displays per platform", () => {
    expect(displayAccelerator("Alt+Space", "macos")).toBe("⌥Space");
    expect(displayAccelerator("Control+Space", "windows")).toBe("Ctrl+Space");
    expect(displayAccelerator("Super+Shift+K", "windows")).toBe("Win+Shift+K");
  });
});
