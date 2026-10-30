import datetime
import json
import os
from pathlib import Path
import re
import subprocess
import tomllib
from zoneinfo import ZoneInfo


def run(*args):
    return subprocess.check_output(args, text=True).strip()


def api(endpoint):
    return json.loads(run("gh", "api", endpoint))


def release_version(created_at, number):
    date = datetime.datetime.fromisoformat(created_at.replace("Z", "+00:00"))
    return f"0.{date.astimezone(ZoneInfo('Asia/Tokyo')):%Y%m%d}.{int(number)}"


def updated_files(manifest, lock, version):
    package = tomllib.loads(manifest)["package"]
    old = package["version"]
    if tuple(map(int, version.split("."))) <= tuple(map(int, old.split("."))):
        raise RuntimeError("Release version must be greater than the current version")
    manifest, count = re.subn(
        rf'(?m)^version = "{re.escape(old)}"$', f'version = "{version}"', manifest
    )
    if count != 1:
        raise RuntimeError("Expected exactly one package version in Cargo.toml")
    lock, count = re.subn(
        rf'(\[\[package\]\]\nname = "{re.escape(package["name"])}"\nversion = "){re.escape(old)}"',
        rf'\g<1>{version}"',
        lock,
    )
    if count != 1:
        raise RuntimeError("Expected exactly one matching package in Cargo.lock")
    return {"Cargo.toml": manifest, "Cargo.lock": lock}


def main():
    repository = os.environ["GITHUB_REPOSITORY"]
    source = os.environ["GITHUB_SHA"]
    run_id = os.environ["GITHUB_RUN_ID"]
    metadata = api(f"repos/{repository}/actions/runs/{run_id}")
    version = release_version(metadata["created_at"], metadata["run_number"])
    tag = f"v{version}"
    expected = updated_files(
        run("git", "show", f"{source}:Cargo.toml") + "\n",
        run("git", "show", f"{source}:Cargo.lock") + "\n",
        version,
    )
    run("git", "fetch", "origin", "main", "--tags")
    exists = subprocess.run(
        ["git", "show-ref", "--verify", "--quiet", f"refs/tags/{tag}"]
    ).returncode == 0
    if exists:
        if run("git", "rev-list", "--parents", "-n", "1", tag).split()[1:] != [source]:
            raise RuntimeError("Existing release tag has an unexpected parent")
        if run("git", "diff", "--name-only", source, tag).splitlines() != sorted(expected):
            raise RuntimeError("Existing release tag changes unexpected files")
        for path, content in expected.items():
            if run("git", "show", f"{tag}:{path}") + "\n" != content:
                raise RuntimeError(f"Existing release tag has unexpected {path}")
        run("git", "merge-base", "--is-ancestor", tag, "origin/main")
        run("git", "checkout", "--detach", tag)
    else:
        if run("git", "rev-parse", "origin/main") != source:
            raise RuntimeError("main has advanced; start a new release workflow")
        runs = api(
            f"repos/{repository}/actions/workflows/ci.yml/runs"
            f"?head_sha={source}&branch=main&event=push&per_page=1"
        )["workflow_runs"]
        if not runs or runs[0]["status"] != "completed" or runs[0]["conclusion"] != "success":
            raise RuntimeError("The latest push CI run for this commit must succeed first")
        run("git", "checkout", "--detach", source)
        for path, content in expected.items():
            Path(path).write_text(content)
        run("cargo", "metadata", "--locked", "--offline", "--no-deps", "--format-version", "1")
        run("git", "config", "user.name", "github-actions[bot]")
        run("git", "config", "user.email", "github-actions[bot]@users.noreply.github.com")
        run("git", "add", "Cargo.toml", "Cargo.lock")
        run("git", "commit", "-m", f"Bump version to {version}")
        run("git", "tag", tag)
        run("git", "push", "--atomic", "origin", "HEAD:refs/heads/main", f"refs/tags/{tag}")

    # Listing releases distinguishes an absent release from authentication/network errors.
    pages = json.loads(run("gh", "api", "--paginate", "--slurp", f"repos/{repository}/releases"))
    release = next((item for page in pages for item in page if item["tag_name"] == tag), None)
    published = release is not None and not release["draft"]
    with open(os.environ["GITHUB_OUTPUT"], "a") as output:
        output.write(f"tag={tag}\npublished={str(published).lower()}\n")
    print(f"Release {tag}: {'already published' if published else 'ready to build'}")


if __name__ == "__main__":
    main()
