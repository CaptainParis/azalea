# Support for generating data for 1.21.4 (protocol 769) on this codebase.
#
# 1.21.4 jars are obfuscated, while the pinned azalea-burger and
# azalea-pumpkin-extractor target 26.1+ (unobfuscated). For legacy versions we
# apply our patches in `codegen/patches/<version>/` to the pinned tool commits:
# the extractor is built with Fabric loom and Mojang mappings, and burger runs on
# the Mojang-mapped jar that loom produces.

import glob
import os
import shutil
import subprocess

from .utils import get_dir_location

LEGACY_VERSIONS = {
    "1.21.4": {
        "java_home_env": "AZALEA_CODEGEN_JAVA21_HOME",
        "mojmap_jar_glob": os.path.expanduser(
            "~/.gradle/caches/fabric-loom/minecraftMaven/net/minecraft/minecraft-merged/"
            "1.21.4-loom.mappings.1_21_4.layered+hash.*-v2/minecraft-merged-1.21.4-*.jar"
        ),
    },
}


def is_legacy(version_id: str) -> bool:
    return version_id in LEGACY_VERSIONS


def java_home(version_id: str) -> str:
    env = LEGACY_VERSIONS[version_id]["java_home_env"]
    home = os.environ.get(env)
    if not home or not os.path.exists(os.path.join(home, "bin", "java")):
        raise RuntimeError(
            f"{version_id} needs Java 21: set {env} to a Java 21 home (for example `mise where java@21`)"
        )
    return home


def java_env(version_id: str) -> dict:
    env = dict(os.environ)
    home = java_home(version_id)
    env["JAVA_HOME"] = home
    env["PATH"] = os.path.join(home, "bin") + os.pathsep + env.get("PATH", "")
    return env


def patched_tool(base_dir: str, tool: str, version_id: str) -> str:
    """A copy of `base_dir` (at its pinned commit) with our patch for `tool` applied."""
    target = f"{base_dir}-{version_id}"
    patch = get_dir_location(f"patches/{version_id}/{tool}.patch")
    marker = os.path.join(target, ".azalea-hunt-patched")
    if os.path.exists(marker):
        return target
    if os.path.exists(target):
        shutil.rmtree(target)
    subprocess.run(["git", "clone", "-q", base_dir, target], check=True)
    from . import download

    pinned = {"burger": download.BURGER_COMMIT, "pumpkin-extractor": download.PUMPKIN_EXTRACTOR_COMMIT}[tool]
    subprocess.run(["git", "-C", target, "checkout", "-q", pinned], check=True)
    subprocess.run(
        ["git", "-C", target, "-c", "user.name=codegen", "-c", "user.email=codegen@local", "am", "-q", patch],
        check=True,
    )
    venv = os.path.join(base_dir, "venv")
    if os.path.exists(venv):
        os.symlink(venv, os.path.join(target, "venv"))
    open(marker, "w").close()
    return target


def mojmap_jar(version_id: str) -> str:
    """Mojang-mapped merged jar produced by loom while building the patched extractor."""
    out = get_dir_location(f"__cache__/client-{version_id}-mojmap.jar")
    if os.path.exists(out):
        return out
    matches = [m for m in glob.glob(LEGACY_VERSIONS[version_id]["mojmap_jar_glob"]) if not m.endswith("-sources.jar")]
    if not matches:
        raise RuntimeError(
            f"no Mojang-mapped {version_id} jar found; run the pumpkin extractor for {version_id} first"
        )
    shutil.copy(sorted(matches)[0], out)
    return out
