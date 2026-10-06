# et-modules-service

Serves edge-toolkit ws-module packages to browsers as static files. A ws-module is any directory holding a
`package.json` whose `main` names the entry point the browser loads; the server only scans for those directories
and serves their files, and never runs any of them itself.

[`configure`] mounts three kinds of route on an `actix-web` app: a JSON list of every module found at `/modules/`,
each module's files under `/modules/{name}/`, and the root module -- the one [`ModulesConfig`] names as the
deployment's front page -- at `/`.
