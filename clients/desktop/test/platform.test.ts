import { describe, expect, it } from "vitest";
import { detectPlatform, deviceNoun, formatShortcut, isModKey } from "@/lib/platform";

describe("platform", () => {
  it("names the platform a user agent runs on", () => {
    expect(detectPlatform("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15")).toBe("macos");
    expect(detectPlatform("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")).toBe("windows");
    expect(detectPlatform("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36")).toBe("linux");
    expect(detectPlatform("")).toBe("unknown");
  });

  it("writes a shortcut the way each platform shows it", () => {
    expect(formatShortcut("Mod+,", "macos")).toBe("⌘,");
    expect(formatShortcut("Mod+Enter", "macos")).toBe("⌘↵");
    expect(formatShortcut("Mod+Shift+f", "macos")).toBe("⌘⇧F");
    expect(formatShortcut("Mod+,", "windows")).toBe("Ctrl+,");
    expect(formatShortcut("Mod+Enter", "linux")).toBe("Ctrl+Enter");
    expect(formatShortcut("Mod+Shift+f", "unknown")).toBe("Ctrl+Shift+F");
  });

  it("takes ⌘ as the main modifier on a Mac and Ctrl elsewhere", () => {
    const meta = { metaKey: true, ctrlKey: false };
    const ctrl = { metaKey: false, ctrlKey: true };
    expect(isModKey(meta, "macos")).toBe(true);
    expect(isModKey(ctrl, "macos")).toBe(false);
    expect(isModKey(ctrl, "windows")).toBe(true);
    expect(isModKey(meta, "linux")).toBe(false);
  });

  it("calls the computer what its platform calls it", () => {
    expect(deviceNoun("macos")).toBe("this Mac");
    expect(deviceNoun("windows")).toBe("this PC");
    expect(deviceNoun("linux")).toBe("this computer");
  });
});
