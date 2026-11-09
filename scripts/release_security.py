#!/usr/bin/env python3
"""Read-only release checks. Does not sign, notarize, install or publish."""
import argparse
import json
import plistlib
import re
import subprocess
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

ANDROID = "{http://schemas.android.com/apk/res/android}"
DOMAINS = {"root", "file", "database", "sharedpref", "external", "device_root", "device_file", "device_database", "device_sharedpref"}
PERMISSIONS = {"INTERNET", "RECORD_AUDIO", "MODIFY_AUDIO_SETTINGS", "RECEIVE_BOOT_COMPLETED", "WAKE_LOCK", "POST_NOTIFICATIONS"}
UNSAFE_ENTITLEMENTS = {"com.apple.security.get-task-allow", "com.apple.security.cs.allow-unsigned-executable-memory", "com.apple.security.cs.disable-library-validation", "com.apple.security.cs.disable-executable-page-protection", "com.apple.security.cs.allow-dyld-environment-variables"}


def android_findings(manifest, paths, extraction):
    issues = []
    app = manifest.find("application")
    if app is None:
        return ["Android manifest has no application"]
    package = manifest.get("package", "")
    if app.get(ANDROID + "debuggable", "false") != "false":
        issues.append("Android release is debuggable")
    for name in ("allowBackup", "fullBackupContent", "usesCleartextTraffic"):
        if app.get(ANDROID + name) != "false":
            issues.append(f"Android release must set {name}=false")
    if app.get(ANDROID + "dataExtractionRules") != "@xml/data_extraction_rules":
        issues.append("Android release does not select the reviewed data extraction rules")
    grants = [(e.tag, e.get("path")) for e in paths]
    if grants != [("external-files-path", "Pictures/")]:
        issues.append("FileProvider must share only app-owned Pictures/")
    providers = [e for e in app.findall("provider") if e.get(ANDROID + "name") == "androidx.core.content.FileProvider"]
    if len(providers) != 1 or any(p.get(ANDROID + "exported") != "false" or p.get(ANDROID + "authorities") != package + ".fileprovider" or not any(m.get(ANDROID + "name") == "android.support.FILE_PROVIDER_PATHS" and m.get(ANDROID + "resource") == "@xml/file_paths" for m in p.findall("meta-data")) for p in providers):
        issues.append("FileProvider configuration differs from the reviewed private provider")
    for mode in ("cloud-backup", "device-transfer"):
        rules = extraction.find(mode)
        excluded = set() if rules is None else {e.get("domain") for e in rules.findall("exclude") if e.get("path") == "."}
        if excluded != DOMAINS or (rules is not None and rules.findall("include")):
            issues.append(f"Android {mode} must exclude every app data domain")
    if any(e.tag not in {"cloud-backup", "device-transfer"} for e in extraction):
        issues.append("Unreviewed Android data extraction mode")
    for component in app:
        if component.tag not in {"activity", "activity-alias", "service", "receiver", "provider"}:
            continue
        exported = component.get(ANDROID + "exported")
        if exported == "false" or (exported is None and not component.findall("intent-filter")):
            continue
        name = component.get(ANDROID + "name", "")
        launcher = component.tag == "activity" and name in {".MainActivity", package + ".MainActivity"} and any(
            f.find("action[@" + ANDROID + "name='android.intent.action.MAIN']") is not None and
            f.find("category[@" + ANDROID + "name='android.intent.category.LAUNCHER']") is not None
            for f in component.findall("intent-filter"))
        profiler = component.tag == "receiver" and name == "androidx.profileinstaller.ProfileInstallReceiver" and component.get(ANDROID + "permission") == "android.permission.DUMP"
        if not launcher and not profiler:
            issues.append(f"Unreviewed exported Android component: {name}")
    allowed = {"android.permission." + p for p in PERMISSIONS} | {package + ".DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION"}
    receiver_permission = package + ".DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION"
    if any(e.get(ANDROID + "name") == receiver_permission for e in manifest.findall("uses-permission")):
        if not any(e.get(ANDROID + "name") == receiver_permission and e.get(ANDROID + "protectionLevel") == "signature" for e in manifest.findall("permission")):
            issues.append("Dynamic receiver permission must be signature-protected")
    for element in manifest:
        if element.tag.startswith("uses-permission") and element.get(ANDROID + "name") not in allowed:
            issues.append("Unreviewed Android permission: " + str(element.get(ANDROID + "name")))
    return issues


def macos_metadata_findings(display, entitlements):
    issues = []
    if not re.search(r"^Authority=Developer ID Application:", display, re.M):
        issues.append("macOS release needs a Developer ID Application signature")
    if not re.search(r"^TeamIdentifier=(?!not set$)\S+", display, re.M):
        issues.append("macOS release has no signing team")
    flags = re.search(r"flags=0x([0-9a-fA-F]+)", display)
    if not flags or not int(flags[1], 16) & 0x10000:
        issues.append("macOS release lacks hardened runtime")
    if not re.search(r"^Timestamp=.+", display, re.M):
        issues.append("macOS release lacks a secure signing timestamp")
    for name in sorted(UNSAFE_ENTITLEMENTS):
        if entitlements.get(name):
            issues.append("Unapproved macOS entitlement: " + name)
    return issues


def run(*command):
    return subprocess.run(command, capture_output=True, timeout=45, check=False)


def macos_findings(app):
    verified = run("codesign", "--verify", "--deep", "--strict", str(app))
    if verified.returncode:
        return ["macOS code signature verification failed"]
    display = run("codesign", "--display", "--verbose=4", str(app))
    ent = run("codesign", "--display", "--entitlements", ":-", str(app))
    if display.returncode or ent.returncode:
        return ["Could not inspect macOS signing metadata"]
    entitlements = plistlib.loads(ent.stdout) if ent.stdout.strip() else {}
    issues = macos_metadata_findings(display.stderr.decode(errors="replace"), entitlements)
    info = plistlib.loads((app / "Contents/Info.plist").read_bytes())
    if not info.get("NSMicrophoneUsageDescription"):
        issues.append("macOS voice capture needs a microphone purpose string")
    if issues:
        return issues
    if run("xcrun", "stapler", "validate", str(app)).returncode:
        issues.append("macOS release has no valid stapled notarization ticket")
    if run("spctl", "--assess", "--type", "execute", str(app)).returncode:
        issues.append("Gatekeeper did not accept the macOS release")
    return issues


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--android-manifest", type=Path, help="Actual merged release manifest XML")
    parser.add_argument("--android-res", type=Path, help="Merged packaged_res directory from the same Android release build")
    parser.add_argument("--macos-app", type=Path, help="Final Developer ID signed and notarized .app")
    args = parser.parse_args()
    if not args.macos_app and not args.android_manifest:
        parser.error("select at least one release artifact")
    if bool(args.android_manifest) != bool(args.android_res):
        parser.error("Android manifest and resources must be supplied together")
    try:
        issues = []
        if args.android_manifest:
            issues += android_findings(ET.parse(args.android_manifest).getroot(), ET.parse(args.android_res / "xml/file_paths.xml").getroot(), ET.parse(args.android_res / "xml/data_extraction_rules.xml").getroot())
        if args.macos_app:
            issues += macos_findings(args.macos_app)
    except (OSError, ValueError, ET.ParseError, subprocess.TimeoutExpired) as exc:
        issues = ["Release inspection failed: " + str(exc)]
    print(json.dumps({"passed": not issues, "issues": issues}, indent=2))
    return int(bool(issues))


if __name__ == "__main__":
    sys.exit(main())
