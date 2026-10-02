// Signs Android release builds with Zwiim's own key instead of the
// template's debug keystore, which is public: anyone could sign an "update"
// with it. The key comes from Gradle properties, never a committed file;
// set them in the environment as ORG_GRADLE_PROJECT_<name>:
//
//   ZWIIM_ANDROID_KEYSTORE           path to the PKCS12 keystore
//   ZWIIM_ANDROID_KEYSTORE_PASSWORD  its password (also the key's)
//   ZWIIM_ANDROID_KEY_ALIAS          the key's alias
//
// Without them a release build stays debug-signed (local demo builds); the
// release pipeline (scripts/release/build-android.mjs) refuses that.
const { withAppBuildGradle } = require("expo/config-plugins");

const DEBUG_CONFIG = /(signingConfigs \{\n(\s*)debug \{[^}]*\}\n)/;
const RELEASE_TYPE = /(buildTypes \{[\s\S]*?release \{[\s\S]*?)signingConfig signingConfigs\.debug/;

function addReleaseSigning(gradle) {
  if (gradle.includes("ZWIIM_ANDROID_KEYSTORE")) return gradle;
  if (!DEBUG_CONFIG.test(gradle) || !RELEASE_TYPE.test(gradle)) {
    throw new Error("with-release-signing: android/app/build.gradle has an unexpected shape; update the plugin");
  }
  return gradle
    .replace(
      DEBUG_CONFIG,
      (_, debug, indent) =>
        debug +
        `${indent}release {\n` +
        `${indent}    if (findProperty('ZWIIM_ANDROID_KEYSTORE')) {\n` +
        `${indent}        storeFile file(findProperty('ZWIIM_ANDROID_KEYSTORE'))\n` +
        `${indent}        storePassword findProperty('ZWIIM_ANDROID_KEYSTORE_PASSWORD')\n` +
        `${indent}        keyAlias findProperty('ZWIIM_ANDROID_KEY_ALIAS')\n` +
        `${indent}        keyPassword findProperty('ZWIIM_ANDROID_KEYSTORE_PASSWORD')\n` +
        `${indent}    }\n` +
        `${indent}}\n`,
    )
    .replace(
      RELEASE_TYPE,
      "$1signingConfig findProperty('ZWIIM_ANDROID_KEYSTORE') ? signingConfigs.release : signingConfigs.debug",
    );
}

module.exports = function withReleaseSigning(config) {
  return withAppBuildGradle(config, (config) => {
    config.modResults.contents = addReleaseSigning(config.modResults.contents);
    return config;
  });
};
module.exports.addReleaseSigning = addReleaseSigning;
