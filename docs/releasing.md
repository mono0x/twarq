# Release twarq

## Start a release

Run the **Release** workflow on `main` after its CI run succeeds. No manual
version update or tag creation is needed. The workflow uses
`0.YYYYMMDD.<run_number>`, with the initial run date in Japan time, and pushes
the version commit to `main` together with its tag atomically. The `prepare`
job uses cargo-release to update the version and create the commit and tag.
The `build` job calls a reusable workflow that checks out the tag, builds with
GoReleaser, generates provenance attestations, and uploads workflow artifacts.
The `publish` job downloads those artifacts, verifies their checksums, uploads
them to a draft release, and publishes it. The token must be allowed to push
to `main` under the repository's branch rules.

## Retry a failed run

If CI has not succeeded, rerun after it completes. If `main` has advanced before
the version commit is pushed, start a new workflow run for the latest commit.
Select **Re-run failed jobs** to retry the failed build or publication without
rerunning `prepare`. A publication retry reuses the workflow artifacts, which
are retained for seven days. **Re-run all jobs** rebuilds the existing tag;
use it if the workflow artifacts have expired.
Publication starts only after attestation succeeds. A retry can replace draft
artifacts, while an already published release is left intact.
Fixes to source or workflow code require a new run from the corrected `main`.

The release orchestration is defined in [release.yml](../.github/workflows/release.yml),
and the reusable build workflow in [release-build.yml](../.github/workflows/release-build.yml);
cargo-release settings are in [release.toml](../release.toml).
