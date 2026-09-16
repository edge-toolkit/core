# et-repo-check

Repository-wide checks that need to read git itself, rather than judge a file on its own. Anything one of the
repo's linters can express belongs there instead; this is for the questions they cannot answer, because the
answer lives in the object database or the commit graph rather than in any file.

Run it with:

```bash
mise run repo-check
```

It takes no arguments and reads only the checkout it runs in, so it needs no network and no credentials. It
does need real history: a shallow clone resolves almost nothing.

## Checks

- **commit-hashes** -- every full 40-character commit hash written into a tracked file must name a commit this
  repository has. Each failure prints the file and line to edit, plus the web URL to write instead.

To add another, put it in its own module with a `run(&[String]) -> anyhow::Result<usize>` returning its failure
count, and call it from `main`.
