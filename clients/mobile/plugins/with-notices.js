// Ships THIRD_PARTY_NOTICES.txt (scripts/licenses/notices.mjs) inside the
// Android app, as an asset next to the bundle. iOS is not built yet; add it
// to the app's resources when it is.
const fs = require("fs");
const path = require("path");
const { withDangerousMod } = require("expo/config-plugins");

const NOTICES = "THIRD_PARTY_NOTICES.txt";

module.exports = function withNotices(config) {
  return withDangerousMod(config, [
    "android",
    (config) => {
      const assets = path.join(config.modRequest.platformProjectRoot, "app/src/main/assets");
      fs.mkdirSync(assets, { recursive: true });
      fs.copyFileSync(path.join(config.modRequest.projectRoot, NOTICES), path.join(assets, NOTICES));
      return config;
    },
  ]);
};
