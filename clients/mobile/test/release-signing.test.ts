import { describe, expect, it } from "vitest";
import withReleaseSigning from "../plugins/with-release-signing.js";

const { addReleaseSigning } = withReleaseSigning as unknown as { addReleaseSigning: (gradle: string) => string };

// android/app/build.gradle as `expo prebuild` writes it (Expo 57).
const generated = `android {
    signingConfigs {
        debug {
            storeFile file('debug.keystore')
            storePassword 'android'
            keyAlias 'androiddebugkey'
            keyPassword 'android'
        }
    }
    buildTypes {
        debug {
            signingConfig signingConfigs.debug
        }
        release {
            // Caution! In production, you need to generate your own keystore file.
            // see https://reactnative.dev/docs/signed-apk-android.
            signingConfig signingConfigs.debug
            minifyEnabled enableMinifyInReleaseBuilds
        }
    }
}
`;

describe("Android release signing", () => {
  it("signs release builds with Zwiim's key when it is given", () => {
    const out = addReleaseSigning(generated);
    expect(out).toContain("        release {\n            if (findProperty('ZWIIM_ANDROID_KEYSTORE')) {");
    expect(out).toContain("storeFile file(findProperty('ZWIIM_ANDROID_KEYSTORE'))");
    expect(out).toContain(
      "signingConfig findProperty('ZWIIM_ANDROID_KEYSTORE') ? signingConfigs.release : signingConfigs.debug",
    );
  });

  it("leaves debug builds on the debug key", () => {
    const out = addReleaseSigning(generated);
    expect(out).toMatch(/debug \{\n\s*signingConfig signingConfigs\.debug\n/);
  });

  it("applies once", () => {
    const once = addReleaseSigning(generated);
    expect(addReleaseSigning(once)).toBe(once);
  });

  it("refuses a build.gradle it does not recognize", () => {
    expect(() => addReleaseSigning("android {}\n")).toThrow(/unexpected shape/);
  });
});
