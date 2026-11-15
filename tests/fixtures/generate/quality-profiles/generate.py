#!/usr/bin/env python3
"""Run unmodified Tanukarr profile algorithms offline in the cached .NET SDK image."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import xml.etree.ElementTree as ET

workspace = Path(__file__).resolve().parents[4]
reference = Path(sys.argv[1]).resolve()
revision = subprocess.check_output(["git", "-C", str(reference), "rev-parse", "HEAD"], text=True).strip()
out = workspace / ".dev/verification/quality-profiles"
out.mkdir(parents=True, exist_ok=True)
subprocess.run([
    "podman", "run", "--rm", "--network", "none", "--cpus", "2", "--memory", "1g",
    "-e", "DOTNET_CLI_TELEMETRY_OPTOUT=1", "-e", "DOTNET_SKIP_FIRST_TIME_EXPERIENCE=1",
    "-v", f"{Path(__file__).resolve().parent}:/harness:ro",
    "-v", f"{out}:/out:rw", "-v", f"{reference}:/reference:ro",
    "mcr.microsoft.com/dotnet/sdk:10.0.402", "dotnet", "run", "--project", "/harness/Reference.csproj",
    "-p:ReferenceRoot=/reference", "-p:BaseIntermediateOutputPath=/out/obj/", "-p:OutputPath=/out/bin/",
    "--", "/out/tanukarr-reference.json", revision,
], check=True)
data = json.loads((out / "tanukarr-reference.json").read_text())
paths = [c.attrib["Include"].replace("$(ReferenceRoot)/", "")
         for c in ET.parse(Path(__file__).with_name("Reference.csproj")).iter("Compile")]
data["sourceFiles"] = {p: hashlib.sha256((reference / p).read_bytes()).hexdigest() for p in paths}
destination = workspace / "tests/fixtures/quality-profiles/tanukarr-reference.json"
destination.parent.mkdir(parents=True, exist_ok=True)
destination.write_text(json.dumps(data, indent=2) + "\n")
print(f"Saved {destination.relative_to(workspace)}")
