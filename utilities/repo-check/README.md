# et-int-repo-check

Repository-wide checks that no file-at-a-time linter can answer: the answer lives in the object database or the
commit graph, or comes from running another tool and judging its result. Anything one of the repo's linters can
express belongs there instead.

Run it with:

```bash
mise run repo-check
```

It takes no arguments and reads only the checkout it runs in, so it needs no network and no credentials. It
does need real history: a shallow clone resolves almost nothing. It runs from the repository root, with the mise
tools on `PATH`.

## Checks

- **commit-hashes** -- every full 40-character commit hash written into a tracked file must name a commit this
  repository has. Each failure prints the file and line to edit, plus the web URL to write instead.
- **typos-ignores** -- every entry in `config/typos.toml` that suppresses findings (`extend-ignore-re`,
  `extend-identifiers`, `extend-words`, `files.extend-exclude`, ...) must still suppress at least one. Each entry is
  judged by running typos with that entry alone left out; one that leaves typos clean is reported for removal.

To add another, put it in its own module with a `run` returning its failure count, and call it from `main`.
