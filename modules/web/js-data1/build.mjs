// Bundle the js-data1 module (including its @aws-sdk/client-s3 dependency) into a single self-contained ESM
// that et-modules-service can serve as a static file.
//
// The esbuild JS API is used rather than the `esbuild` CLI bin: esbuild's postinstall overwrites its JS bin
// shim with the platform-native binary, which pnpm's node-based `.bin` wrapper then fails to execute
// ("Invalid or unexpected token"). The JS API loads the native binary itself and sidesteps that entirely.

import { build } from "esbuild";

await build({
  bundle: true,
  entryPoints: ["src/index.js"],
  format: "esm",
  outfile: "pkg/et_ws_js_data1.js",
  platform: "browser",
  target: "es2022",
});
