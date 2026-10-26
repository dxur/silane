#!/usr/bin/env python3
"""
Build silane.vsix — the VS Code / Antigravity IDE extension package.
A .vsix is a ZIP with a specific internal layout understood by VS Code.
"""
import zipfile
import json
import os
import shutil

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
PROJECT_ROOT = os.path.dirname(os.path.dirname(SCRIPT_DIR))
EXT_DIR = os.path.join(SCRIPT_DIR, "silane")
OUT = os.path.join(PROJECT_ROOT, "silane.vsix")

# Read version from package.json
with open(os.path.join(EXT_DIR, "package.json")) as f:
    pkg = json.load(f)

version = pkg["version"]
publisher = pkg["publisher"]
name = pkg["name"]

# The vsixmanifest that VS Code reads
MANIFEST = f"""<?xml version="1.0" encoding="utf-8"?>
<PackageManifest Version="2.0.0" xmlns="http://schemas.microsoft.com/developer/vsx-schema/2011">
  <Metadata>
    <Identity Language="en-US" Id="{name}" Version="{version}" Publisher="{publisher}"/>
    <DisplayName>{pkg['displayName']}</DisplayName>
    <Description xml:space="preserve">{pkg['description']}</Description>
    <Categories>Programming Languages</Categories>
    <Tags>silane,hdl,hardware,verilog</Tags>
  </Metadata>
  <Installation>
    <InstallationTarget Id="Microsoft.VisualStudio.Code"/>
  </Installation>
  <Dependencies/>
  <Assets>
    <Asset Type="Microsoft.VisualStudio.Code.Manifest" Path="extension/package.json" Addressable="true"/>
  </Assets>
</PackageManifest>
"""

CONTENT_TYPES = """<?xml version="1.0" encoding="utf-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension=".json" ContentType="application/json"/>
  <Default Extension=".vsixmanifest" ContentType="text/xml"/>
</Types>
"""

with zipfile.ZipFile(OUT, 'w', zipfile.ZIP_DEFLATED) as zf:
    zf.writestr("[Content_Types].xml", CONTENT_TYPES)
    zf.writestr("extension.vsixmanifest", MANIFEST)

    for dirpath, dirnames, filenames in os.walk(EXT_DIR):
        for filename in filenames:
            full_path = os.path.join(dirpath, filename)
            rel = os.path.relpath(full_path, EXT_DIR)
            zf.write(full_path, f"extension/{rel}")

print(f"Built: {OUT}")
print(f"  Publisher: {publisher}")
print(f"  Extension: {name} v{version}")
