#!/usr/bin/env python3
"""Capture actual Tanukarr filename and regex results using cached images, offline."""
import hashlib
import json
from pathlib import Path
import subprocess

workspace = Path(__file__).resolve().parents[4]
out = workspace / ".dev/verification/rename-media"
out.mkdir(parents=True, exist_ok=True)
source_image = "ghcr.io/kvasfan/tanukarr:0.1.5"
image_id = subprocess.check_output(["podman", "image", "inspect", source_image, "--format", "{{.Id}}"], text=True).strip()
app = out / ("reference-" + image_id.removeprefix("sha256:")[:12])
if not (app / "Sonarr.Core.dll").exists():
    container = subprocess.check_output(["podman", "create", "--network", "none", source_image], text=True).strip()
    try:
        subprocess.run(["podman", "cp", container + ":/app", str(app)], check=True)
    finally:
        subprocess.run(["podman", "rm", container], check=True, stdout=subprocess.DEVNULL)
subprocess.run([
    "podman", "run", "--rm", "--network", "none", "--cpus", "2", "--memory", "1g",
    "-e", "DOTNET_CLI_TELEMETRY_OPTOUT=1",
    "-v", f"{Path(__file__).resolve().parent}:/harness:ro", "-v", f"{out}:/out:rw",
    "-v", f"{app}:/reference-app:ro", "mcr.microsoft.com/dotnet/sdk:10.0.402",
    "dotnet", "run", "--project", "/harness/Reference.csproj", "-p:ReferenceApp=/reference-app",
    "-p:BaseIntermediateOutputPath=/out/obj/", "-p:OutputPath=/out/bin/", "--", "/out/tanukarr-naming.json",
], check=True)
data = json.loads((out / "tanukarr-naming.json").read_text())
data["imageId"] = image_id
data["assemblySha256"] = {n: hashlib.sha256((app / n).read_bytes()).hexdigest()
                          for n in ["Sonarr.Core.dll", "Sonarr.Common.dll"]}
target = workspace / "tests/fixtures/naming/tanukarr-0.1.5.json"
target.parent.mkdir(parents=True, exist_ok=True)
target.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n")
print(f"Saved {len(data['cases'])} filenames and {len(data['regexCases'])} regex cases to {target.relative_to(workspace)}")
