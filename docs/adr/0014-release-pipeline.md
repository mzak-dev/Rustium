# Versions come from Conventional Commits through a bot-approved release PR

Every push to `main` runs `scripts/release.py plan`. Releasable commits since the last `v*` tag (`feat`, `fix`, `perf`, `revert`, breaking) make the workflow keep a PR on `release/next` that bumps `[workspace.package] version`, the workspace crates in `Cargo.lock` and `CHANGELOG.md`. A second workflow verifies that PR from main's code and approves it as `github-actions[bot]`; merging it makes the version untagged on main, which the next plan turns into a build of every target, a tag and a GitHub release. The whole workspace shares one version, because the crates are not published and only ship together inside the `rustium` binary. Breaking changes bump the minor version while it is 0.x, as Cargo treats 0.x minor bumps as breaking. Setup and the exact checks are in [docs/releasing.md](../releasing.md).

## Considered Options

- **release-plz**: Rust-native and close to this design, but built around crates.io: it compares packages with the registry, while every crate here is `publish = false`.
- **release-please / semantic-release**: mature, but their Rust support handles `version.workspace = true` poorly, they need Node, and they leave the "is this PR only a bump?" question unanswered, which is what makes auto-approval safe.
- **Tag on every merge, no PR**: simplest, but the version bump would have to be pushed to `main` by a bot, bypassing branch protection, and there would be no point at which several changes are batched into one release.
- **Our own script (chosen)**: about 600 lines of standard-library Python with tests, no third-party tools in the release path, and the verification of the PR lives next to the code that writes it.

## Consequences

- Commit messages are part of the release process: a `feat` written as `fix` ships as a patch. PR titles are checked because squash merges use them.
- The release PR is opened by a GitHub App rather than `GITHUB_TOKEN`, so its pushes start CI and a different identity can approve it. Auto-merge is enabled with the app's token too, since a merge by `GITHUB_TOKEN` would not start the release workflow.
- The auto-approval refuses anything beyond the version lines (dependencies, checksums, other files, extra commits, a version the commits do not call for), so it can be trusted without a human reading the diff; CI still gates the merge.
- `plan` depends only on the commit and the tags, not on the push event, so a cancelled or failed release run is caught up by the next push or a re-run.
- Release binaries are unsigned. Linux builds need glibc 2.39 (Ubuntu 24.04 runner); targeting older distributions would mean building on an older image or in a container.
