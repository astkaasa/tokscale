#!/usr/bin/env bash
set -euo pipefail

EXPECTED_VERSION="${1:-}"
if [[ "${EXPECTED_VERSION}" == "--expect-version" ]]; then
  if [[ -z "${2:-}" ]]; then
    echo "--expect-version requires a value" >&2
    exit 2
  fi
  EXPECTED_VERSION="${2}"
fi

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

python3 - <<'PY' "${EXPECTED_VERSION}"
import pathlib
import sys

expected_version = sys.argv[1] or None
root = pathlib.Path(".")

try:
    import tomllib
except ModuleNotFoundError:
    raise SystemExit("Python tomllib is required (Python 3.11+)")

with (root / "Cargo.toml").open("rb") as cargo_file:
    cargo_data = tomllib.load(cargo_file)

with (root / "Cargo.lock").open("rb") as cargo_lock_file:
    cargo_lock_data = tomllib.load(cargo_lock_file)

workspace_version = cargo_data.get("workspace", {}).get("package", {}).get("version")
if not workspace_version:
    raise SystemExit("Could not find [workspace.package] version in Cargo.toml")

if expected_version and workspace_version != expected_version:
    raise SystemExit(
        f"Cargo workspace version mismatch: expected {expected_version}, found {workspace_version}"
    )

workspace_packages = {
    pathlib.Path(member).name
    for member in cargo_data.get("workspace", {}).get("members", [])
}
if not workspace_packages:
    raise SystemExit("Cargo workspace has no members")

lock_packages = {
    package.get("name"): package.get("version")
    for package in cargo_lock_data.get("package", [])
    if package.get("name") in workspace_packages and "source" not in package
}

errors = []
for package_name in sorted(workspace_packages):
    lock_version = lock_packages.get(package_name)
    if lock_version is None:
        errors.append(f"Cargo.lock missing package {package_name}")
    elif lock_version != workspace_version:
        errors.append(
            f"Cargo.lock package {package_name}: expected {workspace_version}, found {lock_version}"
        )

if errors:
    raise SystemExit("Version coherence check failed:\n- " + "\n- ".join(errors))

print(f"Cargo version coherence OK: {workspace_version}")
PY
