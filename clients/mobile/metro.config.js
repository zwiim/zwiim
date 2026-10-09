// The app bundles the Zwiim SDK and preview package from their source in
// `clients/sdk` and `clients/preview`, as the Desktop does. Their own
// dependencies are installed at the repository root (`npm install` there),
// so Metro watches and resolves from both.
const { getDefaultConfig } = require("expo/metro-config");
const path = require("path");

const root = path.resolve(__dirname, "../..");
const config = getDefaultConfig(__dirname);

config.watchFolders = [path.join(root, "clients/sdk"), path.join(root, "clients/preview"), path.join(root, "node_modules")];
config.resolver.nodeModulesPaths = [path.join(__dirname, "node_modules"), path.join(root, "node_modules")];

module.exports = config;
