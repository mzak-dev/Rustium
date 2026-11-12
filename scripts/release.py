#!/usr/bin/env python3
"""Release tooling for Rustium: Conventional Commits decide the next workspace version.

The workspace has one version, `[workspace.package] version` in the root `Cargo.toml`, inherited by every crate.
This script reads the commits since the last `v*` tag, decides the bump, and writes it into `Cargo.toml`,
`Cargo.lock` and `CHANGELOG.md`. The release workflow (`.github/workflows/release.yml`) calls it; it also runs
locally from the repository root. Standard library only (Python 3.11+).

Bump rules:
    BREAKING CHANGE footer or `type!:`   major (minor while the version is 0.x)
    feat                                minor
    fix, perf, revert                   patch
    anything else                       no release

Subcommands:
    plan          decide what the release workflow does for a push to main
    prepare       write the next version into Cargo.toml, Cargo.lock and CHANGELOG.md
    verify-pr     check that a release PR is exactly the bump its commits call for
    notes         print the release notes of one version
    check-title   check that a pull request title follows Conventional Commits
"""

from __future__ import annotations

import argparse
import dataclasses
import datetime
import enum
import os
import re
import subprocess
import sys
import tomllib
from collections.abc import Iterable, Sequence
from pathlib import Path

TAG_PREFIX = "v"
CHANGELOG = "CHANGELOG.md"
# The only files a release PR may touch; the auto-approval refuses anything else.
RELEASE_FILES = frozenset({"Cargo.toml", "Cargo.lock", CHANGELOG})
ALLOWED_TYPES = ("feat", "fix", "perf", "revert", "docs", "style", "refactor", "test", "build", "ci", "chore")

