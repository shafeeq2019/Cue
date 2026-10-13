// Keyboard helpers: hotkey recording and platform-aware display.

export type Platform = "macos" | "windows" | "linux";

const MODIFIER_CODES = new Set([
  "ShiftLeft",
  "ShiftRight",
  "ControlLeft",
  "ControlRight",
  "AltLeft",
  "AltRight",
  "MetaLeft",
  "MetaRight",
  "OSLeft",
  "OSRight",
]);

interface KeyLike {
  code: string;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
}

/** Convert a keydown into a global-shortcut accelerator ("Alt+Space"). */
export function eventToAccelerator(e: KeyLike): string | null {
  if (MODIFIER_CODES.has(e.code) || !e.code) return null;
  const parts: string[] = [];
  if (e.ctrlKey) parts.push("Control");
  if (e.altKey) parts.push("Alt");
  if (e.shiftKey) parts.push("Shift");
  if (e.metaKey) parts.push("Super");
  let key = e.code;
  if (key.startsWith("Key")) key = key.slice(3);
  else if (key.startsWith("Digit")) key = key.slice(5);
  parts.push(key);
  return parts.join("+");
}

/** Display an accelerator for the current platform. */
export function displayAccelerator(acc: string, platform: Platform): string {
  const mac = platform === "macos";
  const map: Record<string, string> = mac
    ? { control: "⌃", ctrl: "⌃", alt: "⌥", option: "⌥", shift: "⇧", super: "⌘", cmd: "⌘", command: "⌘", meta: "⌘", commandorcontrol: "⌘", cmdorctrl: "⌘" }
    : { control: "Ctrl", ctrl: "Ctrl", alt: "Alt", option: "Alt", shift: "Shift", super: "Win", cmd: "Win", command: "Win", meta: "Win", commandorcontrol: "Ctrl", cmdorctrl: "Ctrl" };
  const parts = acc.split("+").map((p) => map[p.trim().toLowerCase()] ?? p.trim());
  return mac ? parts.join("") : parts.join("+");
}

/** "Mod" = ⌘ on macOS, Ctrl elsewhere — for in-app shortcuts. */
export function isMod(e: { metaKey: boolean; ctrlKey: boolean }, platform: Platform): boolean {
  return platform === "macos" ? e.metaKey : e.ctrlKey;
}

export function modLabel(platform: Platform): string {
  return platform === "macos" ? "⌘" : "Ctrl+";
}
