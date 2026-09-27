import { ZwiimClientError } from "@zwiim/sdk";
import { describe, expect, it } from "vitest";
import {
  addLabel,
  addProblem,
  browseProblem,
  canAdd,
  canCreateFolder,
  createFolderProblem,
  folderName,
  folderNameProblem,
  homeText,
  upPath,
} from "../src/files/browse";

const err = (code: ConstructorParameters<typeof ZwiimClientError>[0]) => new ZwiimClientError(code, "x");

describe("browsing the Mac's home folder", () => {
  it("names folders from ~", () => {
    expect(homeText([])).toBe("~");
    expect(homeText(["Projects", "zwiim"])).toBe("~/Projects/zwiim");
    expect(folderName(["Projects", "zwiim"])).toBe("zwiim");
  });

  it("adds anything but home itself", () => {
    expect(canAdd([])).toBe(false);
    expect(canAdd(["Projects"])).toBe(true);
    expect(addLabel(["Projects", "zwiim"])).toBe("Add ~/Projects/zwiim");
  });

  it("goes up one folder, never above home", () => {
    expect(upPath(["Projects", "zwiim"])).toEqual(["Projects"]);
    expect(upPath(["Projects"])).toEqual([]);
    expect(upPath([])).toEqual([]);
  });

  it("says why a folder didn't open", () => {
    const path = ["Projects", "zwiim"];
    expect(browseProblem(err("permission_denied"), path, "Studio Mac").title).toBe("Zwiim can't open zwiim on Studio Mac.");
    expect(browseProblem(err("not_found"), path, "Studio Mac").title).toBe("zwiim is gone.");
    expect(browseProblem(err("network"), path, "Studio Mac").title).toBe("Not connected to Studio Mac");
  });

  it("says why adding didn't work", () => {
    const path = ["Projects", "zwiim"];
    expect(addProblem(err("permission_denied"), path, "Studio Mac")).toBe("Zwiim can't open zwiim on Studio Mac.");
    expect(addProblem(err("not_found"), path, "Studio Mac")).toBe("zwiim is gone.");
    expect(addProblem(new Error("x"), path, "Studio Mac")).toBe("Couldn't add zwiim. Try again.");
  });
});

describe("making a folder on the Mac", () => {
  it("takes a plain, visible folder name", () => {
    expect(canCreateFolder("new app")).toBe(true);
    expect(canCreateFolder("  spaced  ")).toBe(true);
    expect(folderNameProblem("new app")).toBeNull();
    // Empty: nothing to say, nothing to send.
    expect(canCreateFolder("  ")).toBe(false);
    expect(folderNameProblem("")).toBeNull();
  });

  it("refuses hidden names, slashes and paths before asking", () => {
    for (const bad of [".hidden", ".", "..", "a/b", "/Users/me", "a\\b", "a\u0000b"]) {
      expect(canCreateFolder(bad)).toBe(false);
      expect(folderNameProblem(bad)).not.toBeNull();
    }
    expect(folderNameProblem(".env")).toBe("Folder names can't start with a dot.");
    expect(folderNameProblem("~/x/y")).toBe("Folder names can't contain slashes.");
    expect(canCreateFolder("x".repeat(256))).toBe(false);
    expect(canCreateFolder("x".repeat(255))).toBe(true);
  });

  it("says why a folder wasn't made", () => {
    const path = ["Projects"];
    const say = (code: Parameters<typeof err>[0]) => createFolderProblem(err(code), path, " app ", "Studio Mac");
    expect(say("already_exists")).toBe("There's already something named app in ~/Projects.");
    expect(say("permission_denied")).toBe("Zwiim can't create folders in Projects on Studio Mac.");
    expect(say("not_found")).toBe("Projects is gone.");
    expect(say("invalid_input")).toBe("Use a plain folder name: no slashes, not starting with a dot.");
    expect(say("network")).toBe("Not connected to Studio Mac. Try again when it's back.");
    expect(say("unknown_outcome")).toBe("Not sure that went through. Refreshed from your Mac.");
    expect(say("internal")).toBe("Couldn't create the folder. Try again.");
    expect(createFolderProblem(err("already_exists"), [], "app", "Studio Mac")).toBe("There's already something named app in ~.");
  });
});
