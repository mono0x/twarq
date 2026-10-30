# Release twarq

## Start a release

Run the **Release** workflow on `main` after its CI run succeeds. No manual
version update or tag creation is needed. The workflow uses
`0.YYYYMMDD.<run_number>`, with the initial run date in Japan time, and pushes
the version commit to `main` together with its tag atomically. The `prepare`
job uses cargo-release to update the version and create the commit and tag;
the `publish` job checks out that tag and builds with GoReleaser. The token must
be allowed to push to `main` under the repository's branch rules.

## Retry a failed run

If CI has not succeeded, rerun after it completes. If `main` has advanced before
the version commit is pushed, start a new workflow run for the latest commit.
If `publish` fails, select **Re-run failed jobs** to rebuild the same tag without
rerunning `prepare`. **Re-run all jobs** also reuses the existing tag.
Artifacts remain in a draft until attestation succeeds; a retry
can replace draft artifacts, while an already published release is left intact.
Fixes to source or workflow code require a new run from the corrected `main`.

The workflow is defined in [release.yml](../.github/workflows/release.yml);
cargo-release settings are in [release.toml](../release.toml).
