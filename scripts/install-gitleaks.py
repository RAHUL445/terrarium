#!/usr/bin/env python3
"""Install a SHA-256 pinned upstream Gitleaks binary; never extract arbitrary paths."""
import argparse
import hashlib
import io
from pathlib import Path
import platform
import tarfile
import urllib.request

VERSION = "8.30.1"
ASSETS = {
    ("Linux", "x86_64"): ("linux_x64", "551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb"),
    ("Darwin", "arm64"): ("darwin_arm64", "b40ab0ae55c505963e365f271a8d3846efbc170aa17f2607f13df610a9aeb6a5"),
}

def install(destination):
    suffix, expected = ASSETS[(platform.system(), platform.machine())]
    url = f"https://github.com/gitleaks/gitleaks/releases/download/v{VERSION}/gitleaks_{VERSION}_{suffix}.tar.gz"
    with urllib.request.urlopen(url, timeout=60) as response:
        data = response.read(32 * 1024 * 1024 + 1)
    if len(data) > 32 * 1024 * 1024 or hashlib.sha256(data).hexdigest() != expected:
        raise ValueError("Gitleaks download failed integrity verification")
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
        member = archive.getmember("gitleaks")
        if not member.isfile() or member.size > 64 * 1024 * 1024:
            raise ValueError("Invalid Gitleaks archive")
        binary = archive.extractfile(member).read()
    destination.mkdir(parents=True, exist_ok=True)
    output = destination / "gitleaks"
    if output.exists() or output.is_symlink():
        raise ValueError("Choose a fresh security tools directory")
    with output.open("xb") as target:
        target.write(binary)
    output.chmod(0o755)
    print(f"Installed verified Gitleaks {VERSION}")

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", type=Path)
    install(parser.parse_args().destination)
