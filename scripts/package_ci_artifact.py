from __future__ import annotations

import argparse
import shutil
import stat
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class Config:
    workspace: Path
    target_os: str
    arch: str
    artifact_name: str
    staging_root: Path
    zju_connect: Path
    wintun: Path | None
    app_name: str


class ParsedArgs(argparse.Namespace):
    workspace: str = ""
    target_os: str = ""
    arch: str = ""
    artifact_name: str = ""
    staging_root: str = ""
    zju_connect: str = ""
    wintun: str | None = None
    app_name: str = "zju-connect-gui"


def parse_args() -> Config:
    parser = argparse.ArgumentParser(
        description="Stage packaged zju-connect-gui artifacts with required runtime layout."
    )
    _ = parser.add_argument(
        "--workspace", required=True, help="Repository workspace root"
    )
    _ = parser.add_argument(
        "--target-os",
        required=True,
        choices=["windows", "linux", "macos"],
        help="Packaging target operating system",
    )
    _ = parser.add_argument(
        "--arch", required=True, help="Packaging target architecture"
    )
    _ = parser.add_argument(
        "--artifact-name", required=True, help="Stable artifact directory name"
    )
    _ = parser.add_argument(
        "--staging-root", required=True, help="Root directory for staged packages"
    )
    _ = parser.add_argument(
        "--zju-connect", required=True, help="Built upstream zju-connect binary path"
    )
    _ = parser.add_argument("--wintun", help="Path to wintun.dll for Windows packages")
    _ = parser.add_argument(
        "--app-name", default="zju-connect-gui", help="Wails app base name"
    )
    namespace = parser.parse_args(namespace=ParsedArgs())
    wintun = Path(namespace.wintun).resolve() if namespace.wintun else None
    return Config(
        workspace=Path(namespace.workspace).resolve(),
        target_os=namespace.target_os,
        arch=namespace.arch,
        artifact_name=namespace.artifact_name,
        staging_root=Path(namespace.staging_root).resolve(),
        zju_connect=Path(namespace.zju_connect).resolve(),
        wintun=wintun,
        app_name=namespace.app_name,
    )


def wails_output_path(workspace: Path, target_os: str, app_name: str) -> Path:
    build_bin = workspace / "build" / "bin"
    if target_os == "windows":
        return build_bin / f"{app_name}.exe"
    if target_os == "macos":
        return build_bin / f"{app_name}.app"
    return build_bin / app_name


def ensure_exists(path: Path, description: str) -> None:
    if not path.exists():
        raise FileNotFoundError(f"Missing {description}: {path}")


def ensure_executable(path: Path) -> None:
    current_mode = path.stat().st_mode
    path.chmod(current_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def copy_app_bundle(source: Path, destination: Path) -> None:
    _ = shutil.copytree(source, destination)


def copy_file(source: Path, destination: Path) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    _ = shutil.copy2(source, destination)


def verify_packaged_layout(stage_dir: Path, target_os: str, app_name: str) -> None:
    if target_os == "macos":
        runtime_root = stage_dir / f"{app_name}.app" / "Contents" / "MacOS"
        required = [
            runtime_root / app_name,
            runtime_root / "bin" / "zju-connect",
        ]
    elif target_os == "windows":
        required = [
            stage_dir / f"{app_name}.exe",
            stage_dir / "bin" / "zju-connect.exe",
            stage_dir / "bin" / "wintun.dll",
        ]
    else:
        required = [
            stage_dir / app_name,
            stage_dir / "bin" / "zju-connect",
        ]

    for path in required:
        ensure_exists(path, "packaged runtime path")


def main() -> None:
    args = parse_args()

    wails_output = wails_output_path(args.workspace, args.target_os, args.app_name)
    zju_connect_source = args.zju_connect
    ensure_exists(wails_output, "Wails build output")
    ensure_exists(zju_connect_source, "built zju-connect helper")

    stage_dir = args.staging_root / args.artifact_name
    if stage_dir.exists():
        shutil.rmtree(stage_dir)
    stage_dir.mkdir(parents=True, exist_ok=True)

    if args.target_os == "macos":
        staged_app = stage_dir / f"{args.app_name}.app"
        copy_app_bundle(wails_output, staged_app)
        runtime_root = staged_app / "Contents" / "MacOS"
        helper_dir = runtime_root / "bin"
        helper_name = "zju-connect"
    else:
        staged_app = stage_dir / wails_output.name
        copy_file(wails_output, staged_app)
        runtime_root = stage_dir
        helper_dir = stage_dir / "bin"
        helper_name = (
            "zju-connect.exe" if args.target_os == "windows" else "zju-connect"
        )

    helper_dir.mkdir(parents=True, exist_ok=True)
    helper_destination = helper_dir / helper_name
    copy_file(zju_connect_source, helper_destination)

    if args.target_os != "windows":
        ensure_executable(helper_destination)
        if staged_app.is_file():
            ensure_executable(staged_app)
        else:
            ensure_executable(runtime_root / args.app_name)

    if args.target_os == "windows":
        if not args.wintun:
            raise ValueError("Windows packaging requires --wintun")
        wintun_source = args.wintun
        ensure_exists(wintun_source, "wintun.dll")
        copy_file(wintun_source, helper_dir / "wintun.dll")

    verify_packaged_layout(stage_dir, args.target_os, args.app_name)

    print(f"staged {args.target_os}/{args.arch} package at {stage_dir}")


if __name__ == "__main__":
    main()