HEADER_RE = re.compile(r"^(?P<type>[a-z]+)(?:\((?P<scope>[^()\r\n]+)\))?(?P<bang>!)?: (?P<description>\S.*)$")
BREAKING_FOOTER_RE = re.compile(r"^BREAKING[ -]CHANGE: ", re.MULTILINE)
VERSION_RE = re.compile(r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$")
SECTION_HEADING_RE = re.compile(r"^## \[?(?P<version>\d+\.\d+\.\d+)\]?", re.MULTILINE)
PULL_REQUEST_REF_RE = re.compile(r"\(#(\d+)\)")

CHANGELOG_HEADER = """# Changelog

All notable changes to Rustium. The release workflow writes this file from
[Conventional Commits](https://www.conventionalcommits.org/) (see `scripts/release.py`); versions follow
[Semantic Versioning](https://semver.org/), with breaking changes bumping the minor version while it is 0.x.
"""

CHANGELOG_SECTIONS = (
    ("breaking", "⚠ Breaking changes"),
    ("feat", "Features"),
    ("fix", "Bug fixes"),
    ("perf", "Performance"),
    ("revert", "Reverts"),
)


class ReleaseError(Exception):
    """A release step cannot continue; the message says why."""


class Bump(enum.IntEnum):
    """How far a set of commits moves the version."""

    NONE = 0
    PATCH = 1
    MINOR = 2
    MAJOR = 3


@dataclasses.dataclass(frozen=True, order=True)
class Version:
    """A plain `MAJOR.MINOR.PATCH` version (pre-release suffixes are not used by this project)."""

    major: int
    minor: int
    patch: int

    @classmethod
    def parse(cls, text: str) -> Version:
        match = VERSION_RE.match(text.strip())
        if match is None:
            raise ReleaseError(f"`{text}` is not a MAJOR.MINOR.PATCH version")
        return cls(*(int(part) for part in match.groups()))

    @classmethod
    def from_tag(cls, tag: str) -> Version | None:
        if not tag.startswith(TAG_PREFIX):
            return None
        try:
            return cls.parse(tag.removeprefix(TAG_PREFIX))
        except ReleaseError:
            return None

    def bumped(self, bump: Bump) -> Version:
        if bump is Bump.MAJOR and self.major == 0:
            # Pre-1.0: a breaking change moves the minor version (Cargo treats 0.x minor bumps as breaking).
            bump = Bump.MINOR
        if bump is Bump.MAJOR:
            return Version(self.major + 1, 0, 0)
        if bump is Bump.MINOR:
            return Version(self.major, self.minor + 1, 0)
        if bump is Bump.PATCH:
            return Version(self.major, self.minor, self.patch + 1)
        return self

    @property
    def tag(self) -> str:
        return f"{TAG_PREFIX}{self}"

    def __str__(self) -> str:
        return f"{self.major}.{self.minor}.{self.patch}"


@dataclasses.dataclass(frozen=True)
class Commit:
    """One Conventional Commit."""

    sha: str
    type: str
    scope: str | None
    breaking: bool
    description: str

    @classmethod
    def parse(cls, sha: str, subject: str, body: str = "") -> Commit | None:
        """Returns `None` for commits that do not follow Conventional Commits (they never trigger a release)."""
        match = HEADER_RE.match(subject.strip())
        if match is None:
            return None
        breaking = match["bang"] is not None or BREAKING_FOOTER_RE.search(body) is not None
        return cls(sha, match["type"], match["scope"], breaking, match["description"].strip())

    @property
    def bump(self) -> Bump:
        if self.breaking:
            return Bump.MAJOR
        if self.type == "feat":
            return Bump.MINOR
        if self.type in ("fix", "perf", "revert"):
            return Bump.PATCH
        return Bump.NONE


def bump_for(commits: Iterable[Commit]) -> Bump:
    return max((commit.bump for commit in commits), default=Bump.NONE)


class Repo:
    """Git and Cargo state of one checkout."""

    def __init__(self, root: Path) -> None:
        self.root = root

    def git(self, *args: str, check: bool = True) -> str:
        result = subprocess.run(
            ["git", *args], cwd=self.root, capture_output=True, text=True, encoding="utf-8", check=False
        )
        if check and result.returncode != 0:
            raise ReleaseError(f"`git {' '.join(args)}` failed: {result.stderr.strip()}")
        return result.stdout

    def show(self, rev: str, path: str) -> str | None:
        result = subprocess.run(
            ["git", "show", f"{rev}:{path}"], cwd=self.root, capture_output=True, text=True, encoding="utf-8"
        )
        return result.stdout if result.returncode == 0 else None

    def initial_version(self, rev: str) -> Version | None:
        """The workspace version in the first commit (up to `rev`) that had a root Cargo.toml."""
        first = self.git("log", "--reverse", "--format=%H", rev, "--", "Cargo.toml").split()
        if not first:
            return None
        try:
            return self.workspace_version(first[0])
        except (ReleaseError, tomllib.TOMLDecodeError):
            return None

    def tag_exists(self, tag: str) -> bool:
        return bool(self.git("rev-parse", "--quiet", "--verify", f"refs/tags/{tag}", check=False).strip())

    def latest_release(self, rev: str) -> Version | None:
        """The highest `vMAJOR.MINOR.PATCH` tag that is an ancestor of `rev`."""
        tags = self.git("tag", "--merged", rev, "--list", f"{TAG_PREFIX}*").split()
        versions = [version for version in (Version.from_tag(tag) for tag in tags) if version is not None]
        return max(versions, default=None)

    def commits(self, since: Version | None, rev: str) -> list[Commit]:
        """Conventional commits after the `since` tag up to `rev`, oldest first."""
        revision_range = f"{since.tag}..{rev}" if since is not None else rev
        log = self.git("log", "--no-merges", "--reverse", "--format=%H%x1f%s%x1f%b%x1e", revision_range)
        commits = []
        for record in log.split("\x1e"):
            fields = record.strip("\n").split("\x1f")
            if len(fields) != 3:
                continue
            commit = Commit.parse(*fields)
            if commit is not None:
                commits.append(commit)
        return commits

    def manifest(self, rev: str | None) -> str:
        return self._file(rev, "Cargo.toml")

    def lockfile(self, rev: str | None) -> str:
        return self._file(rev, "Cargo.lock")

    def changelog(self, rev: str | None) -> str | None:
        if rev is None:
            path = self.root / CHANGELOG
            return path.read_text(encoding="utf-8") if path.exists() else None
        return self.show(rev, CHANGELOG)

    def workspace_version(self, rev: str | None) -> Version:
        return Version.parse(workspace_version_text(self.manifest(rev)))

    def inherited_members(self, rev: str | None) -> set[str]:
        """Names of the workspace crates whose version is `version.workspace = true`."""
        manifest = tomllib.loads(self.manifest(rev))
        names = set()
        for member in manifest.get("workspace", {}).get("members", []):
            text = self._file(rev, f"{member}/Cargo.toml")
            package = tomllib.loads(text).get("package", {})
            if package.get("version") == {"workspace": True}:
                names.add(package["name"])
        return names

    def _file(self, rev: str | None, path: str) -> str:
        if rev is None:
            return (self.root / path).read_text(encoding="utf-8")
        text = self.show(rev, path)
        if text is None:
            raise ReleaseError(f"{path} does not exist at {rev}")
        return text


@dataclasses.dataclass(frozen=True)
class ReleaseState:
    """What the history up to one commit says about the next release."""

    current: Version
    previous: Version | None
    commits: list[Commit]

    @property
    def next(self) -> Version | None:
        bump = bump_for(self.commits)
        return self.current.bumped(bump) if bump is not Bump.NONE else None


def release_state(repo: Repo, rev: str) -> ReleaseState:
    previous = repo.latest_release(rev)
    return ReleaseState(repo.workspace_version(rev), previous, repo.commits(previous, rev))


# ---------------------------------------------------------------------------------------------------------------
# Cargo.toml / Cargo.lock edits
# ---------------------------------------------------------------------------------------------------------------


def workspace_version_text(manifest_text: str) -> str:
    try:
        return tomllib.loads(manifest_text)["workspace"]["package"]["version"]
    except KeyError as error:
        raise ReleaseError("the root Cargo.toml has no [workspace.package] version") from error


def set_workspace_version(manifest_text: str, version: Version) -> str:
    """Rewrites only the `version` line of `[workspace.package]`, keeping comments and layout."""
    lines = manifest_text.splitlines(keepends=True)
    in_section = False
    for index, line in enumerate(lines):
        stripped = line.strip()
        if stripped.startswith("["):
            in_section = stripped == "[workspace.package]"
        elif in_section and re.match(r"version\s*=", stripped):
            lines[index] = re.sub(r'(version\s*=\s*)"[^"]*"', rf'\g<1>"{version}"', line, count=1)
            return "".join(lines)
    raise ReleaseError("the root Cargo.toml has no [workspace.package] version line")


def set_lock_versions(lock_text: str, names: set[str], version: Version) -> str:
    """Sets the version of the workspace packages (those without a `source`) in Cargo.lock."""
    blocks = re.split(r"(?m)^(?=\[\[package\]\]$)", lock_text)
    found = set()
    for index, block in enumerate(blocks):
        name = re.search(r'(?m)^name = "([^"]+)"$', block)
        if name is None or name[1] not in names or re.search(r"(?m)^source = ", block):
            continue
        blocks[index] = re.sub(r'(?m)^version = "[^"]*"$', f'version = "{version}"', block, count=1)
        found.add(name[1])
    missing = names - found
    if missing:
        raise ReleaseError(f"Cargo.lock has no entry for {', '.join(sorted(missing))}; run `cargo metadata` first")
    return "".join(blocks)


# ---------------------------------------------------------------------------------------------------------------
# Changelog
# ---------------------------------------------------------------------------------------------------------------


def render_section(
    version: Version, previous: Version | None, commits: Sequence[Commit], date: datetime.date, repo_url: str | None
) -> str:
    if repo_url and previous is not None:
        heading = f"## [{version}]({repo_url}/compare/{previous.tag}...{version.tag}) - {date.isoformat()}"
    elif repo_url:
        heading = f"## [{version}]({repo_url}/releases/tag/{version.tag}) - {date.isoformat()}"
    else:
        heading = f"## {version} - {date.isoformat()}"

    groups: dict[str, list[Commit]] = {key: [] for key, _ in CHANGELOG_SECTIONS}
    for commit in commits:
        key = "breaking" if commit.breaking else commit.type
        if key in groups:
            groups[key].append(commit)

    parts = [heading, ""]
    for key, title in CHANGELOG_SECTIONS:
        if not groups[key]:
            continue
        parts.append(f"### {title}")
        parts.append("")
        parts.extend(render_entry(commit, repo_url) for commit in groups[key])
        parts.append("")
    if len(parts) == 2:
        parts.extend(["No user-facing changes.", ""])
    return "\n".join(parts)


def render_entry(commit: Commit, repo_url: str | None) -> str:
    description = commit.description
    short_sha = commit.sha[:7]
    if repo_url:
        description = PULL_REQUEST_REF_RE.sub(rf"([#\1]({repo_url}/pull/\1))", description)
        reference = f"([{short_sha}]({repo_url}/commit/{commit.sha}))"
    else:
        reference = f"({short_sha})"
    scope = f"**{commit.scope}:** " if commit.scope else ""
    return f"- {scope}{description} {reference}"


def insert_section(changelog_text: str | None, section: str, version: Version) -> str:
    if not changelog_text:
        return f"{CHANGELOG_HEADER}\n{section}"
    if find_section(changelog_text, version) is not None:
        raise ReleaseError(f"{CHANGELOG} already has a section for {version}")
    first = SECTION_HEADING_RE.search(changelog_text)
    if first is None:
        return f"{changelog_text.rstrip()}\n\n{section}"
    return f"{changelog_text[: first.start()]}{section}\n{changelog_text[first.start() :]}"


def find_section(changelog_text: str, version: Version) -> str | None:
    """The body (without the heading) of the changelog section for `version`."""
    headings = list(SECTION_HEADING_RE.finditer(changelog_text))
    for index, heading in enumerate(headings):
        if heading["version"] != str(version):
            continue
        line_end = changelog_text.find("\n", heading.end())
        start = len(changelog_text) if line_end == -1 else line_end + 1
        end = headings[index + 1].start() if index + 1 < len(headings) else len(changelog_text)
        return changelog_text[start:end].strip() + "\n"
    return None


# ---------------------------------------------------------------------------------------------------------------
# Subcommands
# ---------------------------------------------------------------------------------------------------------------


def plan(repo: Repo, rev: str, force: bool) -> dict[str, str]:
    """Decides what the release workflow does for main at `rev`.

    `release`: the version in Cargo.toml has no tag yet, so build and publish it. Before the first tag exists that
    needs a bump away from the version Cargo.toml started with (or `force`). The decision depends only on `rev`
    and the tags, so a skipped or failed run is caught up by the next one. `pr`: open or refresh the release PR
    with the next version. `none`: nothing releasable since the last tag, so close a stale release PR.
    """
    current = repo.workspace_version(rev)
    latest = repo.latest_release(rev)
    outputs = {"version": str(current), "tag": current.tag, "previous-tag": latest.tag if latest else ""}

    if not repo.tag_exists(current.tag):
        if latest is not None and current < latest:
            raise ReleaseError(f"Cargo.toml says {current}, which is older than the released {latest.tag}")
        bumped = latest is not None or repo.initial_version(rev) not in (None, current)
        if force or bumped:
            return {**outputs, "mode": "release", "next-version": ""}
    elif force:
        raise ReleaseError(f"{current.tag} is already released; bump the version to release again")

    state = release_state(repo, rev)
    next_version = state.next
    return {
        **outputs,
        "mode": "pr" if next_version is not None else "none",
        "next-version": str(next_version) if next_version else "",
    }


def prepare(repo: Repo, version: Version | None, date: datetime.date, repo_url: str | None) -> Version:
    """Writes the next version into the working tree of `repo`; returns it."""
    state = release_state(repo, "HEAD")
    version = version or state.next
    if version is None:
        raise ReleaseError("no feat, fix, perf, revert or breaking commits since the last release")
    if version <= state.current:
        raise ReleaseError(f"{version} is not newer than the current {state.current}")

    members = repo.inherited_members(None)
    (repo.root / "Cargo.toml").write_text(set_workspace_version(repo.manifest(None), version), encoding="utf-8")
    (repo.root / "Cargo.lock").write_text(set_lock_versions(repo.lockfile(None), members, version), encoding="utf-8")
    section = render_section(version, state.previous, state.commits, date, repo_url)
    changelog = insert_section(repo.changelog(None), section, version)
    (repo.root / CHANGELOG).write_text(changelog, encoding="utf-8")
    return version


def verify_pr(repo: Repo, base: str, head: str) -> Version:
    """Checks that `head` is one commit on top of `base` that only applies the bump the commits call for.

    Run it from a checkout of the trusted base branch: it reads the PR's files through git, never runs them.
    """
    merge_base = repo.git("merge-base", base, head).strip()
    extra_commits = int(repo.git("rev-list", "--count", f"{merge_base}..{head}").strip())
    if extra_commits != 1:
        raise ReleaseError(f"a release PR is exactly one commit on top of main, this one has {extra_commits}")

    changed = set(repo.git("diff", "--no-renames", "--name-only", merge_base, head).split())
    unexpected = changed - RELEASE_FILES
    if unexpected:
        raise ReleaseError(f"a release PR may only change {sorted(RELEASE_FILES)}, not {sorted(unexpected)}")

    state = release_state(repo, merge_base)
    expected = state.next
    if expected is None:
        raise ReleaseError("main has no releasable commits since the last release, so there is nothing to bump")

    old_manifest = tomllib.loads(repo.manifest(merge_base))
    new_manifest = tomllib.loads(repo.manifest(head))
    new_version = Version.parse(workspace_version_text(repo.manifest(head)))
    if new_version != expected:
        raise ReleaseError(f"the commits since {state.previous or 'the start'} call for {expected}, not {new_version}")
    del old_manifest["workspace"]["package"]["version"]
    del new_manifest["workspace"]["package"]["version"]
    if old_manifest != new_manifest:
        raise ReleaseError("Cargo.toml changes more than [workspace.package] version")

    members = repo.inherited_members(merge_base)
    old_lock = normalize_lock(tomllib.loads(repo.lockfile(merge_base)), members, state.current)
    new_lock = normalize_lock(tomllib.loads(repo.lockfile(head)), members, expected)
    if old_lock != new_lock:
        raise ReleaseError("Cargo.lock changes more than the versions of the workspace crates")

    old_changelog = repo.changelog(merge_base) or ""
    new_changelog = repo.changelog(head) or ""
    if find_section(new_changelog, expected) is None:
        raise ReleaseError(f"{CHANGELOG} has no section for {expected}")
    first = SECTION_HEADING_RE.search(old_changelog)
    if first is not None and old_changelog[first.start() :] not in new_changelog:
        raise ReleaseError(f"{CHANGELOG} changes earlier releases")
    return expected


def normalize_lock(lock: dict, members: set[str], version: Version) -> dict:
    """Replaces the workspace crates' versions with a placeholder after checking they are `version`."""
    for package in lock.get("package", []):
        if package.get("name") in members and "source" not in package:
            if package.get("version") != str(version):
                raise ReleaseError(f"Cargo.lock has {package['name']} {package.get('version')}, expected {version}")
            package["version"] = "<workspace>"
    return lock


def release_notes(repo: Repo, version: Version, repo_url: str | None) -> str:
    changelog = repo.changelog(None)
    section = find_section(changelog, version) if changelog else None
    if section is not None:
        return section
    state = release_state(repo, "HEAD")
    rendered = render_section(version, state.previous, state.commits, datetime.date.today(), repo_url)
    return rendered.split("\n", 2)[2]


def check_title(title: str) -> None:
    match = HEADER_RE.match(title.strip())
    if match is None:
        raise ReleaseError(
            f"`{title}` is not a Conventional Commit header; use `type(scope): description`, "
            f"for example `fix(render): keep TAA history on resize`"
        )
    if match["type"] not in ALLOWED_TYPES:
        raise ReleaseError(f"`{match['type']}` is not one of {', '.join(ALLOWED_TYPES)}")


# ---------------------------------------------------------------------------------------------------------------
# Command line
# ---------------------------------------------------------------------------------------------------------------


def write_outputs(outputs: dict[str, str]) -> None:
    """Prints `key=value` lines and, inside GitHub Actions, appends them to the step outputs."""
    lines = [f"{key}={value}" for key, value in outputs.items()]
    print("\n".join(lines))
    output_path = os.environ.get("GITHUB_OUTPUT")
    if output_path:
        with open(output_path, "a", encoding="utf-8") as output:
            output.write("\n".join(lines) + "\n")


def default_repo_url() -> str | None:
    server = os.environ.get("GITHUB_SERVER_URL")
    repository = os.environ.get("GITHUB_REPOSITORY")
    return f"{server}/{repository}" if server and repository else None


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("--root", type=Path, default=Path.cwd(), help="repository root (default: current directory)")
    parser.add_argument("--repo-url", default=default_repo_url(), help="https URL of the repository for links")
    commands = parser.add_subparsers(dest="command", required=True)

    plan_parser = commands.add_parser("plan", help=plan.__doc__.split("\n", 1)[0])
    plan_parser.add_argument("--rev", default="HEAD", help="the main commit to plan for")
    plan_parser.add_argument("--force", action="store_true", help="release the current version if it has no tag")

    prepare_parser = commands.add_parser("prepare", help=prepare.__doc__.split("\n", 1)[0])
    prepare_parser.add_argument("--version", type=Version.parse, help="override the computed version")
    prepare_parser.add_argument("--date", type=datetime.date.fromisoformat, default=datetime.date.today())

    verify_parser = commands.add_parser("verify-pr", help=verify_pr.__doc__.split("\n", 1)[0])
    verify_parser.add_argument("--base", required=True, help="base commit of the pull request")
    verify_parser.add_argument("--head", required=True, help="head commit of the pull request")

    notes_parser = commands.add_parser("notes", help="print the release notes of one version")
    notes_parser.add_argument("--version", type=Version.parse, help="default: the version in Cargo.toml")

    title_parser = commands.add_parser("check-title", help="check a pull request title")
    title_parser.add_argument("title", nargs="?", help="default: the PR_TITLE environment variable")

    commands.add_parser("next-version", help="print the next version, or nothing when no release is due")

    args = parser.parse_args(argv)
    repo = Repo(args.root)
    try:
        if args.command == "plan":
            write_outputs(plan(repo, args.rev, args.force))
        elif args.command == "prepare":
            version = prepare(repo, args.version, args.date, args.repo_url)
            write_outputs({"version": str(version), "tag": version.tag})
        elif args.command == "verify-pr":
            version = verify_pr(repo, args.base, args.head)
            write_outputs({"version": str(version), "tag": version.tag})
        elif args.command == "notes":
            version = args.version or repo.workspace_version(None)
            sys.stdout.write(release_notes(repo, version, args.repo_url))
        elif args.command == "check-title":
            check_title(args.title if args.title is not None else os.environ.get("PR_TITLE", ""))
            print("title follows Conventional Commits")
        elif args.command == "next-version":
            next_version = release_state(repo, "HEAD").next
            print(next_version or "")
    except ReleaseError as error:
        if os.environ.get("GITHUB_ACTIONS"):
            print(f"::error::{error}")
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
