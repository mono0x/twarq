import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import prepare_release as release


MANIFEST = '[package]\nname = "twarq"\nversion = "0.20261017.33"\n'
LOCK = 'version = 4\n\n[[package]]\nname = "twarq"\nversion = "0.20261017.33"\n'


class ReleaseTests(unittest.TestCase):
    def test_version_uses_initial_run_date_in_japan(self):
        self.assertEqual(release.release_version("2026-10-29T16:00:00Z", 42), "0.20261030.42")

    def test_rejects_non_increasing_version(self):
        with self.assertRaises(RuntimeError):
            release.updated_files(MANIFEST, LOCK, "0.20261017.1")

    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        previous = Path.cwd()
        os.chdir(self.directory.name)
        self.addCleanup(os.chdir, previous)
        self.git("init", "--bare", "remote")
        self.git("init", "-b", "main", "work")
        os.chdir("work")
        self.git("config", "commit.gpgsign", "false")
        self.git("config", "core.hooksPath", "/dev/null")
        self.git("config", "user.name", "Test")
        self.git("config", "user.email", "test@example.com")
        Path("Cargo.toml").write_text(MANIFEST)
        Path("Cargo.lock").write_text(LOCK)
        self.git("add", ".")
        self.git("commit", "-m", "Initial")
        self.source = self.git("rev-parse", "HEAD")
        self.git("remote", "add", "origin", "../remote")
        self.git("push", "origin", "main")
        self.environment = patch.dict(os.environ, {
            "GITHUB_REPOSITORY": "owner/twarq",
            "GITHUB_SHA": self.source,
            "GITHUB_RUN_ID": "123",
            "GITHUB_OUTPUT": str(Path.cwd() / "output"),
        })
        self.environment.start()
        self.addCleanup(self.environment.stop)
        self.ci = "success"
        self.published = False
        self.real_run = release.run

    def git(self, *args):
        return subprocess.check_output(["git", *args], text=True, stderr=subprocess.DEVNULL).strip()

    def api(self, endpoint):
        if endpoint.endswith("/123"):
            return {"created_at": "2026-10-30T00:00:00Z", "run_number": 42}
        return {"workflow_runs": [{"status": "completed", "conclusion": self.ci}]}

    def command(self, *args):
        if args[0] == "cargo":
            return "{}"
        if args[0] == "gh":
            return '[[{"tag_name":"v0.20261030.42","draft":false}]]' if self.published else "[[]]"
        return self.real_run(*args)

    def prepare(self):
        with patch.object(release, "api", self.api), patch.object(release, "run", self.command):
            release.main()

    def test_prepare_and_retry_reuse_commit_and_tag(self):
        self.prepare()
        commit = self.git("rev-parse", "HEAD")
        self.assertEqual(self.git("rev-parse", "origin/main"), commit)
        self.assertEqual(self.git("rev-parse", "HEAD^"), self.source)
        self.assertIn('version = "0.20261030.42"', Path("Cargo.toml").read_text())
        self.prepare()
        self.assertEqual(self.git("rev-parse", "HEAD"), commit)
        self.published = True
        self.prepare()
        self.assertEqual(self.git("rev-parse", "HEAD"), commit)
        self.assertIn("published=true", Path("output").read_text())

    def test_failed_ci_does_not_change_remote(self):
        self.ci = "failure"
        with self.assertRaisesRegex(RuntimeError, "CI run"):
            self.prepare()
        self.assertEqual(self.git("rev-parse", "origin/main"), self.source)
        self.assertEqual(self.git("tag"), "")

    def test_advanced_main_requires_new_run(self):
        self.git("commit", "--allow-empty", "-m", "Another change")
        self.git("push", "origin", "main")
        with self.assertRaisesRegex(RuntimeError, "main has advanced"):
            self.prepare()
        self.assertEqual(self.git("tag"), "")

    def test_retry_rejects_unexpected_tag_contents(self):
        Path("unexpected").write_text("change")
        self.git("add", ".")
        self.git("commit", "-m", "Unexpected release")
        self.git("tag", "v0.20261030.42")
        self.git("push", "origin", "main", "--tags")
        with self.assertRaisesRegex(RuntimeError, "unexpected files"):
            self.prepare()

    def test_atomic_push_does_not_leave_tag_when_main_advances(self):
        command = self.command

        def racing_command(*args):
            if args[:3] == ("git", "push", "--atomic"):
                tree = self.git("rev-parse", f"{self.source}^{{tree}}")
                other = self.git("commit-tree", tree, "-p", self.source, "-m", "Concurrent change")
                self.git("push", "origin", f"{other}:refs/heads/main")
            return command(*args)

        with patch.object(release, "api", self.api), patch.object(release, "run", racing_command):
            with self.assertRaises(subprocess.CalledProcessError):
                release.main()
        self.assertEqual(self.git("ls-remote", "--tags", "origin"), "")


if __name__ == "__main__":
    unittest.main()
