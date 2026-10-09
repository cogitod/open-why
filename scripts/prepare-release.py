#!/usr/bin/env python3
"""Build and exercise a local candidate. Does not tag, upload, sign, or publish."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile


def run(*args, **kwargs):
    return subprocess.run(args, check=True, text=True, **kwargs)


def require(condition, message):
    if not condition:
        sys.exit(message)


def output(*args, **kwargs):
    return subprocess.check_output(args, text=True, **kwargs).strip()


root = Path(__file__).resolve().parent.parent
os.chdir(root)
if len(sys.argv) != 2:
    sys.exit("usage: python3 scripts/prepare-release.py /absolute/new-output-directory")
out = Path(sys.argv[1])
if not out.is_absolute() or out.exists():
    sys.exit("output must be an absolute, new directory")
if output("git", "status", "--porcelain"):
    sys.exit("release candidates require a clean committed source tree")
revision = output("git", "rev-parse", "HEAD")
metadata = json.loads(output("cargo", "metadata", "--locked", "--format-version", "1", "--no-default-features"))
package = next(p for p in metadata["packages"] if p["id"] == metadata["resolve"]["root"])
version = package["version"]
host = next(line.split(": ", 1)[1] for line in output("rustc", "-vV").splitlines() if line.startswith("host:"))
out.mkdir(parents=True)
assets = out / "assets"
assets.mkdir()
run("cargo", "package", "--locked", "--no-verify")
archive = Path(metadata["target_directory"]) / "package" / ("open-why-" + version + ".crate")
shutil.copy2(archive, assets / archive.name)
source = out / "source"
source.mkdir()
with tarfile.open(archive) as tar:
    # Cargo's archive is our own output, but reject unsafe member types and paths.
    for member in tar.getmembers():
        path = Path(member.name)
        if path.is_absolute() or ".." in path.parts or not (member.isfile() or member.isdir()):
            sys.exit("unsafe source archive member")
    tar.extractall(source)
checkout = source / ("open-why-" + version)
install = out / "install"
run("cargo", "install", "--locked", "--path", str(checkout), "--bin", "why", "--no-default-features", "--root", str(install), "--target-dir", str(out / "target"))
binary = install / "bin" / "why"
name = "open-why-" + version + "-" + host + "-lexical.tar.gz"
with tarfile.open(assets / name, "w:gz") as tar:
    tar.add(binary, arcname="why")
    for file in ["LICENSE", "NOTICE", "README.md"]:
        if (checkout / file).exists():
            tar.add(checkout / file, arcname=file)
unpacked = out / "unpacked-binary"
unpacked.mkdir()
with tarfile.open(assets / name) as tar:
    tar.extractall(unpacked)
binary = unpacked / "why"
require(output(str(binary), "--version") == "why " + version, "artifact version mismatch")
# Exercise the installed artifact, not a source-tree executable.
env = dict(os.environ)
for key in list(env):
    if key.startswith("OPEN_WHY_"):
        del env[key]
env["HOME"] = str(out / "isolated-home")
Path(env["HOME"]).mkdir()
env["OPEN_WHY_BIN"] = str(binary)
run("bash", str(checkout / "examples/quickstart.sh"), str(out / "demo"), "generic", env=env)
config = json.loads((out / "demo/mcp.json").read_text())
env.update(config["env"])
run(str(binary), "init", str(out / "demo/repository"), env=env)
require("SQLite" in output(str(binary), "search", "SQLite", "--scope", str(out / "demo/repository"), env=env), "artifact retrieval failed")
run(str(binary), "capture", "--id", "release-smoke", "--title", "Release smoke evidence", "--content", "Synthetic artifact validation", env=env)
run(str(binary), "backup", "--to", str(out / "snapshot.db"), env=env)
run(str(binary), "verify-backup", str(out / "snapshot.db"), env=env)
run(str(binary), "restore", str(out / "snapshot.db"), "--to", str(out / "restored.db"), env=env)
env["OPEN_WHY_DB"] = str(out / "restored.db")
require("Synthetic artifact validation" in output(str(binary), "get", "release-smoke", env=env), "restored evidence missing")
request = json.dumps({"jsonrpc":"2.0", "id":1, "method":"initialize"}) + "\n"
response = run(str(binary), "serve", input=request, stdout=subprocess.PIPE, env=env)
require(json.loads(response.stdout)["result"]["serverInfo"]["version"] == version, "artifact MCP version mismatch")
# List locked dependencies without leaking local filesystem paths into artifacts.
components = [{"type":"library", "name":p["name"], "version":p["version"],
               "licenses":[{"expression":p["license"]}] if p.get("license") else []}
              for p in metadata["packages"]]
(assets / ("sbom-" + host + ".cdx.json")).write_text(json.dumps({"bomFormat":"CycloneDX", "specVersion":"1.5", "version":1,
    "metadata":{"component":{"type":"application","name":"open-why","version":version}},
    "components":components}, indent=2) + "\n")
(assets / ("build-" + host + ".json")).write_text(json.dumps({"version":version, "revision":revision, "host":host,
    "features":[], "rustc":output("rustc","--version"), "cargo":output("cargo","--version"),
    "source_archive":archive.name, "artifact_tests":"install, version, demo, index, capture, search, MCP initialize, backup, verify, restore, get",
    "provenance":"local unsigned build record; not an attestation",
    "sbom_scope":"Cargo lexical resolution (includes development/build dependencies); OS libraries excluded"}, indent=2) + "\n")
lines = [hashlib.sha256(p.read_bytes()).hexdigest() + "  " + p.name for p in sorted(assets.iterdir())]
(assets / ("SHA256SUMS-" + host)).write_text("\n".join(lines) + "\n")
print("Validated local candidate:", assets)
